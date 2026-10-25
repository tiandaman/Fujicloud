//! Movies: iPhone `.MOV` (including the moving half of a Live Photo) and
//! `.MP4`. Metadata comes from the `moov` box, which is all that's read;
//! the poster frame for thumbnails comes from Windows, which has the codecs.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::Error;
use crate::meta::PhotoMeta;

/// Seconds from 1904-01-01 (QuickTime's epoch) to 1970-01-01.
const QT_EPOCH: i64 = 2_082_844_800;
/// A `moov` box bigger than this isn't a camera's.
const MAX_MOOV: u64 = 64 << 20;

fn bad(msg: &str) -> Error {
    Error::Decode(format!("movie: {msg}"))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// The boxes directly inside `data`: `(type, payload)`.
fn children(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 8 <= data.len() {
        let size = be32(data, at).unwrap_or(0) as usize;
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap();
        let (header, size) = match size {
            0 => (8, data.len() - at),
            1 => match be64(data, at + 8) {
                Some(s) => (16, s as usize),
                None => break,
            },
            s => (8, s),
        };
        if size < header || at + size > data.len() {
            break;
        }
        out.push((kind, &data[at + header..at + size]));
        at += size;
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(data)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, d)| d)
}

/// Reads just the `moov` box, wherever it is in the file.
fn read_moov(path: &Path) -> Result<Vec<u8>, Error> {
    let mut file = BufReader::with_capacity(8 * 1024, File::open(path)?);
    let len = file.get_ref().metadata()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        let mut head = [0u8; 16];
        file.seek(SeekFrom::Start(at))?;
        file.read_exact(&mut head[..8])?;
        let (header, size) = match be32(&head, 0).unwrap() {
            0 => (8, len - at),
            1 => {
                file.read_exact(&mut head[8..])?;
                (16, be64(&head, 8).unwrap())
            }
            s => (8, s as u64),
        };
        if size < header || at + size > len {
            break;
        }
        if &head[4..8] == b"moov" {
            if size - header > MAX_MOOV {
                return Err(bad("index too large"));
            }
            let mut moov = vec![0; (size - header) as usize];
            file.read_exact(&mut moov)?;
            return Ok(moov);
        }
        at += size;
    }
    Err(bad("no index (moov) found"))
}

/// `(creation time, duration in seconds)` from `mvhd`.
fn movie_header(mvhd: &[u8]) -> Option<(i64, f64)> {
    let (created, scale, duration) = if mvhd.first()? == &1 {
        (
            be64(mvhd, 4)? as i64,
            be32(mvhd, 20)?,
            be64(mvhd, 24)? as f64,
        )
    } else {
        (
            be32(mvhd, 4)? as i64,
            be32(mvhd, 12)?,
            be32(mvhd, 16)? as f64,
        )
    };
    (scale > 0).then(|| (created, duration / scale as f64))
}

/// The video track's display size, turned by its matrix.
fn video_size(moov: &[u8]) -> Option<(u32, u32)> {
    children(moov)
        .into_iter()
        .filter(|(k, _)| k == b"trak")
        .find_map(|(_, trak)| {
            let tkhd = child(trak, b"tkhd")?;
            // The matrix (a, b, u, c, d, v, x, y, w), then width and height.
            let matrix = if tkhd.first()? == &1 { 52 } else { 40 };
            let (a, d) = (be32(tkhd, matrix)?, be32(tkhd, matrix + 16)?);
            let w = be32(tkhd, matrix + 36)? >> 16;
            let h = be32(tkhd, matrix + 40)? >> 16;
            (w > 0 && h > 0).then_some(if a == 0 && d == 0 { (h, w) } else { (w, h) })
        })
}

/// QuickTime `meta` items: `keys` names, `ilst` values (UTF-8 only).
fn apple_items(moov: &[u8]) -> Vec<(String, String)> {
    let Some(meta) = child(moov, b"meta") else {
        return Vec::new();
    };
    // ISO `meta` is a full box (4 bytes of version and flags); QuickTime's isn't.
    let meta = if meta.get(4..8) == Some(b"hdlr") {
        meta
    } else {
        meta.get(4..).unwrap_or_default()
    };
    let (Some(keys), Some(ilst)) = (child(meta, b"keys"), child(meta, b"ilst")) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut at = 8;
    for _ in 0..be32(keys, 4).unwrap_or(0).min(256) {
        let Some(size) = be32(keys, at).map(|s| s as usize) else {
            break;
        };
        let Some(name) = keys.get(at + 8..at + size) else {
            break;
        };
        names.push(String::from_utf8_lossy(name).into_owned());
        at += size;
    }
    children(ilst)
        .into_iter()
        .filter_map(|(index, item)| {
            let name = names.get((u32::from_be_bytes(index) as usize).checked_sub(1)?)?;
            let data = child(item, b"data")?;
            // type 1 = UTF-8; then a 4-byte locale
            (be32(data, 0)? == 1).then(|| {
                let value = String::from_utf8_lossy(data.get(8..)?).into_owned();
                Some((name.clone(), value))
            })?
        })
        .collect()
}

