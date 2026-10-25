//! Decoding for edited files (Photoshop PSD/PSB, TIFF) and colour management.
//!
//! Edits are often saved in a wide-gamut space such as Adobe RGB. Shown as
//! if they were sRGB they look dull, which would make a before/after
//! comparison misleading, so every rendition is converted to sRGB using the
//! file's embedded ICC profile.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::sync::Mutex;

use crate::{Error, Kind};

/// Packed 8-bit RGB pixels.
pub struct Rgb {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Full-size decodes of 16-bit edits can take hundreds of MB; one at a time.
static BIG_DECODE: Mutex<()> = Mutex::new(());

fn err(msg: impl Into<String>) -> Error {
    Error::Decode(msg.into())
}

/// Decodes a PSD/PSB or TIFF to upright sRGB pixels.
pub fn image(path: &Path, kind: Kind) -> Result<Rgb, Error> {
    let _one_at_a_time = BIG_DECODE.lock().unwrap_or_else(|e| e.into_inner());
    match kind {
        Kind::Psd => psd(path),
        Kind::Tiff => tiff(path),
        other => Err(Error::Unsupported(other)),
    }
}

/// Width, height and EXIF orientation, reading only the file header.
pub fn dimensions(path: &Path, kind: Kind) -> Result<(u32, u32, u16), Error> {
    match kind {
        Kind::Psd => {
            let mut header = [0u8; 26];
            File::open(path)?.read_exact(&mut header)?;
            let h = psd_header(&header)?;
            Ok((h.width, h.height, 1))
        }
        Kind::Tiff => {
            let mut d =
                tiff::decoder::Decoder::new(BufReader::new(File::open(path)?)).map_err(tiff_err)?;
            let (w, h) = d.dimensions().map_err(tiff_err)?;
            let o = d
                .find_tag_unsigned::<u16>(tiff::tags::Tag::Orientation)
                .ok()
                .flatten()
                .unwrap_or(1);
            Ok(if crate::meta::swaps_axes(o) {
                (h, w, o)
            } else {
                (w, h, o)
            })
        }
        other => Err(Error::Unsupported(other)),
    }
}

// ---- colour management --------------------------------------------------------

/// Converts packed RGB from the space described by `icc` to sRGB in place.
/// Profiles that are already sRGB (or unreadable) leave the pixels alone.
pub fn to_srgb(rgb: &mut [u8], icc: &[u8]) {
    let Some(profile) = qcms::Profile::new_from_slice(icc, false) else {
        return;
    };
    convert(rgb, &profile);
}

/// Adobe RGB (1998), for camera JPEGs that say so without embedding a profile.
pub fn adobe_rgb_to_srgb(rgb: &mut [u8]) {
    let xy = |x, y| qcms::CIE_xyY { x, y, Y: 1.0 };
    let primaries = qcms::CIE_xyYTRIPLE {
        red: xy(0.64, 0.33),
        green: xy(0.21, 0.71),
        blue: xy(0.15, 0.06),
    };
    let gamma = 563.0 / 256.0;
    if let Some(profile) =
        qcms::Profile::new_rgb_with_gamma_set(xy(0.3127, 0.3290), primaries, gamma, gamma, gamma)
    {
        convert(rgb, &profile);
    }
}

fn convert(rgb: &mut [u8], from: &qcms::Profile) {
    if from.is_sRGB() {
        return;
    }
    let mut srgb = qcms::Profile::new_sRGB();
    srgb.precache_output_transform();
    if let Some(t) =
        qcms::Transform::new(from, &srgb, qcms::DataType::RGB8, qcms::Intent::Perceptual)
    {
        t.apply(rgb);
    }
}

/// The ICC profile embedded in a JPEG (APP2 `ICC_PROFILE` chunks), if any.
pub fn jpeg_icc(jpeg: &[u8]) -> Option<Vec<u8>> {
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    let mut i = 2;
    while i + 4 <= jpeg.len() && jpeg[i] == 0xFF {
        let marker = jpeg[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let len = u16::from_be_bytes([jpeg[i + 2], jpeg[i + 3]]) as usize;
        let body = jpeg.get(i + 4..i + 2 + len)?;
        if marker == 0xE2 && body.len() > 14 && body.starts_with(b"ICC_PROFILE\0") {
            chunks.push((body[12], &body[14..]));
        }
        i += 2 + len;
    }
    if chunks.is_empty() {
        return None;
    }
    chunks.sort_by_key(|c| c.0);
    Some(
        chunks
            .into_iter()
            .flat_map(|c| c.1.iter().copied())
            .collect(),
    )
}

// ---- TIFF -----------------------------------------------------------------------

fn tiff_err(e: tiff::TiffError) -> Error {
    err(format!("TIFF: {e}"))
}

fn tiff(path: &Path) -> Result<Rgb, Error> {
    use tiff::ColorType as C;
    use tiff::decoder::{Decoder, DecodingResult as D};

    let mut d = Decoder::new(BufReader::new(File::open(path)?)).map_err(tiff_err)?;
    let (w, h) = d.dimensions().map_err(tiff_err)?;
    let orientation = d
        .find_tag_unsigned::<u16>(tiff::tags::Tag::Orientation)
        .ok()
        .flatten()
        .unwrap_or(1);
    let icc = d.get_tag_u8_vec(tiff::tags::Tag::IccProfile).ok();
    let color = d.colortype().map_err(tiff_err)?;
    // Samples as 8-bit: 16-bit keeps its high byte.
    let samples: Vec<u8> = match d.read_image().map_err(tiff_err)? {
        D::U8(v) => v,
        D::U16(v) => v.into_iter().map(|s| (s >> 8) as u8).collect(),
        _ => return Err(err("floating-point TIFFs aren't supported")),
    };
    let (channels, cmyk) = match color {
        C::RGB(_) => (3, false),
        C::RGBA(_) => (4, false),
        C::Gray(_) => (1, false),
        C::GrayA(_) => (2, false),
        C::CMYK(_) => (4, true),
        other => return Err(err(format!("TIFF colour type {other:?} isn't supported"))),
    };
    let mut data = to_rgb(&samples, channels, cmyk);
    if let Some(icc) = icc.filter(|_| !cmyk) {
        to_srgb(&mut data, &icc);
    }
    let (data, width, height) = crate::thumbs::orient(data, w, h, orientation);
    Ok(Rgb {
        data,
        width,
        height,
    })
}

/// Interleaved samples (1–4 per pixel) → packed RGB.
fn to_rgb(samples: &[u8], channels: usize, cmyk: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() / channels * 3);
    for px in samples.chunks_exact(channels) {
        match (channels, cmyk) {
            (4, true) => {
                let k = 255 - px[3] as u16;
                out.extend([0, 1, 2].map(|c| ((255 - px[c] as u16) * k / 255) as u8));
            }
            (1 | 2, _) => out.extend([px[0]; 3]),
            _ => out.extend_from_slice(&px[..3]),
        }
    }
    out
}

// ---- Photoshop ------------------------------------------------------------------

struct PsdHeader {
    /// 1 = PSD, 2 = PSB (large document).
    version: u16,
    channels: u16,
    height: u32,
    width: u32,
    depth: u16,
    mode: u16,
}

fn psd_header(b: &[u8; 26]) -> Result<PsdHeader, Error> {
    if &b[..4] != b"8BPS" {
        return Err(err("not a Photoshop file"));
    }
    let u16_at = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
    let u32_at = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    let h = PsdHeader {
        version: u16_at(4),
        channels: u16_at(12),
        height: u32_at(14),
        width: u32_at(18),
        depth: u16_at(22),
        mode: u16_at(24),
    };
    if !(1..=2).contains(&h.version) || h.width == 0 || h.height == 0 {
        return Err(err("damaged Photoshop header"));
    }
    Ok(h)
}

const RES_ICC: u16 = 1039;
const RES_THUMBNAIL: u16 = 1036;

/// Composites bigger than this aren't decoded; the full image would need
/// several GB of memory just to make a 2560px preview.
const PSD_MAX_PIXELS: u64 = 120_000_000;

/// Reads the flattened composite Photoshop stores after the layers ("Maximize
/// compatibility"), skipping the layer data without reading it.
fn psd(path: &Path) -> Result<Rgb, Error> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut f = BufReader::with_capacity(1 << 20, file);
    let mut header = [0u8; 26];
    f.read_exact(&mut header)?;
    let h = psd_header(&header)?;
    let damaged = || err("damaged Photoshop file");
    let wide = h.version == 2;

