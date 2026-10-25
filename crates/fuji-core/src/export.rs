//! Copies to share: the original files, or JPEGs sized for the web or
//! Instagram. Re-encoded JPEGs are sRGB, upright, and keep the date and
//! camera details; the location only when asked for.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::db::PhotoRow;
use crate::{Error, Kind, decode, discard, ingest, raf, thumbs};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Preset {
    /// The files as they are, with a RAW or Live Photo twin.
    Original,
    /// A full-size JPEG.
    Full,
    /// 2048 px on the long side: sharp on any screen, quick to upload.
    Web,
    /// Instagram's largest feed size: 1080 px wide, at most 1350 tall.
    Instagram,
}

impl Preset {
    /// The box the picture must fit in, or None for full size.
    fn bounds(self) -> Option<(u32, u32)> {
        match self {
            Preset::Web => Some((2048, 2048)),
            Preset::Instagram => Some((1080, 1350)),
            Preset::Original | Preset::Full => None,
        }
    }

    fn quality(self) -> u8 {
        match self {
            Preset::Web => 85,
            Preset::Instagram => 90,
            Preset::Original | Preset::Full => 93,
        }
    }
}

/// What an exported JPEG says about itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Info {
    pub taken_at: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<u32>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
}

impl From<&PhotoRow> for Info {
    fn from(p: &PhotoRow) -> Self {
        Info {
            taken_at: p.taken_at,
            camera: p.camera.clone(),
            lens: p.lens.clone(),
            focal_length: p.focal_length,
            aperture: p.aperture,
            shutter: p.shutter,
            iso: p.iso,
            // A borrowed position is a guess; don't publish it.
            lat: p.lat.filter(|_| !p.gps_estimated),
            lon: p.lon.filter(|_| !p.gps_estimated),
        }
    }
}

/// One photo to export.
pub struct Item {
    pub path: PathBuf,
    pub kind: Kind,
    pub info: Info,
    /// Its RAW or Live Photo twin, copied along in [`Preset::Original`].
    pub twin: Option<PathBuf>,
}