/// `udta` `©xyz` (Android and many cameras): an ISO 6709 position.
fn udta_location(moov: &[u8]) -> Option<String> {
    let xyz = child(child(moov, b"udta")?, b"\xa9xyz")?;
    let len = u16::from_be_bytes(xyz.get(..2)?.try_into().ok()?) as usize;
    Some(String::from_utf8_lossy(xyz.get(4..4 + len)?).into_owned())
}

/// `+37.3317-122.0302+012.345/` → latitude, longitude, altitude.
fn iso6709(s: &str) -> Option<(f64, f64, Option<f64>)> {
    let s = s.trim().trim_end_matches('/');
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices().skip(1) {
        if c == '+' || c == '-' {
            parts.push(&s[start..i]);
            start = i;
        }
    }
    parts.push(&s[start..]);
    let num = |p: &&str| p.parse::<f64>().ok();
    let lat = num(parts.first()?)?;
    let lon = num(parts.get(1)?)?;
    ((-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon))
        .then(|| (lat, lon, parts.get(2).and_then(num)))
}

/// "2026-10-21T14:03:22+0200" → naive local seconds, like EXIF dates.
fn local_date(s: &str) -> Option<i64> {
    let naive = chrono::NaiveDateTime::parse_from_str(s.get(..19)?, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(naive.and_utc().timestamp())
}

pub fn read(path: &Path) -> Result<PhotoMeta, Error> {
    let moov = read_moov(path)?;
    let items = apple_items(&moov);
    let item = |key: &str| {
        items
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    let header = child(&moov, b"mvhd").and_then(movie_header);
    let (width, height) = video_size(&moov).unwrap_or((0, 0));
    let taken_at = item("com.apple.quicktime.creationdate")
        .and_then(|d| local_date(&d))
        .or_else(|| {
            header
                .map(|(created, _)| created - QT_EPOCH)
                .filter(|t| *t > 0)
        });
    let place = item("com.apple.quicktime.location.ISO6709")
        .or_else(|| udta_location(&moov))
        .and_then(|s| iso6709(&s));
    Ok(PhotoMeta {
        taken_at,
        width,
        height,
        orientation: 1,
        camera: crate::meta::camera_name(
            item("com.apple.quicktime.make"),
            item("com.apple.quicktime.model"),
        ),
        lat: place.map(|p| p.0),
        lon: place.map(|p| p.1),
        altitude: place.and_then(|p| p.2),
        duration: header.map(|(_, d)| d).filter(|d| *d > 0.0),
        ..Default::default()
    })
}

/// A frame of the movie as upright RGB, at least `long_edge` on its long
/// side where Windows can manage it. Uses the same thumbnail handlers as
/// File Explorer, so any format with a codec installed works (iPhone HEVC
/// needs Microsoft's free HEVC Video Extensions).
#[cfg(windows)]
pub fn poster(path: &Path, long_edge: u32) -> Result<crate::decode::Rgb, Error> {
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC,
        GetDIBits, GetObjectW, HGDIOBJ, ReleaseDC,
    };
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
        SIIGBF_THUMBNAILONLY,
    };
    use windows::core::HSTRING;

    let shell = |e: windows::core::Error| bad(&format!("Windows can't make a preview ({e})"));
    // SAFETY: plain Win32 calls; every handle is released before returning.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None)
                    .map_err(shell)?;
            let edge = long_edge.min(2560) as i32;
            let bitmap = factory
                .GetImage(
                    SIZE { cx: edge, cy: edge },
                    SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK,
                )
                .map_err(shell)?;
            let object = HGDIOBJ(bitmap.0);
            let mut bm = BITMAP::default();
            let got = GetObjectW(
                object,
                size_of::<BITMAP>() as i32,
                Some((&raw mut bm).cast()),
            );
            let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
            let mut bgra = vec![0u8; (w.max(0) * h * 4) as usize];
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = GetDC(None);
            let lines = if got > 0 && w > 0 && h > 0 {
                GetDIBits(
                    dc,
                    bitmap,
                    0,
                    h as u32,
                    Some(bgra.as_mut_ptr().cast()),
                    &mut info,
                    DIB_RGB_COLORS,
                )
            } else {
                0
            };
            ReleaseDC(None, dc);
            let _ = DeleteObject(object);
            if lines != h || w <= 0 {
                return Err(bad("Windows gave an empty preview"));
            }
            let data = bgra
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[2], p[1], p[0]])
                .collect();
            Ok(crate::decode::Rgb {
                data,
                width: w as u32,
                height: h as u32,
            })
        })();
        if com {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
pub fn poster(_: &Path, _: u32) -> Result<crate::decode::Rgb, Error> {
    Err(Error::Unsupported(crate::Kind::Video))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn boxed(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend(kind);
        out.extend(payload);
        out
    }

    /// An iPhone-like portrait clip: `ftyp`, `mdat`, then `moov` at the end.
    pub(crate) fn iphone_clip() -> Vec<u8> {
        let mut mvhd = vec![0u8; 100];
        mvhd[4..8].copy_from_slice(&((1_792_000_000i64 + QT_EPOCH) as u32).to_be_bytes());
        mvhd[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&(600u32 * 3 / 2).to_be_bytes()); // 1.5 s

        let mut tkhd = vec![0u8; 84];
        // Rotated 90°: a = 0, b = 1, c = -1, d = 0.
        tkhd[44..48].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        tkhd[52..56].copy_from_slice(&0xFFFF_0000u32.to_be_bytes());
        tkhd[76..80].copy_from_slice(&(1920u32 << 16).to_be_bytes());
        tkhd[80..84].copy_from_slice(&(1080u32 << 16).to_be_bytes());
        let sound = boxed(b"tkhd", &[0u8; 84]);

        let keys_list = [
            "com.apple.quicktime.make",
            "com.apple.quicktime.model",
            "com.apple.quicktime.creationdate",
            "com.apple.quicktime.location.ISO6709",
        ];
        let values = [
            "Apple",
            "iPhone 16 Pro",
            "2026-10-21T14:03:22+0200",
            "+64.1466-021.9426+012.345/",
        ];
        let mut keys = vec![0, 0, 0, 0];
        keys.extend((keys_list.len() as u32).to_be_bytes());
        for k in keys_list {
            keys.extend(((k.len() + 8) as u32).to_be_bytes());
            keys.extend(b"mdta");
            keys.extend(k.as_bytes());
        }
        let mut ilst = Vec::new();
        for (i, v) in values.iter().enumerate() {
            let mut data = 1u32.to_be_bytes().to_vec();
            data.extend([0; 4]);
            data.extend(v.as_bytes());
            ilst.extend(boxed(
                &((i + 1) as u32).to_be_bytes(),
                &boxed(b"data", &data),
            ));
        }
        let mut meta = boxed(b"hdlr", &[0; 24]);
        meta.extend(boxed(b"keys", &keys));
        meta.extend(boxed(b"ilst", &ilst));

        let mut moov = boxed(b"mvhd", &mvhd);
        moov.extend(boxed(b"trak", &sound));
        moov.extend(boxed(b"trak", &boxed(b"tkhd", &tkhd)));
        moov.extend(boxed(b"meta", &meta));

        let mut file = boxed(b"ftyp", b"qt  \0\0\0\0qt  ");
        file.extend(boxed(b"mdat", &[0xAB; 4096]));
        file.extend(boxed(b"moov", &moov));
        file
    }

    #[test]
    fn iphone_movies_have_date_place_length_and_upright_size() {
        let path = std::env::temp_dir().join(format!("fuji-video-{}.mov", std::process::id()));
        std::fs::write(&path, iphone_clip()).unwrap();
        let meta = read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();

        assert_eq!((meta.width, meta.height), (1080, 1920), "portrait");
        assert_eq!(meta.duration, Some(1.5));
        assert_eq!(meta.camera.as_deref(), Some("Apple iPhone 16 Pro"));
        // Local wall-clock time, like a camera's EXIF date.
        assert_eq!(
            meta.taken_at,
            Some(
                chrono::NaiveDate::from_ymd_opt(2026, 10, 21)
                    .unwrap()
                    .and_hms_opt(14, 3, 22)
                    .unwrap()
                    .and_utc()
                    .timestamp()
            )
        );
        assert_eq!(meta.lat, Some(64.1466));
        assert_eq!(meta.lon, Some(-21.9426));
        assert_eq!(meta.altitude, Some(12.345));
    }

    #[test]
    fn positions_without_altitude_and_nonsense_are_handled() {
        assert_eq!(
            iso6709("+40.6892-074.0445/"),
            Some((40.6892, -74.0445, None))
        );
        assert_eq!(
            iso6709("-33.8568+151.2153+005.0/"),
            Some((-33.8568, 151.2153, Some(5.0)))
        );
        assert_eq!(iso6709("+99.0+000.0/"), None);
        assert_eq!(iso6709("nowhere"), None);
    }

    #[test]
    fn files_without_an_index_are_errors_not_panics() {
        let path = std::env::temp_dir().join(format!("fuji-video-bad-{}.mp4", std::process::id()));
        std::fs::write(&path, b"\0\0\0\x10ftypisom\0\0\0\0\xff\xff\xff\xffmoov").unwrap();
        assert!(read(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