    let skip_section = |f: &mut BufReader<File>, wide: bool| -> Result<(), Error> {
        let len = if wide {
            read_u64(f)?
        } else {
            read_u32(f)? as u64
        };
        f.seek_relative(len as i64)?;
        Ok(())
    };
    skip_section(&mut f, false)?; // colour mode data

    let res_len = read_u32(&mut f)? as usize;
    if res_len as u64 > file_len {
        return Err(damaged());
    }
    let mut resources = vec![0u8; res_len];
    f.read_exact(&mut resources)?;
    let icc = resource(&resources, RES_ICC).map(<[u8]>::to_vec);
    let thumb = resource(&resources, RES_THUMBNAIL)
        .and_then(|r| r.get(28..))
        .map(<[u8]>::to_vec);

    skip_section(&mut f, wide)?; // layers and masks

    let (planes, cmyk) = match h.mode {
        3 => (3usize, false),
        1 => (1, false),
        4 => (4, true),
        m => return Err(err(format!("Photoshop colour mode {m} isn't supported"))),
    };
    if h.depth != 8 && h.depth != 16 {
        return Err(err(format!(
            "{}-bit Photoshop files aren't supported",
            h.depth
        )));
    }
    let planes = planes.min(h.channels as usize);
    if h.width as u64 * h.height as u64 > PSD_MAX_PIXELS {
        return thumb
            .and_then(thumbnail_rgb)
            .ok_or_else(|| err("too large to preview, and it has no thumbnail"));
    }
    let (w, rows) = (h.width as usize, h.height as usize);
    let bps = h.depth as usize / 8;
    let row_bytes = w * bps;
    let compression = read_u16(&mut f)?;

