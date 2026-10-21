//! Thumbnail and preview generation with an on-disk cache.
//!
//! Pipeline: source JPEG (or RAF preview) → DCT-domain downscale while
//! decoding (1/2, 1/4, 1/8) → SIMD Lanczos resize to the exact size →
//! EXIF orientation applied to pixels → JPEG encode → atomic write.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use fast_image_resize::{self as fr, images::Image};
use serde::{Deserialize, Serialize};

use crate::{Error, Kind, meta, raf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    /// Grid tiles. 512px covers a ~256pt tile on a 2x display.
    Thumb,
    /// Loupe view. Large enough for a 1440p/4K window without the original.
    Preview,
}

impl Size {
    pub fn long_edge(self) -> u32 {
        match self {
            Size::Thumb => 512,
            Size::Preview => 2560,
        }
    }

    fn quality(self) -> u8 {
        match self {
            Size::Thumb => 80,
            Size::Preview => 86,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Size::Thumb => "t",
            Size::Preview => "p",
        }
    }
}

/// What the cache needs to know about a source file.
#[derive(Debug, Clone)]
pub struct Source<'a> {
    pub path: &'a Path,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
}

pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Cache location, keyed by path + size + mtime so edits invalidate it.
    pub fn path_for(&self, src: &Source, size: Size) -> PathBuf {
        let key = format!(
            "{}|{}|{}",
            src.path.to_string_lossy().to_lowercase(),
            src.size,
            src.mtime
        );
        let hash = xxhash_rust::xxh3::xxh3_64(key.as_bytes());
        self.root
            .join(format!("{:02x}", hash >> 56))
            .join(format!("{hash:016x}_{}.jpg", size.suffix()))
    }

    pub fn get(&self, src: &Source, size: Size) -> Option<PathBuf> {
        let path = self.path_for(src, size);
        path.is_file().then_some(path)
    }

    /// Returns the cached rendition, generating it first if necessary.
    pub fn ensure(&self, src: &Source, size: Size) -> Result<PathBuf, Error> {
        let path = self.path_for(src, size);
        if path.is_file() {
            return Ok(path);
        }
        if !src.kind.has_jpeg_preview() {
            return Err(Error::Unsupported(src.kind));
        }

        // A preview is a much better source than a 40MP original when
        // building the thumbnail, so reuse it if it already exists.
        let preview = (size == Size::Thumb)
            .then(|| self.get(src, Size::Preview))
            .flatten();
        let bytes = match &preview {
            Some(p) => fs::read(p)?,
            None => source_jpeg(src)?,
        };
        let out = render(&bytes, size.long_edge(), size.quality(), preview.is_none())?;
        write_atomic(&path, &out.jpeg)?;
        Ok(path)
    }

    /// Stores renditions built ahead of time (see [`render_all`]).
    pub fn store(&self, src: &Source, renditions: &Renditions) -> std::io::Result<()> {
        write_atomic(&self.path_for(src, Size::Preview), &renditions.preview.jpeg)?;
        write_atomic(&self.path_for(src, Size::Thumb), &renditions.thumb.jpeg)
    }

    /// Total bytes on disk, for the settings screen.
    pub fn disk_usage(&self) -> u64 {
        fn walk(dir: &Path) -> u64 {
            fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => walk(&e.path()),
                    Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                    Err(_) => 0,
                })
                .sum()
        }
        walk(&self.root)
    }

    pub fn clear(&self) -> std::io::Result<()> {
        fs::remove_dir_all(&self.root)?;
        fs::create_dir_all(&self.root)
    }
}

/// The JPEG bytes a rendition is built from.
pub fn source_jpeg(src: &Source) -> Result<Vec<u8>, Error> {
    Ok(match src.kind {
        Kind::Raf => raf::read_embedded_jpeg(src.path)?,
        Kind::Jpeg => fs::read(src.path)?,
        kind => return Err(Error::Unsupported(kind)),
    })
}

