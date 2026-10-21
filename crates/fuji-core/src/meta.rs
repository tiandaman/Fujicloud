//! Metadata extraction for every supported file kind.

use std::fs::File;
use std::io::{BufRead, BufReader, Cursor, Seek};
use std::path::Path;

use exif::{Exif, In, Tag, Value};
use serde::Serialize;

use crate::{Error, Kind, fuji, raf};

#[derive(Debug, Default, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMeta {
    /// Capture time as naive local seconds since the epoch (camera clock).
    pub taken_at: Option<i64>,
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
        Kind::Jpeg => from_jpeg_stream(&mut BufReader::with_capacity(64 * 1024, File::open(path)?)),
        Kind::Heif | Kind::Tiff => {
            let exif =
                exif::Reader::new().read_from_container(&mut BufReader::new(File::open(path)?))?;
            Ok(from_exif(&exif))
        }
    }
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

    let make = text(Tag::Make);
    let model = text(Tag::Model);
    let camera = match (make, model) {
        // "FUJIFILM" + "X-T5" reads better as just the model.
        (Some(make), Some(model)) if model.to_lowercase().contains(&make.to_lowercase()) => {
            Some(model)
        }
        (Some(make), Some(model)) if make.eq_ignore_ascii_case("fujifilm") => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => model.or(make),
    };

    let fuji = match field(Tag::MakerNote) {
        Some(Value::Undefined(bytes, _)) => fuji::parse_makernote(bytes),
        _ => None,
    }
    .unwrap_or_default();

    PhotoMeta {
        taken_at,
        width,
        height,
        orientation,
        camera,
        lens: text(Tag::LensModel),
        focal_length: rational(Tag::FocalLength),
        aperture: rational(Tag::FNumber),
        shutter: rational(Tag::ExposureTime),
        iso: uint(Tag::PhotographicSensitivity),
        exposure_comp: rational(Tag::ExposureBiasValue),
        film_sim: fuji.film_simulation,
        grain: fuji.grain,
        dynamic_range: fuji.dynamic_range,
    }
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