/// Fits `(w, h)` inside `bounds`, never enlarging.
pub fn fit_box((w, h): (u32, u32), bounds: Option<(u32, u32)>) -> (u32, u32) {
    let Some((bw, bh)) = bounds else {
        return (w, h);
    };
    let scale = (bw as f64 / w as f64).min(bh as f64 / h as f64).min(1.0);
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

/// Exports one photo into `dir`; returns the files written.
pub fn export(
    item: &Item,
    preset: Preset,
    location: bool,
    dir: &Path,
) -> Result<Vec<PathBuf>, Error> {
    fs::create_dir_all(dir)?;
    // Movies are shared as they are.
    if preset == Preset::Original || item.kind == Kind::Video {
        let mut written = Vec::new();
        let twin = item.twin.as_deref().filter(|_| preset == Preset::Original);
        for file in std::iter::once(item.path.as_path()).chain(twin) {
            let dest = discard::free_name(&dir.join(file.file_name().unwrap_or_default()));
            ingest::copy_verified(file, &dest)?;
            written.push(dest);
        }
        return Ok(written);
    }

    let stem = item.path.file_stem().unwrap_or_default().to_string_lossy();
    let dest = discard::free_name(&dir.join(format!("{stem}.jpg")));
    let camera_jpeg = match item.kind {
        Kind::Jpeg => Some(fs::read(&item.path)?).filter(|b| decode::disguise(b).is_none()),
        Kind::Raf => Some(raf::read_embedded_jpeg(&item.path)?),
        _ => None,
    };
    // A full-size camera JPEG is already the best JPEG there is: copy it,
    // unless the location has to come out.
    if let Some(jpeg) = &camera_jpeg
        && preset == Preset::Full
        && (location || item.info.lat.is_none())
    {
        fs::write(&dest, jpeg)?;
        return Ok(vec![dest]);
    }

    let want = preset.bounds().map_or(u32::MAX, |(w, h)| w.max(h));
    let image = crate::guard(|| match &camera_jpeg {
        Some(jpeg) => thumbs::upright(jpeg, want),
        None if item.kind == Kind::Jpeg => decode::disguised(&fs::read(&item.path)?),
        None => decode::image_at(&item.path, item.kind, want),
    })?;
    let size = fit_box((image.width, image.height), preset.bounds());
    let exif = exif_block(&item.info, location);
    let out = thumbs::resize_encode(
        image.data,
        (image.width, image.height),
        size,
        preset.quality(),
        exif.as_deref(),
    )?;
    fs::write(&dest, out.jpeg)?;
    Ok(vec![dest])
}

/// An APP1 EXIF block for `info`, or None if there's nothing to say.
fn exif_block(info: &Info, location: bool) -> Option<Vec<u8>> {
    use exif::{Field, In, Rational, Tag, Value};
    let field = |tag, value| Field {
        tag,
        ifd_num: In::PRIMARY,
        value,
    };
    let ascii = |tag, s: &str| field(tag, Value::Ascii(vec![s.as_bytes().to_vec()]));
    let ratio = |v: f64, denom: u32| Rational {
        num: (v * denom as f64).round().max(0.0) as u32,
        denom,
    };
    let mut fields = vec![ascii(Tag::Software, "Fujicloud")];
    if let Some(t) = info
        .taken_at
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
    {
        let date = t.naive_utc().format("%Y:%m:%d %H:%M:%S").to_string();
        fields.push(ascii(Tag::DateTime, &date));
        fields.push(ascii(Tag::DateTimeOriginal, &date));
    }
    if let Some(camera) = &info.camera {
        fields.push(ascii(Tag::Model, camera));
    }
    if let Some(lens) = &info.lens {
        fields.push(ascii(Tag::LensModel, lens));
    }
    if let Some(f) = info.aperture {
        fields.push(field(Tag::FNumber, Value::Rational(vec![ratio(f, 10)])));
    }
    if let Some(s) = info.shutter.filter(|s| *s > 0.0) {
        let r = if s < 1.0 {
            Rational {
                num: 1,
                denom: (1.0 / s).round() as u32,
            }
        } else {
            ratio(s, 10)
        };
        fields.push(field(Tag::ExposureTime, Value::Rational(vec![r])));
    }
    if let Some(iso) = info.iso {
        fields.push(field(
            Tag::PhotographicSensitivity,
            Value::Short(vec![iso.min(u16::MAX as u32) as u16]),
        ));
    }
    if let Some(mm) = info.focal_length {
        fields.push(field(
            Tag::FocalLength,
            Value::Rational(vec![ratio(mm, 10)]),
        ));
    }
    if location && let (Some(lat), Some(lon)) = (info.lat, info.lon) {
        let dms = |v: f64| {
            let v = v.abs();
            let (d, m) = (v.trunc(), (v.fract() * 60.0).trunc());
            let s = ((v * 60.0).fract() * 60.0).max(0.0);
            vec![ratio(d, 1), ratio(m, 1), ratio(s, 1000)]
        };
        fields.push(field(Tag::GPSVersionID, Value::Byte(vec![2, 3, 0, 0])));
        fields.push(ascii(
            Tag::GPSLatitudeRef,
            if lat < 0.0 { "S" } else { "N" },
        ));
        fields.push(field(Tag::GPSLatitude, Value::Rational(dms(lat))));
        fields.push(ascii(
            Tag::GPSLongitudeRef,
            if lon < 0.0 { "W" } else { "E" },
        ));
        fields.push(field(Tag::GPSLongitude, Value::Rational(dms(lon))));
    }
    let mut writer = exif::experimental::Writer::new();
    for f in &fields {
        writer.push_field(f);
    }
    let mut tiff = Cursor::new(Vec::new());
    writer.write(&mut tiff, false).ok()?;
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend(tiff.into_inner());
    Some(app1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta;

    fn size_of(jpeg: &[u8]) -> (u32, u32) {
        let mut d = jpeg_decoder::Decoder::new(Cursor::new(jpeg));
        d.read_info().unwrap();
        let i = d.info().unwrap();
        (i.width as u32, i.height as u32)
    }

    fn item(dir: &Path, name: &str, jpeg: &[u8]) -> Item {
        let path = dir.join(name);
        fs::write(&path, jpeg).unwrap();
        let m = meta::from_jpeg_bytes(jpeg).unwrap();
        Item {
            path,
            kind: Kind::Jpeg,
            info: Info {
                taken_at: m.taken_at,
                camera: m.camera,
                lens: m.lens,
                focal_length: m.focal_length,
                aperture: m.aperture,
                shutter: m.shutter,
                iso: m.iso,
                lat: m.lat,
                lon: m.lon,
            },
            twin: None,
        }
    }

    #[test]
    fn instagram_and_web_sizes_fit_their_boxes() {
        assert_eq!(
            fit_box((3000, 4000), Preset::Instagram.bounds()),
            (1013, 1350)
        );
        assert_eq!(
            fit_box((6000, 4000), Preset::Instagram.bounds()),
            (1080, 720)
        );
        assert_eq!(fit_box((6000, 4000), Preset::Web.bounds()), (2048, 1365));
        assert_eq!(
            fit_box((800, 600), Preset::Web.bounds()),
            (800, 600),
            "never enlarged"
        );
        assert_eq!(fit_box((6000, 4000), None), (6000, 4000));
    }

    #[test]
    fn exports_are_upright_sized_and_keep_the_date_but_not_the_place() {
        let dir = tempfile::tempdir().unwrap();
        // Portrait (orientation 6) with a GPS position.
        let jpeg = crate::TestPhoto {
            width: 300,
            height: 200,
            taken: Some("2026:10:21 09:15:30"),
            orientation: 6,
            gps: Some((64.1466, -21.9426)),
        }
        .jpeg();
        let shot = item(dir.path(), "DSCF0001.JPG", &jpeg);
        let out = dir.path().join("out");

        let [web] = export(&shot, Preset::Web, false, &out)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(web.file_name().unwrap(), "DSCF0001.jpg");
        let bytes = fs::read(&web).unwrap();
        assert_eq!(size_of(&bytes), (200, 300), "turned upright");
        let m = meta::from_jpeg_bytes(&bytes).unwrap();
        assert_eq!(m.taken_at, shot.info.taken_at);
        assert_eq!(m.camera.as_deref(), Some("X-T5"));
        assert_eq!(m.orientation, 1);
        assert_eq!(m.lat, None, "location left out");

        // Again, with the place: a second file, not an overwrite.
        let [again] = export(&shot, Preset::Web, true, &out)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(again.file_name().unwrap(), "DSCF0001 (2).jpg");
        let m = meta::from_jpeg_bytes(&fs::read(&again).unwrap()).unwrap();
        assert!((m.lat.unwrap() - 64.1466).abs() < 1e-3);
        assert!((m.lon.unwrap() + 21.9426).abs() < 1e-3);

        // Full size with the place: the camera's own file, untouched.
        let [full] = export(&shot, Preset::Full, true, &out)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(fs::read(full).unwrap(), jpeg);
    }

    #[test]
    fn originals_bring_their_twin() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = crate::test_jpeg(64, 48, Some("2026:10:21 09:15:30"));
        let mut shot = item(dir.path(), "DSCF0002.JPG", &jpeg);
        let raw = dir.path().join("DSCF0002.RAF");
        fs::write(&raw, raf::wrap_jpeg_for_tests(&jpeg)).unwrap();
        shot.twin = Some(raw);
        let out = dir.path().join("out");
        let files = export(&shot, Preset::Original, false, &out).unwrap();
        let names: Vec<_> = files
            .iter()
            .map(|f| f.file_name().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["DSCF0002.JPG", "DSCF0002.RAF"]);
    }
}