pub struct Rendered {
    pub jpeg: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub struct Renditions {
    pub preview: Rendered,
    pub thumb: Rendered,
}

/// Builds both renditions in memory. Imports use this on the fast local copy
/// of a file while it is still uploading to the network library.
pub fn render_all(jpeg: &[u8]) -> Result<Renditions, Error> {
    let preview = render(
        jpeg,
        Size::Preview.long_edge(),
        Size::Preview.quality(),
        true,
    )?;
    let thumb = render(
        &preview.jpeg,
        Size::Thumb.long_edge(),
        Size::Thumb.quality(),
        false,
    )?;
    Ok(Renditions { preview, thumb })
}

/// Decodes `jpeg`, fits it inside `long_edge`, and re-encodes it.
/// `apply_orientation` is false when the input is one of our own renditions,
/// which are already upright and carry no EXIF.
pub fn render(
    jpeg: &[u8],
    long_edge: u32,
    quality: u8,
    apply_orientation: bool,
) -> Result<Rendered, Error> {
    let orientation = if apply_orientation {
        exif::Reader::new()
            .read_from_container(&mut Cursor::new(jpeg))
            .map(|e| meta::orientation(&e))
            .unwrap_or(1)
    } else {
        1
    };

    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
    decoder.read_info().map_err(decode_err)?;
    let info = decoder
        .info()
        .ok_or_else(|| Error::Decode("missing frame header".into()))?;
    let (target_w, target_h) = fit(info.width as u32, info.height as u32, long_edge);
    // Let the decoder skip most of the IDCT work; it picks the smallest
    // factor that is still at least as large as what we asked for.
    let (dw, dh) = decoder
        .scale(target_w as u16, target_h as u16)
        .map_err(decode_err)?;
    let pixels = decoder.decode().map_err(decode_err)?;
    let rgb = to_rgb(pixels, info.pixel_format);

    let (dw, dh) = (dw as u32, dh as u32);
    let rgb = if (dw, dh) == (target_w, target_h) {
        rgb
    } else {
        let src = Image::from_vec_u8(dw, dh, rgb, fr::PixelType::U8x3)
            .map_err(|e| Error::Decode(e.to_string()))?;
        let mut dst = Image::new(target_w, target_h, fr::PixelType::U8x3);
        let options = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
        fr::Resizer::new()
            .resize(&src, &mut dst, &options)
            .map_err(|e| Error::Decode(e.to_string()))?;
        dst.into_vec()
    };

    let (rgb, width, height) = orient(rgb, target_w, target_h, orientation);
    let mut jpeg = Vec::with_capacity((width * height / 4) as usize);
    let mut encoder = jpeg_encoder::Encoder::new(&mut jpeg, quality);
    encoder.set_progressive(false);
    encoder
        .encode(
            &rgb,
            width as u16,
            height as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(Rendered {
        jpeg,
        width,
        height,
    })
}

fn decode_err(e: jpeg_decoder::Error) -> Error {
    Error::Decode(e.to_string())
}

/// Scales `(w, h)` down (never up) so the longer side is `long_edge`.
pub fn fit(w: u32, h: u32, long_edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= long_edge {
        return (w.max(1), h.max(1));
    }
    let scale = long_edge as f64 / long as f64;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

fn to_rgb(pixels: Vec<u8>, format: jpeg_decoder::PixelFormat) -> Vec<u8> {
    use jpeg_decoder::PixelFormat as P;
    match format {
        P::RGB24 => pixels,
        P::L8 => pixels.iter().flat_map(|&l| [l, l, l]).collect(),
        P::L16 => pixels
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0]])
            .collect(),
        // Adobe-style inverted CMYK, which is what jpeg-decoder hands back.
        P::CMYK32 => pixels
            .chunks_exact(4)
            .flat_map(|c| {
                let k = c[3] as u16;
                [
                    (c[0] as u16 * k / 255) as u8,
                    (c[1] as u16 * k / 255) as u8,
                    (c[2] as u16 * k / 255) as u8,
                ]
            })
            .collect(),
    }
}

/// Applies an EXIF orientation (1–8) to packed RGB pixels.
pub fn orient(rgb: Vec<u8>, w: u32, h: u32, orientation: u16) -> (Vec<u8>, u32, u32) {
    if orientation <= 1 || orientation > 8 {
        return (rgb, w, h);
    }
    let (w, h) = (w as usize, h as usize);
    let (ow, oh) = if meta::swaps_axes(orientation) {
        (h, w)
    } else {
        (w, h)
    };
    let mut out = vec![0u8; rgb.len()];
    for y in 0..h {
        for x in 0..w {
            let (nx, ny) = match orientation {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (h - 1 - y, x),
                7 => (h - 1 - y, w - 1 - x),
                _ => (y, w - 1 - x), // 8
            };
            let s = (y * w + x) * 3;
            let d = (ny * ow + nx) * 3;
            out[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
        }
    }
    (out, ow as u32, oh as u32)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().expect("cache paths always have a parent");
    fs::create_dir_all(dir)?;
    // Unique per thread so concurrent requests for one key never collide.
    let tmp = path
        .with_extension(format!("{:?}.tmp", std::thread::current().id()).replace(['(', ')'], ""));
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        // Lost a race with another worker writing the same rendition.
        Err(_) if path.is_file() => fs::remove_file(&tmp),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_keeps_aspect() {
        assert_eq!(fit(6240, 4160, 512), (512, 341));
        assert_eq!(fit(4160, 6240, 512), (341, 512));
        assert_eq!(fit(300, 200, 512), (300, 200));
    }

    #[test]
    fn orientation_6_rotates_clockwise() {
        // 2x1 image: red, green.
        let rgb = vec![255, 0, 0, 0, 255, 0];
        let (out, w, h) = orient(rgb, 2, 1, 6);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![255, 0, 0, 0, 255, 0]);

        let rgb = vec![255, 0, 0, 0, 255, 0];
        let (out, w, h) = orient(rgb, 2, 1, 8);
        assert_eq!((w, h), (1, 2));
        assert_eq!(out, vec![0, 255, 0, 255, 0, 0]);

        let rgb = vec![255, 0, 0, 0, 255, 0];
        let (out, _, _) = orient(rgb, 2, 1, 3);
        assert_eq!(out, vec![0, 255, 0, 255, 0, 0]);
    }
}
