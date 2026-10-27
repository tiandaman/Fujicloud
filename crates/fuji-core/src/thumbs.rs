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

use crate::{Error, Kind, decode, meta, raf};

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

    /// Where a movie converted for playback is kept.
    pub fn movie_path(&self, src: &Source) -> PathBuf {
        let jpeg = self.path_for(src, Size::Preview);
        let name = jpeg.file_name().unwrap_or_default().to_string_lossy();
        self.root
            .join("movies")
            .join(name.replace(&format!("_{}.jpg", Size::Preview.suffix()), ".mp4"))
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
        crate::guard(move || self.generate(src, size, path))
    }

    fn generate(&self, src: &Source, size: Size, path: PathBuf) -> Result<PathBuf, Error> {
        match self.generate_from_picture(src, size, &path) {
            // A damaged or unusual file can usually still show the small
            // picture embedded in it (EXIF thumbnail, RAW or DNG preview).
            Err(e @ (Error::Decode(_) | Error::Unsupported(_))) if src.kind != Kind::Video => {
                match embedded_renditions(src.path)? {
                    Some(r) => {
                        log::warn!("{}: {e}; showing its embedded preview", src.path.display());
                        self.store(src, &r)?;
                        Ok(path)
                    }
                    None => Err(e),
                }
            }
            other => other.map(|()| path),
        }
    }

    fn generate_from_picture(&self, src: &Source, size: Size, path: &Path) -> Result<(), Error> {
        // HEIF and PSD/TIFF edits are expensive to decode (and may be large
        // network reads): make both renditions from one decode.
        if !matches!(src.kind, Kind::Jpeg | Kind::Raf) {
            return self.store_decoded(src, decode::image(src.path, src.kind)?);
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
        if preview.is_none() && decode::disguise(&bytes).is_some() {
            return self.store_decoded(src, decode::disguised(&bytes)?);
        }
        let out = render(&bytes, size.long_edge(), size.quality(), preview.is_none())?;
        write_atomic(path, &out.jpeg)?;
        Ok(())
    }

    /// Stores both renditions of an already-decoded, upright image.
    fn store_decoded(&self, src: &Source, image: decode::Rgb) -> Result<(), Error> {
        Ok(self.store(src, &render_rgb_all(image)?)?)
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

/// Renditions from the largest JPEG picture embedded in a file, for files
/// whose own picture can't be read. Turned as the file says.
fn embedded_renditions(path: &Path) -> Result<Option<Renditions>, Error> {
    // Reading a huge file whole just for this isn't worth it.
    if fs::metadata(path)?.len() > 400 << 20 {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let Some(at) = decode::embedded_jpeg(&bytes) else {
        return Ok(None);
    };
    let orientation = exif::Reader::new()
        .read_from_container(&mut Cursor::new(&bytes))
        .map_or(1, |e| meta::orientation(&e));
    let (rgb, w, h) = decode_scaled(&bytes[at..], Size::Preview.long_edge())?;
    let (data, width, height) = orient(rgb, w, h, orientation);
    Ok(Some(render_rgb_all(decode::Rgb {
        data,
        width,
        height,
    })?))
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
    crate::guard(|| render_both(jpeg))
}

fn render_both(jpeg: &[u8]) -> Result<Renditions, Error> {
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

/// Builds both renditions from an already-decoded, upright image.
pub fn render_rgb_all(image: decode::Rgb) -> Result<Renditions, Error> {
    let preview = render_rgb(
        image.data,
        image.width,
        image.height,
        Size::Preview.long_edge(),
        Size::Preview.quality(),
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
    from_camera: bool,
) -> Result<Rendered, Error> {
    // Our own renditions are already upright sRGB without EXIF.
    let (orientation, adobe_rgb) = if from_camera {
        match exif::Reader::new().read_from_container(&mut Cursor::new(jpeg)) {
            Ok(e) => (
                meta::orientation(&e),
                // "Uncalibrated" is how cameras flag Adobe RGB (DCF option file).
                e.get_field(exif::Tag::ColorSpace, exif::In::PRIMARY)
                    .and_then(|f| f.value.get_uint(0))
                    == Some(0xFFFF),
            ),
            Err(_) => (1, false),
        }
    } else {
        (1, false)
    };

    let (mut rgb, dw, dh) = decode_scaled(jpeg, long_edge)?;
    if from_camera {
        match decode::jpeg_icc(jpeg) {
            Some(icc) => decode::to_srgb(&mut rgb, &icc),
            None if adobe_rgb => decode::adobe_rgb_to_srgb(&mut rgb),
            None => {}
        }
    }
    let (rgb, w, h) = orient(rgb, dw, dh, orientation);
    let long_edge = long_edge.min(w.max(h));
    render_rgb(rgb, w, h, long_edge, quality)
}

/// A camera JPEG as upright sRGB pixels, at least `long_edge` on its long
/// side where the decoder can scale.
pub(crate) fn upright(jpeg: &[u8], long_edge: u32) -> Result<decode::Rgb, Error> {
    let orientation = exif::Reader::new()
        .read_from_container(&mut Cursor::new(jpeg))
        .map_or(1, |e| meta::orientation(&e));
    let adobe_rgb = exif::Reader::new()
        .read_from_container(&mut Cursor::new(jpeg))
        .ok()
        .and_then(|e| {
            e.get_field(exif::Tag::ColorSpace, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
        })
        == Some(0xFFFF);
    let (mut rgb, dw, dh) = decode_scaled(jpeg, long_edge)?;
    match decode::jpeg_icc(jpeg) {
        Some(icc) => decode::to_srgb(&mut rgb, &icc),
        None if adobe_rgb => decode::adobe_rgb_to_srgb(&mut rgb),
        None => {}
    }
    let (data, width, height) = orient(rgb, dw, dh, orientation);
    Ok(decode::Rgb {
        data,
        width,
        height,
    })
}

/// One of our own renditions (already upright sRGB) as pixels, decoded at
/// least `long_edge` across where the decoder can scale down.
pub fn decode_rendition(jpeg: &[u8], long_edge: u32) -> Result<decode::Rgb, Error> {
    let (data, width, height) = decode_scaled(jpeg, long_edge)?;
    Ok(decode::Rgb {
        data,
        width,
        height,
    })
}

/// The square around `bbox` (fractions of the picture) `margin` times its
/// longer side, as left, top and side in whole pixels, inside the picture.
/// In whole pixels so it fits exactly: worked out in fractions, a square
/// touching the edge could end a hair past it, and its crop would fail.
fn square_around(width: u32, height: u32, bbox: [f32; 4], margin: f32) -> Option<(f64, f64, f64)> {
    if width == 0 || height == 0 || !bbox.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (w, h) = (width as f64, height as f64);
    let [x, y, bw, bh] = bbox.map(f64::from);
    let side = ((bw * w).max(bh * h) * margin as f64)
        .round()
        .clamp(1.0, w.min(h));
    let left = ((x + bw / 2.0) * w - side / 2.0)
        .round()
        .clamp(0.0, w - side);
    let top = ((y + bh / 2.0) * h - side / 2.0)
        .round()
        .clamp(0.0, h - side);
    Some((left, top, side))
}

/// Just what's inside `bbox` (fractions of the picture), scaled to fit a
/// square JPEG `out` pixels across, the rest neutral grey: a pet's picture
/// without the cat lying next to it or the bed they share.
pub fn crop_box(image: &decode::Rgb, bbox: [f32; 4], out: u32) -> Result<Vec<u8>, Error> {
    if image.width == 0 || image.height == 0 || !bbox.iter().all(|v| v.is_finite()) {
        return Err(Error::Decode("nothing to crop".into()));
    }
    let (w, h) = (image.width as f64, image.height as f64);
    let [x, y, bw, bh] = bbox.map(f64::from);
    let left = (x * w).round().clamp(0.0, w - 1.0);
    let top = (y * h).round().clamp(0.0, h - 1.0);
    let width = (bw * w).round().clamp(1.0, w - left);
    let height = (bh * h).round().clamp(1.0, h - top);
    let scale = out as f64 / width.max(height);
    let (tw, th) = (
        ((width * scale).round() as u32).clamp(1, out),
        ((height * scale).round() as u32).clamp(1, out),
    );
    let src =
        fr::images::ImageRef::new(image.width, image.height, &image.data, fr::PixelType::U8x3)
            .map_err(|e| Error::Decode(e.to_string()))?;
    let mut inside = Image::new(tw, th, fr::PixelType::U8x3);
    let options = fr::ResizeOptions::new()
        .crop(left, top, width, height)
        .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
    fr::Resizer::new()
        .resize(&src, &mut inside, &options)
        .map_err(|e| Error::Decode(e.to_string()))?;
    // ImageNet's average colour: what the models see as "nothing".
    let mut canvas = [124u8, 116, 104].repeat((out * out) as usize);
    let (dx, dy) = ((out - tw) / 2, (out - th) / 2);
    for (row, line) in inside.buffer().chunks_exact(tw as usize * 3).enumerate() {
        let at = (((dy as usize + row) * out as usize) + dx as usize) * 3;
        canvas[at..at + line.len()].copy_from_slice(line);
    }
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 88)
        .encode(
            &canvas,
            out as u16,
            out as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(jpeg)
}

/// A square JPEG, `out` pixels across, around `bbox` (fractions of the
/// picture: x, y, width, height) with `margin` times its size around it.
pub fn crop_square(
    image: &decode::Rgb,
    bbox: [f32; 4],
    margin: f32,
    out: u32,
) -> Result<Vec<u8>, Error> {
    let (left, top, side) = square_around(image.width, image.height, bbox, margin)
        .ok_or_else(|| Error::Decode("nothing to crop".into()))?;
    let src =
        fr::images::ImageRef::new(image.width, image.height, &image.data, fr::PixelType::U8x3)
            .map_err(|e| Error::Decode(e.to_string()))?;
    let mut dst = Image::new(out, out, fr::PixelType::U8x3);
    let options = fr::ResizeOptions::new()
        .crop(left, top, side, side)
        .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
    fr::Resizer::new()
        .resize(&src, &mut dst, &options)
        .map_err(|e| Error::Decode(e.to_string()))?;
    let mut jpeg = Vec::new();
    jpeg_encoder::Encoder::new(&mut jpeg, 88)
        .encode(
            dst.buffer(),
            out as u16,
            out as u16,
            jpeg_encoder::ColorType::Rgb,
        )
        .map_err(|e| Error::Decode(e.to_string()))?;
    Ok(jpeg)
}

/// Decodes a JPEG to RGB, letting the decoder skip most of the work when
/// the result only needs to cover `long_edge`. Returns pixels and size.
pub(crate) fn decode_scaled(jpeg: &[u8], long_edge: u32) -> Result<(Vec<u8>, u32, u32), Error> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
    decoder.read_info().map_err(decode_err)?;
    let info = decoder
        .info()
        .ok_or_else(|| Error::Decode("missing frame header".into()))?;
    if info.width as u64 * info.height as u64 > crate::MAX_PIXELS {
        return Err(Error::Decode(format!(
            "image claims to be {}×{}",
            info.width, info.height
        )));
    }
    let (target_w, target_h) = fit(info.width as u32, info.height as u32, long_edge);
    // The decoder picks the smallest factor that is still at least as large
    // as what we asked for.
    let (dw, dh) = decoder
        .scale(target_w as u16, target_h as u16)
        .map_err(decode_err)?;
    let pixels = decoder.decode().map_err(decode_err)?;
    let adobe = has_adobe_marker(jpeg);
    Ok((
        to_rgb(pixels, info.pixel_format, adobe),
        dw as u32,
        dh as u32,
    ))
}

/// Whether a JPEG carries Adobe's APP14 segment, as Photoshop's do. Its CMYK
/// is stored inverted (0 = full ink).
fn has_adobe_marker(jpeg: &[u8]) -> bool {
    let mut at = 2;
    while at + 4 <= jpeg.len() && jpeg[at] == 0xFF {
        let marker = jpeg[at + 1];
        // Start of scan: no more header segments.
        if marker == 0xDA {
            break;
        }
        let len = u16::from_be_bytes([jpeg[at + 2], jpeg[at + 3]]) as usize;
        if marker == 0xEE && jpeg[at + 4..].starts_with(b"Adobe") {
            return true;
        }
        at += 2 + len;
    }
    false
}

/// Fits upright sRGB pixels inside `long_edge` and encodes them as JPEG.
pub fn render_rgb(
    rgb: Vec<u8>,
    w: u32,
    h: u32,
    long_edge: u32,
    quality: u8,
) -> Result<Rendered, Error> {
    let (width, height) = fit(w, h, long_edge);
    resize_encode(rgb, (w, h), (width, height), quality, None)
}

/// Resizes upright sRGB pixels to exactly `size` and encodes them as JPEG,
/// with an EXIF block (`Exif\0\0` + TIFF) when one is given.
pub(crate) fn resize_encode(
    rgb: Vec<u8>,
    (w, h): (u32, u32),
    (width, height): (u32, u32),
    quality: u8,
    exif: Option<&[u8]>,
) -> Result<Rendered, Error> {
    let rgb = if (width, height) == (w, h) {
        rgb
    } else {
        let src = Image::from_vec_u8(w, h, rgb, fr::PixelType::U8x3)
            .map_err(|e| Error::Decode(e.to_string()))?;
        let mut dst = Image::new(width, height, fr::PixelType::U8x3);
        let options = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Lanczos3));
        fr::Resizer::new()
            .resize(&src, &mut dst, &options)
            .map_err(|e| Error::Decode(e.to_string()))?;
        dst.into_vec()
    };
    let mut jpeg = Vec::with_capacity((width * height / 4) as usize);
    let mut encoder = jpeg_encoder::Encoder::new(&mut jpeg, quality);
    encoder.set_progressive(false);
    if let Some(exif) = exif {
        encoder
            .add_app_segment(1, exif.to_vec())
            .map_err(|e| Error::Decode(e.to_string()))?;
    }
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

fn to_rgb(pixels: Vec<u8>, format: jpeg_decoder::PixelFormat, adobe: bool) -> Vec<u8> {
    use jpeg_decoder::PixelFormat as P;
    match format {
        P::RGB24 => pixels,
        P::L8 => pixels.iter().flat_map(|&l| [l, l, l]).collect(),
        // Big-endian samples: the high byte is a fine 8-bit approximation.
        P::L16 => pixels
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[hi, _]| [hi, hi, hi])
            .collect(),
        // jpeg-decoder inverts what's stored (and turns YCCK into the same
        // form). Photoshop stores CMYK inverted, so for its files (marked
        // Adobe) that gives ink amounts; others store ink, now inverted.
        P::CMYK32 => pixels
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&px| {
                let [c, m, y, k] = if adobe { px.map(|v| 255 - v) } else { px };
                let k = k as u16;
                [
                    (c as u16 * k / 255) as u8,
                    (m as u16 * k / 255) as u8,
                    (y as u16 * k / 255) as u8,
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

    /// A pet or face filling the frame, or touching its edge, crops without
    /// error: a square a hair past the edge once failed a whole photo.
    #[test]
    fn crops_near_the_edges_stay_inside_the_picture() {
        let sizes = [
            (4032u32, 3024u32),
            (3024, 4032),
            (999, 751),
            (1179, 2556),
            (403, 302),
            (7, 5),
        ];
        for (w, h) in sizes {
            for i in 0..400 {
                let size = 0.02 + 0.98 * i as f32 / 399.0;
                for bbox in [
                    [1.0 - size, 1.0 - size * 0.7, size, size * 0.7],
                    [0.0, 0.0, size, size],
                    [1.0 - size * 0.3, 0.2, size * 0.3 + 0.01, size],
                    [0.3333, 0.6667, size, size],
                    [-0.1, 1.05, size, size],
                ] {
                    let (left, top, side) = square_around(w, h, bbox, 1.1).unwrap();
                    let whole = |v: f64| v.fract() == 0.0;
                    assert!(
                        whole(left) && whole(top) && whole(side) && side >= 1.0,
                        "{w}x{h} {bbox:?}"
                    );
                    assert!(left >= 0.0 && left + side <= w as f64, "{w}x{h} {bbox:?}");
                    assert!(top >= 0.0 && top + side <= h as f64, "{w}x{h} {bbox:?}");
                }
            }
        }
        // And real crops, against each edge.
        let image = decode::Rgb {
            data: vec![128; 403 * 302 * 3],
            width: 403,
            height: 302,
        };
        for bbox in [
            [0.98, 0.2, 0.03, 0.05],
            [0.0, 0.9, 0.5, 0.1],
            [0.5, 0.5, 0.5, 0.5],
            [0.0, 0.0, 1.0, 1.0],
        ] {
            crop_square(&image, bbox, 1.1, 64).unwrap();
        }
        assert!(crop_square(&image, [f32::NAN, 0.0, 0.5, 0.5], 1.1, 8).is_err());
    }

    /// A file whose own picture is damaged still gets a thumbnail, from the
    /// preview embedded in it.
    #[test]
    fn damaged_photos_fall_back_to_their_embedded_preview() {
        let dir = tempfile::tempdir().unwrap();
        let preview = crate::test_jpeg(320, 240, None);
        // A JPEG start that leads nowhere, the preview, then junk.
        let mut bytes = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x04, 0xFF, 0xFF];
        bytes.extend_from_slice(&preview);
        bytes.extend_from_slice(&[0x55; 4096]);
        let path = dir.path().join("damaged.jpg");
        fs::write(&path, &bytes).unwrap();
        let cache = Cache::new(dir.path().join("cache")).unwrap();
        let src = Source {
            path: &path,
            kind: Kind::Jpeg,
            size: bytes.len() as u64,
            mtime: 1,
        };
        let thumb = cache.ensure(&src, Size::Thumb).unwrap();
        let image = decode_rendition(&fs::read(thumb).unwrap(), 4096).unwrap();
        assert_eq!((image.width, image.height), (320, 240));
        assert!(cache.get(&src, Size::Preview).is_some());

        // Nothing embedded: still an error.
        fs::write(&path, [0xFF, 0xD8, 0xFF, 0xC0, 0, 4, 1, 2]).unwrap();
        let src = Source { size: 8, ..src };
        assert!(cache.ensure(&src, Size::Thumb).is_err());
    }

    /// Photoshop's CMYK JPEGs (stored inverted, marked Adobe), plain and
    /// YCCK-coded, come out in the right colours.
    #[test]
    fn photoshop_cmyk_jpegs_keep_their_colours() {
        // Ink amounts: red (M+Y), cyan, white (no ink), black (K).
        let inks: [[u8; 4]; 4] = [[0, 255, 255, 0], [255, 0, 0, 0], [0; 4], [0, 0, 0, 255]];
        let expect = [[255, 0, 0], [0, 255, 255], [255, 255, 255], [0, 0, 0]];
        for coding in [
            jpeg_encoder::ColorType::Cmyk,
            jpeg_encoder::ColorType::CmykAsYcck,
        ] {
            // 4 blocks of 16×16, one per ink. The encoder stores them
            // inverted and marks the file Adobe, as Photoshop does.
            let (w, h) = (64u16, 16u16);
            let stored: Vec<u8> = (0..h as usize * w as usize)
                .flat_map(|i| inks[i % w as usize / 16])
                .collect();
            let mut jpeg = Vec::new();
            jpeg_encoder::Encoder::new(&mut jpeg, 95)
                .encode(&stored, w, h, coding)
                .unwrap();
            assert!(has_adobe_marker(&jpeg));
            let (rgb, dw, _) = decode_scaled(&jpeg, 64).unwrap();
            for (n, want) in expect.iter().enumerate() {
                let at = (8 * dw as usize + n * 16 + 8) * 3;
                let got = &rgb[at..at + 3];
                assert!(
                    got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) < 12),
                    "{coding:?} ink {n}: {got:?}, want {want:?}"
                );
            }
        }
    }

    /// A JPEG whose header claims an enormous size is refused before any big
    /// buffer is allocated.
    #[test]
    fn refuses_absurd_sizes() {
        let mut jpeg = crate::test_jpeg(40, 30, None);
        let sof = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        jpeg[sof + 5..sof + 7].copy_from_slice(&60000u16.to_be_bytes());
        jpeg[sof + 7..sof + 9].copy_from_slice(&60000u16.to_be_bytes());
        let err = render(&jpeg, 512, 80, true).err().expect("refused");
        assert!(err.to_string().contains("60000"), "{err}");
    }

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