    // Planar data, one plane per channel, rows top to bottom.
    let mut plane_data = vec![0u8; planes * rows * row_bytes];
    match compression {
        0 => f.read_exact(&mut plane_data)?,
        1 => {
            let total = h.channels as usize * rows;
            let mut counts = Vec::with_capacity(total);
            for _ in 0..total {
                counts.push(if wide {
                    read_u32(&mut f)? as usize
                } else {
                    read_u16(&mut f)? as usize
                });
            }
            // Only read the channels we need; extra alpha channels follow.
            let needed: usize = counts[..planes * rows].iter().sum();
            if needed as u64 > file_len {
                return Err(damaged());
            }
            let mut packed = vec![0u8; needed];
            f.read_exact(&mut packed)?;
            let mut src = 0;
            for (row, &n) in counts[..planes * rows].iter().enumerate() {
                let dst = &mut plane_data[row * row_bytes..(row + 1) * row_bytes];
                unpack_bits(
                    packed
                        .get(src..src + n)
                        .ok_or_else(|| err("truncated Photoshop data"))?,
                    dst,
                );
                src += n;
            }
        }
        2 | 3 => {
            let mut zipped = Vec::new();
            f.read_to_end(&mut zipped)?;
            let mut raw = Vec::new();
            flate2::read::ZlibDecoder::new(&zipped[..])
                .take(plane_data.len() as u64)
                .read_to_end(&mut raw)?;
            let take = plane_data.len().min(raw.len());
            plane_data[..take].copy_from_slice(&raw[..take]);
            if compression == 3 {
                undo_prediction(&mut plane_data, row_bytes, bps);
            }
        }
        c => return Err(err(format!("Photoshop compression {c} isn't supported"))),
    }

    // Interleave planes into 8-bit pixels (16-bit: big-endian high byte).
    let plane = rows * row_bytes;
    let mut samples = vec![0u8; w * rows * planes];
    for p in 0..planes {
        for i in 0..w * rows {
            samples[i * planes + p] = plane_data[p * plane + i * bps];
        }
    }
    // Photoshop CMYK is stored inverted (0 = full ink).
    if cmyk {
        samples.iter_mut().for_each(|s| *s = 255 - *s);
    }
    let mut data = to_rgb(&samples, planes, cmyk);

    // Without "Maximize compatibility" the composite is blank; the small
    // embedded thumbnail is better than a white rectangle.
    if data.iter().all(|&b| b == 255)
        && let Some(small) = thumb.and_then(thumbnail_rgb)
    {
        return Ok(small);
    }
    if let Some(icc) = icc.filter(|_| !cmyk) {
        to_srgb(&mut data, &icc);
    }
    Ok(Rgb {
        data,
        width: h.width,
        height: h.height,
    })
}

/// Photoshop's small embedded JPEG thumbnail.
fn thumbnail_rgb(jpeg: Vec<u8>) -> Option<Rgb> {
    let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg));
    let pixels = d.decode().ok()?;
    let info = d.info()?;
    (info.pixel_format == jpeg_decoder::PixelFormat::RGB24).then_some(Rgb {
        data: pixels,
        width: info.width as u32,
        height: info.height as u32,
    })
}

