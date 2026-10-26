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

/// How the video track is turned for display, as an EXIF orientation (1, 3,
/// 6 or 8): iPhones film portrait sideways and say so in the matrix.
#[cfg_attr(not(windows), allow(dead_code))]
fn video_orientation(moov: &[u8]) -> u16 {
    children(moov)
        .into_iter()
        .filter(|(k, _)| k == b"trak")
        .find_map(|(_, trak)| {
            let tkhd = child(trak, b"tkhd")?;
            let matrix = if tkhd.first()? == &1 { 52 } else { 40 };
            // Sound tracks have no size.
            if be32(tkhd, matrix + 36)? == 0 {
                return None;
            }
            let at = |i: usize| be32(tkhd, matrix + 4 * i).map(|v| (v as i32).signum());
            Some(match (at(0)?, at(1)?, at(3)?, at(4)?) {
                // (a, b, c, d): right becomes down, i.e. turned clockwise.
                (0, 1, -1, 0) => 6,
                (0, -1, 1, 0) => 8,
                (-1, 0, 0, -1) => 3,
                _ => 1,
            })
        })
        .unwrap_or(1)
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
/// needs Microsoft's free HEVC Video Extensions). When Explorer won't make
/// one (thumbnails switched off, or a share it won't preview), a frame is
/// decoded directly instead.
#[cfg(windows)]
pub fn poster(path: &Path, long_edge: u32) -> Result<crate::decode::Rgb, Error> {
    shell_poster(path, long_edge)
        .or_else(|shell| decoded_frame(path).map_err(|frame| bad(&format!("{shell}; {frame}"))))
}

/// A frame decoded with Media Foundation, about a second in (first frames
/// are often black), turned upright by the track's matrix.
#[cfg(windows)]
pub fn decoded_frame(path: &Path) -> Result<crate::decode::Rgb, Error> {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::Win32::System::Variant::VT_I8;
    use windows::core::{GUID, HSTRING};

    let orientation = read_moov(path).map_or(1, |moov| video_orientation(&moov));
    let mf = |e: windows::core::Error| bad(&format!("Windows couldn't decode a frame ({e})"));
    // SAFETY: Media Foundation calls on this thread; the buffer is unlocked
    // and everything started is shut down before returning.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let started = MFStartup(MF_VERSION, MFSTARTUP_FULL).is_ok();
        let result = (|| -> Result<crate::decode::Rgb, Error> {
            let mut attributes = None;
            MFCreateAttributes(&mut attributes, 1).map_err(mf)?;
            let attributes = attributes.ok_or_else(|| bad("no attributes"))?;
            // Let the reader convert whatever the codec gives to RGB.
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(mf)?;
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), &attributes)
                .map_err(mf)?;
            let stream = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(mf)?;
            reader.SetStreamSelection(stream, true).map_err(mf)?;
            let rgb = MFCreateMediaType().map_err(mf)?;
            rgb.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(mf)?;
            rgb.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(mf)?;
            reader.SetCurrentMediaType(stream, None, &rgb).map_err(mf)?;
            let current = reader.GetCurrentMediaType(stream).map_err(mf)?;
            let size = current.GetUINT64(&MF_MT_FRAME_SIZE).map_err(mf)?;
            let (w, h) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
            if w == 0 || h == 0 || w * h > crate::MAX_PIXELS as usize {
                return Err(bad("odd frame size"));
            }
            let stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map_or(w as isize * 4, |s| s as i32 as isize);

            let duration = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .map_or(0, |v| v.Anonymous.Anonymous.Anonymous.uhVal);
            // 100 ns units: a second in, or a tenth of a short clip.
            let mut at = PROPVARIANT::default();
            (*at.Anonymous.Anonymous).vt = VT_I8;
            (*at.Anonymous.Anonymous).Anonymous.hVal = (duration / 10).min(10_000_000) as i64;
            let _ = reader.SetCurrentPosition(&GUID::zeroed(), &at);

            for _ in 0..240 {
                let mut flags = 0u32;
                let mut sample = None;
                reader
                    .ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .map_err(mf)?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else {
                    continue;
                };
                let buffer = sample.ConvertToContiguousBuffer().map_err(mf)?;
                let mut bytes = std::ptr::null_mut();
                let mut len = 0u32;
                buffer.Lock(&mut bytes, None, Some(&mut len)).map_err(mf)?;
                let data = std::slice::from_raw_parts(bytes, len as usize);
                let row = stride.unsigned_abs();
                let mut out = Vec::with_capacity(w * h * 3);
                for y in 0..h {
                    // A negative stride means the rows run bottom-up.
                    let line = if stride < 0 { h - 1 - y } else { y };
                    let Some(px) = data.get(line * row..line * row + w * 4) else {
                        break;
                    };
                    out.extend(
                        px.as_chunks::<4>()
                            .0
                            .iter()
                            .flat_map(|p| [p[2], p[1], p[0]]),
                    );
                }
                let _ = buffer.Unlock();
                if out.len() < w * h * 3 {
                    return Err(bad("short frame"));
                }
                let (data, width, height) =
                    crate::thumbs::orient(out, w as u32, h as u32, orientation);
                return Ok(crate::decode::Rgb {
                    data,
                    width,
                    height,
                });
            }
            Err(bad("no frame"))
        })();
        if started {
            let _ = MFShutdown();
        }
        if com {
            CoUninitialize();
        }
        result
    }
}

