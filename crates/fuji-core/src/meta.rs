//! Metadata extraction for every supported file kind.

use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Read, Seek};
use std::path::Path;

use exif::{Exif, In, Tag, Value};
use serde::Serialize;

use crate::{Error, Kind, fuji, raf};

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMeta {
    /// Capture time as naive local seconds since the epoch (camera clock).
    pub taken_at: Option<i64>,
    /// The camera clock's offset from UTC in minutes, when it says (iPhones
    /// always do; Fujifilm bodies with their time zone set do).
    pub utc_offset: Option<i32>,
    /// Display dimensions, i.e. already swapped for portrait orientation.
    pub width: u32,
    pub height: u32,
    pub orientation: u16,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<u32>,
    pub exposure_comp: Option<f64>,
    pub film_sim: Option<String>,
    pub grain: Option<String>,
    pub dynamic_range: Option<String>,
    /// The full in-camera film recipe (Fujifilm bodies only).
    pub recipe: Option<fuji::Recipe>,
    /// GPS position in decimal degrees, e.g. from the phone's location sync.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude: Option<f64>,
    /// Length in seconds, for movies.
    pub duration: Option<f64>,
}

/// "+09:00", "-0700" or "Z" → minutes east of UTC.
pub fn utc_offset(s: &str) -> Option<i32> {
    let s = s.trim().trim_end_matches('\0');
    if s == "Z" {
        return Some(0);
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits: String = s[1..].chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 4 {
        return None;
    }
    let (h, m): (i32, i32) = (digits[..2].parse().ok()?, digits[2..].parse().ok()?);
    (h <= 14 && m < 60).then_some(sign * (h * 60 + m))
}

/// Reads metadata touching as few bytes as possible: for JPEG the reader stops
/// at the APP1 segment, for RAF only the header and the preview's EXIF.
pub fn read(path: &Path, kind: Kind) -> Result<PhotoMeta, Error> {
    match kind {
        Kind::Raf => {
            // EXIF and the frame header live in the first few hundred KB of the
            // preview; only fall back to the whole preview for unusual files.
            let prefix = raf::read_embedded_jpeg_prefix(path, 512 * 1024)?;
            from_jpeg_stream(&mut Cursor::new(prefix))
                .or_else(|_| from_jpeg_stream(&mut Cursor::new(raf::read_embedded_jpeg(path)?)))
        }
        Kind::Jpeg => from_jpeg_stream(&mut BufReader::with_capacity(64 * 1024, File::open(path)?))
            .or_else(|e| {
                // A HEIC or PNG saved under a .jpg name.
                let mut head = [0u8; 16];
                File::open(path)?.read_exact(&mut head)?;
                match crate::decode::disguise(&head) {
                    Some(crate::decode::Disguise::Heif) => heif(path),
                    Some(crate::decode::Disguise::Png) => png(path),
                    None => Err(e),
                }
            }),
        Kind::Heif => heif(path),
        Kind::Png => png(path),
        Kind::Dng => dng(path),
        Kind::Video => crate::video::read(path),
        // Edits can be hundreds of MB; read just the header. Date, camera and
        // place come from the original they're linked to.
        Kind::Tiff | Kind::Psd => {
            let (width, height, orientation) = crate::decode::dimensions(path, kind)?;
            Ok(PhotoMeta {
                width,
                height,
                orientation,
                ..Default::default()
            })
        }
    }
}

/// HEIC/HEIF: EXIF for the date, camera and place, and the size as shown.
/// The decoder applies the container's rotation, so the EXIF orientation
/// mustn't be applied again.
fn heif(path: &Path) -> Result<PhotoMeta, Error> {
    let mut meta = exif::Reader::new()
        .read_from_container(&mut BufReader::new(File::open(path)?))
        .map(|exif| from_exif(&exif))
        .unwrap_or_default();
    if meta.width == 0 || meta.height == 0 {
        (meta.width, meta.height) = crate::decode::heif_size(&std::fs::read(path)?)?;
    }
    meta.orientation = 1;
    Ok(meta)
}

/// PNG, e.g. a screenshot: the size, plus EXIF when there is any (a date
/// and the device). Without it, the file's date stands in.
fn png(path: &Path) -> Result<PhotoMeta, Error> {
    let reader = png::Decoder::new(BufReader::new(File::open(path)?))
        .read_info()
        .map_err(|e| Error::Decode(format!("PNG: {e}")))?;
    let mut meta = exif::Reader::new()
        .read_from_container(&mut BufReader::new(File::open(path)?))
        .map(|exif| from_exif(&exif))
        .unwrap_or_default();
    (meta.width, meta.height, meta.orientation) = (reader.info().width, reader.info().height, 1);
    Ok(meta)
}

/// DNG (iPhone ProRAW): EXIF from the start of the file, which is where
/// cameras put it, and the developed size from the raw's own directory.
fn dng(path: &Path) -> Result<PhotoMeta, Error> {
    let mut head = Vec::new();
    File::open(path)?.take(1 << 20).read_to_end(&mut head)?;
    let exif = exif::Reader::new().read_raw(head).or_else(|_| {
        exif::Reader::new().read_from_container(&mut BufReader::new(File::open(path)?))
    });
    let found = exif.map(|e| from_exif(&e));
    let info = match (crate::dng::info(path), found.is_ok()) {
        (Ok(info), _) => info,
        // An unusual layout: what EXIF says will do.
        (Err(_), true) => return Ok(found.unwrap_or_default()),
        (Err(e), false) => return Err(e),
    };
    let mut meta = found.unwrap_or_default();
    meta.orientation = info.orientation;
    (meta.width, meta.height) = if swaps_axes(info.orientation) {
        (info.height, info.width)
    } else {
        (info.width, info.height)
    };
    Ok(meta)
}

pub fn from_jpeg_bytes(bytes: &[u8]) -> Result<PhotoMeta, Error> {
    from_jpeg_stream(&mut Cursor::new(bytes))
}

fn from_jpeg_stream<R: BufRead + Seek>(reader: &mut R) -> Result<PhotoMeta, Error> {
    let start = reader.stream_position()?;
    let exif = exif::Reader::new().read_from_container(reader);
    let mut meta = match &exif {
        Ok(exif) => from_exif(exif),
        Err(exif::Error::NotFound(_)) => PhotoMeta {
            orientation: 1,
            ..Default::default()
        },
        Err(e) => return Err(Error::Exif(e.to_string())),
    };
    if meta.width == 0 || meta.height == 0 {
        // EXIF dimensions are optional; the SOF header is authoritative and
        // sits right after the metadata segments.
        reader.seek(std::io::SeekFrom::Start(start))?;
        let mut decoder = jpeg_decoder::Decoder::new(reader);
        decoder
            .read_info()
            .map_err(|e| Error::Decode(e.to_string()))?;
        if let Some(info) = decoder.info() {
            let (w, h) = (info.width as u32, info.height as u32);
            (meta.width, meta.height) = if swaps_axes(meta.orientation) {
                (h, w)
            } else {
                (w, h)
            };
        }
    }
    Ok(meta)
}

/// Phones name each of their cameras: "iPhone 15 Pro back triple camera
/// 6.86mm f/1.78". Which phone it was is enough: "iPhone 15 Pro".
pub fn phone_lens(lens: &str) -> String {
    let lower = lens.to_lowercase();
    let side = [" back ", " front "]
        .iter()
        .filter_map(|s| lower.find(s))
        .min();
    match side {
        Some(at) if lower[at..].contains("camera") => lens[..at].trim().to_owned(),
        _ => lens.to_owned(),
    }
}

pub fn orientation(exif: &Exif) -> u16 {
    exif.get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .filter(|o| (1..=8).contains(o))
        .unwrap_or(1) as u16
}

pub fn swaps_axes(orientation: u16) -> bool {
    (5..=8).contains(&orientation)
}

fn from_exif(exif: &Exif) -> PhotoMeta {
    let field = |tag| exif.get_field(tag, In::PRIMARY).map(|f| &f.value);
    let text = |tag| match field(tag) {
        Some(Value::Ascii(v)) => v
            .first()
            .map(|s| {
                String::from_utf8_lossy(s)
                    .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                    .to_owned()
            })
            .filter(|s| !s.is_empty()),
        _ => None,
    };
    let rational = |tag| match field(tag) {
        Some(Value::Rational(v)) => v.first().filter(|r| r.denom != 0).map(|r| r.to_f64()),
        Some(Value::SRational(v)) => v.first().filter(|r| r.denom != 0).map(|r| r.to_f64()),
        _ => None,
    };
    let uint = |tag| field(tag).and_then(|v| v.get_uint(0));

    let orientation = orientation(exif);
    let (w, h) = (
        uint(Tag::PixelXDimension).unwrap_or(0),
        uint(Tag::PixelYDimension).unwrap_or(0),
    );
    let (width, height) = if swaps_axes(orientation) {
        (h, w)
    } else {
        (w, h)
    };

    let taken_at = [Tag::DateTimeOriginal, Tag::DateTimeDigitized, Tag::DateTime]
        .into_iter()
        .find_map(|tag| match field(tag) {
            Some(Value::Ascii(v)) => v.first().and_then(|s| parse_exif_datetime(s)),
            _ => None,
        });
    let utc_offset = [
        Tag::OffsetTimeOriginal,
        Tag::OffsetTimeDigitized,
        Tag::OffsetTime,
    ]
    .into_iter()
    .find_map(|tag| match field(tag) {
        Some(Value::Ascii(v)) => v
            .first()
            .and_then(|s| utc_offset(std::str::from_utf8(s).ok()?)),
        _ => None,
    });

    let camera = camera_name(text(Tag::Make), text(Tag::Model));
    let lens = text(Tag::LensModel).map(|l| phone_lens(&l));

    let recipe = match field(Tag::MakerNote) {
        Some(Value::Undefined(bytes, _)) => fuji::parse_makernote(bytes),
        _ => None,
    };
    let (lat, lon, altitude) = gps(exif).map_or((None, None, None), |(lat, lon, alt)| {
        (Some(lat), Some(lon), alt)
    });

    PhotoMeta {
        taken_at,
        utc_offset,
        width,
        height,
        orientation,
        camera,
        lens,
        focal_length: rational(Tag::FocalLength),
        aperture: rational(Tag::FNumber),
        shutter: rational(Tag::ExposureTime),
        iso: uint(Tag::PhotographicSensitivity),
        exposure_comp: rational(Tag::ExposureBiasValue),
        film_sim: recipe.as_ref().and_then(|r| r.film_sim.clone()),
        grain: recipe.as_ref().and_then(|r| r.grain.clone()),
        dynamic_range: recipe.as_ref().and_then(|r| r.dynamic_range.clone()),
        recipe,
        lat,
        lon,
        altitude,
        duration: None,
    }
}

/// "FUJIFILM" + "X-T5" reads better as just the model, as does "Apple" +
/// "iPhone 16 Pro".
pub(crate) fn camera_name(make: Option<String>, model: Option<String>) -> Option<String> {
    match (make, model) {
        (Some(make), Some(model)) if model.to_lowercase().contains(&make.to_lowercase()) => {
            Some(model)
        }
        (Some(make), Some(model))
            if ["fujifilm", "apple", "google"]
                .iter()
                .any(|m| make.eq_ignore_ascii_case(m)) =>
        {
            Some(model)
        }
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => model.or(make),
    }
}

/// `(latitude, longitude, altitude)` in degrees and metres, if the camera
/// recorded a position.
fn gps(exif: &Exif) -> Option<(f64, f64, Option<f64>)> {
    let field = |tag| exif.get_field(tag, In::PRIMARY).map(|f| &f.value);
    let degrees = |tag| match field(tag) {
        Some(Value::Rational(v)) if v.len() >= 3 && v.iter().all(|r| r.denom != 0) => {
            Some(v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0)
        }
        _ => None,
    };
    let hemisphere = |tag| match field(tag) {
        Some(Value::Ascii(v)) => v.first().and_then(|s| s.first().copied()),
        _ => None,
    };
    let mut lat = degrees(Tag::GPSLatitude)?;
    let mut lon = degrees(Tag::GPSLongitude)?;
    if hemisphere(Tag::GPSLatitudeRef) == Some(b'S') {
        lat = -lat;
    }
    if hemisphere(Tag::GPSLongitudeRef) == Some(b'W') {
        lon = -lon;
    }
    // Some firmwares write zeros when there was no fix.
    if (lat == 0.0 && lon == 0.0) || lat.abs() > 90.0 || lon.abs() > 180.0 {
        return None;
    }
    let altitude = match field(Tag::GPSAltitude) {
        Some(Value::Rational(v)) => v.first().filter(|r| r.denom != 0).map(|r| {
            let below =
                matches!(field(Tag::GPSAltitudeRef), Some(Value::Byte(b)) if b.first() == Some(&1));
            if below { -r.to_f64() } else { r.to_f64() }
        }),
        _ => None,
    };
    Some((lat, lon, altitude))
}

fn parse_exif_datetime(s: &[u8]) -> Option<i64> {
    let dt = exif::DateTime::from_ascii(s).ok()?;
    let date = chrono::NaiveDate::from_ymd_opt(dt.year as i32, dt.month as u32, dt.day as u32)?;
    let time = chrono::NaiveTime::from_hms_opt(dt.hour as u32, dt.minute as u32, dt.second as u32)?;
    Some(date.and_time(time).and_utc().timestamp())
}

/// Every EXIF field rendered for humans, for the info panel.
pub fn all_fields(path: &Path, kind: Kind) -> Result<Vec<(String, String)>, Error> {
    let exif = match kind {
        Kind::Tiff | Kind::Psd | Kind::Video => return Ok(Vec::new()),
        Kind::Raf => exif::Reader::new()
            .read_from_container(&mut Cursor::new(raf::read_embedded_jpeg(path)?))?,
        _ => exif::Reader::new().read_from_container(&mut BufReader::new(File::open(path)?))?,
    };
    Ok(exif
        .fields()
        .filter(|f| f.ifd_num == In::PRIMARY && !matches!(f.tag, Tag::MakerNote | Tag::UserComment))
        .filter(|f| !matches!(f.value, Value::Undefined(ref v, _) if v.len() > 64))
        .map(|f| {
            (
                f.tag.to_string(),
                f.display_value().with_unit(&exif).to_string(),
            )
        })
        .collect())
}

#[cfg(test)]
mod offset_tests {
    use super::utc_offset;

    #[test]
    fn utc_offsets_are_read_in_every_spelling() {
        assert_eq!(utc_offset("+09:00"), Some(540));
        assert_eq!(utc_offset("-07:00"), Some(-420));
        assert_eq!(utc_offset("+0530"), Some(330));
        assert_eq!(utc_offset("Z"), Some(0));
        assert_eq!(utc_offset("+05:45\0"), Some(345));
        assert_eq!(utc_offset("+9"), None);
        assert_eq!(utc_offset("   :  "), None);
        assert_eq!(utc_offset("+25:00"), None);
    }
}