/// The data of image resource `id` ("8BIM" blocks with padded Pascal names).
fn resource(mut res: &[u8], id: u16) -> Option<&[u8]> {
    while res.len() >= 12 && &res[..4] == b"8BIM" {
        let rid = u16::from_be_bytes([res[4], res[5]]);
        let name_len = res[6] as usize;
        let name_total = (1 + name_len + 1) & !1;
        let at = 6 + name_total;
        let size = u32::from_be_bytes(res.get(at..at + 4)?.try_into().ok()?) as usize;
        let data = res.get(at + 4..at + 4 + size)?;
        if rid == id {
            return Some(data);
        }
        res = res.get(at + 4 + ((size + 1) & !1)..)?;
    }
    None
}

/// PackBits run-length decoding into a fixed-size row.
fn unpack_bits(src: &[u8], dst: &mut [u8]) {
    let (mut i, mut o) = (0, 0);
    while i < src.len() && o < dst.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = (n as usize + 1).min(dst.len() - o).min(src.len() - i);
            dst[o..o + len].copy_from_slice(&src[i..i + len]);
            i += len;
            o += len;
        } else if n != -128 {
            let len = ((1 - n as isize) as usize).min(dst.len() - o);
            if let Some(&b) = src.get(i) {
                dst[o..o + len].fill(b);
            }
            i += 1;
            o += len;
        }
    }
}

/// ZIP-with-prediction stores each sample as the difference to the previous.
fn undo_prediction(data: &mut [u8], row_bytes: usize, bps: usize) {
    for row in data.chunks_mut(row_bytes) {
        if bps == 2 {
            let mut prev = 0u16;
            for s in row.as_chunks_mut::<2>().0 {
                prev = prev.wrapping_add(u16::from_be_bytes(*s));
                *s = prev.to_be_bytes();
            }
        } else {
            for i in 1..row.len() {
                row[i] = row[i].wrapping_add(row[i - 1]);
            }
        }
    }
}

fn read_u16(r: &mut impl Read) -> std::io::Result<u16> {
    let mut b = [0; 2];
    r.read_exact(&mut b)?;
    Ok(u16::from_be_bytes(b))
}

fn read_u32(r: &mut impl Read) -> std::io::Result<u32> {
    let mut b = [0; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_be_bytes(b))
}

fn read_u64(r: &mut impl Read) -> std::io::Result<u64> {
    let mut b = [0; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_be_bytes(b))
}