#[cfg(windows)]
fn shell_poster(path: &Path, long_edge: u32) -> Result<crate::decode::Rgb, Error> {
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

/// Re-encodes a movie as H.264/AAC MP4 with Windows' own codecs, for the
/// web view, which can't play every format Windows can (iPhone HEVC).
/// `progress` gets 0–1.
#[cfg(windows)]
pub fn transcode_h264(src: &Path, dest: &Path, progress: &dyn Fn(f32)) -> Result<(), Error> {
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::core::{GUID, HSTRING, IUnknown, Interface};

    let mf = |e: windows::core::Error| bad(&format!("Windows couldn't convert the movie ({e})"));
    // SAFETY: Media Foundation calls on this thread; everything started is
    // shut down before returning.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let started = MFStartup(MF_VERSION, MFSTARTUP_FULL).is_ok();
        let result = (|| -> Result<(), Error> {
            let resolver = MFCreateSourceResolver().map_err(mf)?;
            let mut kind = MF_OBJECT_INVALID;
            let mut object: Option<IUnknown> = None;
            resolver
                .CreateObjectFromURL(
                    &HSTRING::from(src.as_os_str()),
                    (MF_RESOLUTION_MEDIASOURCE.0 | MF_RESOLUTION_READ.0) as u32,
                    None,
                    &mut kind,
                    &mut object,
                )
                .map_err(mf)?;
            let source: IMFMediaSource =
                object.ok_or_else(|| bad("no movie"))?.cast().map_err(mf)?;
            let duration = source
                .CreatePresentationDescriptor()
                .and_then(|pd| pd.GetUINT64(&MF_PD_DURATION))
                .unwrap_or(0);

            let attributes = || -> Result<IMFAttributes, Error> {
                let mut a = None;
                MFCreateAttributes(&mut a, 8).map_err(mf)?;
                a.ok_or_else(|| bad("no attributes"))
            };
            let profile = MFCreateTranscodeProfile().map_err(mf)?;
            let video = attributes()?;
            video
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
                .map_err(mf)?;
            video
                .SetUINT32(&MF_MT_AVG_BITRATE, 12_000_000)
                .map_err(mf)?;
            profile.SetVideoAttributes(&video).map_err(mf)?;
            let audio = attributes()?;
            audio
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)
                .map_err(mf)?;
            audio.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2).map_err(mf)?;
            audio
                .SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)
                .map_err(mf)?;
            audio
                .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .map_err(mf)?;
            audio
                .SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000)
                .map_err(mf)?;
            profile.SetAudioAttributes(&audio).map_err(mf)?;
            let container = attributes()?;
            container
                .SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4)
                .map_err(mf)?;
            // Size, frame rate and the rest as in the original.
            container
                .SetUINT32(
                    &MF_TRANSCODE_ADJUST_PROFILE,
                    MF_TRANSCODE_ADJUST_PROFILE_USE_SOURCE_ATTRIBUTES.0 as u32,
                )
                .map_err(mf)?;
            profile.SetContainerAttributes(&container).map_err(mf)?;

            let topology =
                MFCreateTranscodeTopology(&source, &HSTRING::from(dest.as_os_str()), &profile)
                    .map_err(mf)?;
            let session = MFCreateMediaSession(None).map_err(mf)?;
            session.SetTopology(0, &topology).map_err(mf)?;
            session
                .Start(&GUID::zeroed(), &PROPVARIANT::default())
                .map_err(mf)?;
            let clock: Option<IMFPresentationClock> =
                session.GetClock().ok().and_then(|c| c.cast().ok());
            let mut outcome = Ok(());
            let mut closing = false;
            loop {
                let event = match session.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                    Ok(event) => event,
                    Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                        if let (Some(clock), true) = (&clock, duration > 0)
                            && let Ok(t) = clock.GetTime()
                        {
                            progress((t as f64 / duration as f64).clamp(0.0, 1.0) as f32);
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                    Err(e) => {
                        outcome = Err(mf(e));
                        break;
                    }
                };
                let kind = event.GetType().unwrap_or(0);
                if let Ok(status) = event.GetStatus()
                    && status.is_err()
                    && outcome.is_ok()
                {
                    outcome = Err(mf(status.into()));
                }
                if kind == MESessionClosed.0 as u32 {
                    break;
                }
                if (kind == MESessionEnded.0 as u32 || outcome.is_err()) && !closing {
                    closing = true;
                    if session.Close().is_err() {
                        break;
                    }
                }
            }
            let _ = source.Shutdown();
            let _ = session.Shutdown();
            if outcome.is_ok() {
                progress(1.0);
            }
            outcome
        })();
        if started {
            let _ = MFShutdown();
        }
        if com {
            CoUninitialize();
        }
        result
    }
}