/// Writes a minimal flattened PSD (8- or 16-bit RGB, RLE or raw). Test helper.
#[doc(hidden)]
pub fn write_test_psd(
    width: u32,
    height: u32,
    rgb: &[u8],
    depth: u16,
    rle: bool,
    layers: &[u8],
) -> Vec<u8> {
    let mut out = b"8BPS".to_vec();
    out.extend(1u16.to_be_bytes());
    out.extend([0u8; 6]);
    out.extend(3u16.to_be_bytes());
    out.extend(height.to_be_bytes());
    out.extend(width.to_be_bytes());
    out.extend(depth.to_be_bytes());
    out.extend(3u16.to_be_bytes());
    out.extend(0u32.to_be_bytes()); // colour mode data
    out.extend(0u32.to_be_bytes()); // image resources
    out.extend((layers.len() as u32).to_be_bytes());
    out.extend(layers);
    out.extend((if rle { 1u16 } else { 0 }).to_be_bytes());
    let bps = depth as usize / 8;
    let mut rows = Vec::new();
    for c in 0..3 {
        for y in 0..height as usize {
            let mut row = Vec::new();
            for x in 0..width as usize {
                let v = rgb[(y * width as usize + x) * 3 + c];
                row.push(v);
                if bps == 2 {
                    row.push(v);
                }
            }
            rows.push(row);
        }
    }
    if rle {
        // Literal runs only: valid PackBits, simple to write.
        let packed: Vec<Vec<u8>> = rows
            .iter()
            .map(|row| {
                row.chunks(128)
                    .flat_map(|c| std::iter::once((c.len() - 1) as u8).chain(c.iter().copied()))
                    .collect()
            })
            .collect();
        for p in &packed {
            out.extend((p.len() as u16).to_be_bytes());
        }
        packed.into_iter().for_each(|p| out.extend(p));
    } else {
        rows.into_iter().for_each(|r| out.extend(r));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<u8> {
        (0..w * h)
            .flat_map(|i| [(i % w * 255 / w) as u8, (i / w * 255 / h) as u8, 77])
            .collect()
    }

    /// A Photoshop file too big to decode sensibly isn't: it would need
    /// gigabytes just to make a preview.
    #[test]
    fn oversized_photoshop_files_are_not_decoded() {
        let mut psd = write_test_psd(4, 4, &gradient(4, 4), 8, false, &[]);
        psd[14..18].copy_from_slice(&30_000u32.to_be_bytes());
        psd[18..22].copy_from_slice(&30_000u32.to_be_bytes());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.psd");
        std::fs::write(&path, &psd).unwrap();
        let err = image(&path, Kind::Psd).err().expect("refused");
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[test]
    fn packbits() {
        let mut out = [0u8; 8];
        // 3 literal bytes, then 'z' repeated 5 times.
        unpack_bits(&[2, b'a', b'b', b'c', 0xFC, b'z'], &mut out);
        assert_eq!(&out, b"abczzzzz");
    }

    #[test]
    fn reads_photoshop_composites() {
        let dir = tempfile::tempdir().unwrap();
        let rgb = gradient(300, 7);
        for (depth, rle) in [(8, true), (8, false), (16, true)] {
            let path = dir.path().join(format!("edit-{depth}-{rle}.psd"));
            // Some fake layer data that must be skipped, not decoded.
            std::fs::write(&path, write_test_psd(300, 7, &rgb, depth, rle, &[9; 1000])).unwrap();
            assert_eq!(dimensions(&path, Kind::Psd).unwrap(), (300, 7, 1));
            let img = image(&path, Kind::Psd).unwrap();
            assert_eq!((img.width, img.height), (300, 7));
            assert_eq!(img.data, rgb, "{depth}-bit rle={rle}");
        }
    }

    #[test]
    fn reads_tiffs_8_and_16_bit() {
        use tiff::encoder::{TiffEncoder, colortype};
        let dir = tempfile::tempdir().unwrap();
        let rgb = gradient(40, 30);

        let p8 = dir.path().join("edit8.tif");
        TiffEncoder::new(File::create(&p8).unwrap())
            .unwrap()
            .write_image::<colortype::RGB8>(40, 30, &rgb)
            .unwrap();
        assert_eq!(dimensions(&p8, Kind::Tiff).unwrap(), (40, 30, 1));
        assert_eq!(image(&p8, Kind::Tiff).unwrap().data, rgb);

        let p16 = dir.path().join("edit16.tif");
        let wide: Vec<u16> = rgb.iter().map(|&v| v as u16 * 257).collect();
        TiffEncoder::new(File::create(&p16).unwrap())
            .unwrap()
            .write_image::<colortype::RGB16>(40, 30, &wide)
            .unwrap();
        assert_eq!(image(&p16, Kind::Tiff).unwrap().data, rgb);
    }

    #[test]
    fn finds_jpeg_icc_profiles() {
        // SOI, APP2 with an ICC chunk, then SOS.
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE2];
        let body = [b"ICC_PROFILE\0".as_slice(), &[1, 1], b"profile-bytes"].concat();
        jpeg.extend(((body.len() + 2) as u16).to_be_bytes());
        jpeg.extend(&body);
        jpeg.extend([0xFF, 0xDA]);
        assert_eq!(jpeg_icc(&jpeg).as_deref(), Some(&b"profile-bytes"[..]));
        assert_eq!(jpeg_icc(&[0xFF, 0xD8, 0xFF, 0xDA]), None);
    }

    #[test]
    fn adobe_rgb_conversion_moves_saturated_colours_not_greys() {
        let mut px = vec![60, 200, 60, 128, 128, 128];
        adobe_rgb_to_srgb(&mut px);
        // This Adobe RGB green lies outside sRGB: its red component drops to ~0.
        assert!(px[0] < 30, "{px:?}");
        // Neutral grey stays neutral.
        assert!(
            (px[3] as i16 - px[4] as i16).abs() <= 1 && (px[4] as i16 - px[5] as i16).abs() <= 1
        );
        let mut srgb = vec![10, 20, 30];
        to_srgb(&mut srgb, b"not a profile");
        assert_eq!(srgb, vec![10, 20, 30]);
    }
}