#[cfg(not(windows))]
pub fn transcode_h264(_: &Path, _: &Path, _: &dyn Fn(f32)) -> Result<(), Error> {
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
        assert_eq!(meta.camera.as_deref(), Some("iPhone 16 Pro"));
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
    fn a_movie_says_which_way_up_it_is() {
        let file = iphone_clip();
        let path = std::env::temp_dir().join(format!("fuji-video-o-{}.mov", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let moov = read_moov(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        // iPhone portrait: turned clockwise, past the sound track.
        assert_eq!(video_orientation(&moov), 6);
        assert_eq!(video_orientation(&[]), 1);
    }

    /// The Media Foundation frame grabber on a real H.264 clip (Windows,
    /// downloaded; run with `--ignored`).
    #[test]
    #[ignore]
    #[cfg(all(windows, feature = "smart-search"))]
    fn real_movie_frames_are_decoded() {
        let dir = std::env::temp_dir().join("fujicloud-movie-test");
        let path = dir.join("movie_5.mp4");
        crate::embed::download_files(
            &[(
                "https://raw.githubusercontent.com/web-platform-tests/wpt/master/media/movie_5.mp4",
                path.clone(),
            )],
            &|_, _| {},
        )
        .expect("download clip");
        let frame = decoded_frame(&path).expect("frame");
        println!("frame {}x{}", frame.width, frame.height);
        assert!(frame.width >= 100 && frame.height >= 100);
        // Not a blank frame.
        let (lo, hi) = frame
            .data
            .iter()
            .fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(hi - lo > 40, "flat frame {lo}..{hi}");
        // And the full poster path, Explorer first.
        let poster = poster(&path, 512).expect("poster");
        println!("poster {}x{}", poster.width, poster.height);
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
