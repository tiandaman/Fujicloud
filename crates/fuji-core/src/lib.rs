//! Core of Fujicloud: everything that touches files lives here, independent
//! of the desktop shell, so it can be tested and benchmarked on its own.

#[cfg(feature = "smart-search")]
pub mod animals;
pub mod backup;
pub mod db;
pub mod decode;
pub mod discard;
pub mod dng;
pub mod dupes;
#[cfg(feature = "smart-search")]
pub mod embed;
pub mod export;
#[cfg(feature = "smart-search")]
pub mod faces;
pub mod ffmpeg;
pub mod fuji;
pub mod geo;
pub mod hdr;
pub mod ingest;
pub mod meta;
pub mod people;
pub mod raf;
pub mod scan;
pub mod search;
pub mod thumbs;
pub mod video;
pub mod xmp;

use std::path::Path;

pub use db::Library;
pub use meta::PhotoMeta;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Jpeg,
    Raf,
    Heif,
    Tiff,
    /// Photoshop document (PSD or PSB), usually an edit of a camera file.
    Psd,
    /// Screenshots and other PNG pictures.
    Png,
    /// Adobe DNG raw, e.g. iPhone ProRAW.
    Dng,
    /// QuickTime or MP4 movie, including the moving part of a Live Photo.
    Video,
}

impl Kind {
    pub fn from_path(path: &Path) -> Option<Kind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "jpg" | "jpeg" => Kind::Jpeg,
            "raf" => Kind::Raf,
            "hif" | "heif" | "heic" => Kind::Heif,
            "tif" | "tiff" => Kind::Tiff,
            "psd" | "psb" => Kind::Psd,
            "png" => Kind::Png,
            "dng" => Kind::Dng,
            "mov" | "mp4" | "m4v" => Kind::Video,
            _ => return None,
        })
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "jpeg" => Kind::Jpeg,
            "raf" => Kind::Raf,
            "heif" => Kind::Heif,
            "tiff" => Kind::Tiff,
            "psd" => Kind::Psd,
            "png" => Kind::Png,
            "dng" => Kind::Dng,
            "video" => Kind::Video,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpeg",
            Kind::Raf => "raf",
            Kind::Heif => "heif",
            Kind::Tiff => "tiff",
            Kind::Psd => "psd",
            Kind::Png => "png",
            Kind::Dng => "dng",
            Kind::Video => "video",
        }
    }

    pub fn is_raw(self) -> bool {
        matches!(self, Kind::Raf | Kind::Dng)
    }

    /// Formats edits are saved in, as opposed to straight-from-camera files.
    pub fn is_edit_format(self) -> bool {
        matches!(self, Kind::Tiff | Kind::Psd)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("database: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("exif: {0}")]
    Exif(String),
    #[error("decode: {0}")]
    Decode(String),
    #[error("no previews for {0:?} files yet")]
    Unsupported(Kind),
    #[error("photo {0} not found")]
    NotFound(i64),
    #[error("smart search: {0}")]
    Model(String),
}

/// The most pixels an image header may claim before we refuse to decode it.
/// Real photos are far below this; a damaged header can claim billions, and
/// trying to allocate that ends the app.
pub(crate) const MAX_PIXELS: u64 = 200_000_000;

/// Runs `f`, turning a panic into an error. Libraries can trip over unusual
/// or damaged files, and one such photo must not stop a scan, an import or
/// thumbnail building for the rest. The panic itself is logged by the app.
pub fn guard<T>(f: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(Error::Decode("this file tripped an internal error".into())))
}

impl From<exif::Error> for Error {
    fn from(e: exif::Error) -> Self {
        Error::Exif(e.to_string())
    }
}

impl Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

/// A small gradient JPEG, optionally with a Fujifilm-style EXIF block.
#[doc(hidden)]
pub fn test_jpeg(width: u16, height: u16, taken: Option<&str>) -> Vec<u8> {
    test_jpeg_with(width, height, taken, 1)
}

#[doc(hidden)]
pub fn test_jpeg_with(width: u16, height: u16, taken: Option<&str>, orientation: u16) -> Vec<u8> {
    TestPhoto {
        width,
        height,
        taken,
        orientation,
        gps: None,
    }
    .jpeg()
}

/// Builds synthetic camera JPEGs for tests and benchmarks.
#[doc(hidden)]
pub struct TestPhoto<'a> {
    pub width: u16,
    pub height: u16,
    /// EXIF DateTimeOriginal; `None` writes no EXIF at all.
    pub taken: Option<&'a str>,
    pub orientation: u16,
    pub gps: Option<(f64, f64)>,
}

impl TestPhoto<'_> {
    pub fn jpeg(&self) -> Vec<u8> {
        use exif::{Field, In, Rational, Tag, Value};

        let (width, height) = (self.width, self.height);
        let mut rgb = Vec::with_capacity(width as usize * height as usize * 3);
        for y in 0..height {
            for x in 0..width {
                rgb.extend_from_slice(&[
                    (x as u32 * 255 / width as u32) as u8,
                    (y as u32 * 255 / height as u32) as u8,
                    128,
                ]);
            }
        }
        let mut out = Vec::new();
        let mut encoder = jpeg_encoder::Encoder::new(&mut out, 90);
        if let Some(taken) = self.taken {
            let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
            let rational = |num, denom| Value::Rational(vec![Rational { num, denom }]);
            let dms = |deg: f64| {
                let deg = deg.abs();
                let secs = (deg * 3600.0 * 100.0).round() as u32;
                Value::Rational(vec![
                    Rational {
                        num: secs / 360_000,
                        denom: 1,
                    },
                    Rational {
                        num: secs / 6000 % 60,
                        denom: 1,
                    },
                    Rational {
                        num: secs % 6000,
                        denom: 100,
                    },
                ])
            };
            // Classic Neg. with a little grain, like a typical recipe.
            let makernote = fuji::build_makernote_for_tests(&[
                (0x1003, 3, &[0]),
                (0x1047, 9, &[32]),
                (0x104c, 3, &[16]),
                (0x1401, 3, &[0x800]),
            ]);
            let mut values = vec![
                (Tag::Make, ascii("FUJIFILM")),
                (Tag::Model, ascii("X-T5")),
                (Tag::Orientation, Value::Short(vec![self.orientation])),
                (Tag::DateTimeOriginal, ascii(taken)),
                (Tag::LensModel, ascii("XF23mmF1.4 R LM WR")),
                (Tag::FNumber, rational(28, 10)),
                (Tag::ExposureTime, rational(1, 250)),
                (Tag::FocalLength, rational(23, 1)),
                (Tag::PhotographicSensitivity, Value::Short(vec![160])),
                (Tag::MakerNote, Value::Undefined(makernote, 0)),
            ];
            if let Some((lat, lon)) = self.gps {
                values.extend([
                    (
                        Tag::GPSLatitudeRef,
                        ascii(if lat < 0.0 { "S" } else { "N" }),
                    ),
                    (Tag::GPSLatitude, dms(lat)),
                    (
                        Tag::GPSLongitudeRef,
                        ascii(if lon < 0.0 { "W" } else { "E" }),
                    ),
                    (Tag::GPSLongitude, dms(lon)),
                    (Tag::GPSAltitude, rational(42, 1)),
                ]);
            }
            let fields: Vec<Field> = values
                .into_iter()
                .map(|(tag, value)| Field {
                    tag,
                    ifd_num: In::PRIMARY,
                    value,
                })
                .collect();
            let mut writer = exif::experimental::Writer::new();
            for f in &fields {
                writer.push_field(f);
            }
            let mut tiff = std::io::Cursor::new(Vec::new());
            writer.write(&mut tiff, true).expect("exif encodes");
            let mut app1 = b"Exif\0\0".to_vec();
            app1.extend_from_slice(&tiff.into_inner());
            encoder.add_app_segment(1, app1).expect("app1 fits");
        }
        encoder
            .encode(&rgb, width, height, jpeg_encoder::ColorType::Rgb)
            .expect("jpeg encodes");
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_turns_a_panic_into_an_error() {
        let failed: Result<(), Error> = guard(|| panic!("an unusual file"));
        assert!(matches!(failed, Err(Error::Decode(_))));
        assert_eq!(guard(|| Ok(7)).unwrap(), 7);
    }

    #[test]
    fn reads_fuji_metadata_from_jpeg_and_raf() {
        let jpeg = test_jpeg_with(60, 40, Some("2026:10:21 09:15:30"), 6);
        let m = meta::from_jpeg_bytes(&jpeg).unwrap();
        assert_eq!(m.camera.as_deref(), Some("X-T5"));
        assert_eq!(m.lens.as_deref(), Some("XF23mmF1.4 R LM WR"));
        assert_eq!(m.film_sim.as_deref(), Some("Classic Neg."));
        assert_eq!(m.iso, Some(160));
        assert_eq!(m.aperture, Some(2.8));
        assert_eq!(m.shutter, Some(0.004));
        assert_eq!(m.orientation, 6);
        // Portrait: dimensions come from the frame header, swapped.
        assert_eq!((m.width, m.height), (40, 60));
        assert_eq!(m.taken_at, Some(1792574130));

        let dir = tempfile::tempdir().unwrap();
        let raf = dir.path().join("DSCF0001.RAF");
        std::fs::write(&raf, raf::wrap_jpeg_for_tests(&jpeg)).unwrap();
        assert_eq!(meta::read(&raf, Kind::Raf).unwrap(), m);
        assert_eq!(m.recipe.unwrap().grain.as_deref(), Some("Weak, Small"));
        assert_eq!(m.lat, None);
    }

    #[test]
    fn reads_gps_from_location_sync() {
        let jpeg = TestPhoto {
            width: 32,
            height: 32,
            taken: Some("2026:04:02 06:30:00"),
            orientation: 1,
            // Fushimi Inari, Kyoto.
            gps: Some((34.96714, 135.77267)),
        }
        .jpeg();
        let m = meta::from_jpeg_bytes(&jpeg).unwrap();
        assert!((m.lat.unwrap() - 34.96714).abs() < 1e-4);
        assert!((m.lon.unwrap() - 135.77267).abs() < 1e-4);
        assert_eq!(m.altitude, Some(42.0));

        let south_west = TestPhoto {
            gps: Some((-22.9519, -43.2105)),
            taken: Some("2026:01:01 12:00:00"),
            width: 8,
            height: 8,
            orientation: 1,
        };
        let m = meta::from_jpeg_bytes(&south_west.jpeg()).unwrap();
        assert!(m.lat.unwrap() < 0.0 && m.lon.unwrap() < 0.0);
    }

    #[test]
    fn photoshop_edits_get_previews_and_thumbnails() {
        let dir = tempfile::tempdir().unwrap();
        let rgb: Vec<u8> = (0..900u32 * 600)
            .flat_map(|i| [(i % 900 / 4) as u8, 90, 160])
            .collect();
        let path = dir.path().join("DSCF0003.psd");
        std::fs::write(&path, decode::write_test_psd(900, 600, &rgb, 8, true, &[])).unwrap();
        let m = meta::read(&path, Kind::Psd).unwrap();
        assert_eq!((m.width, m.height), (900, 600));

        let cache = thumbs::Cache::new(dir.path().join("cache")).unwrap();
        let src = thumbs::Source {
            path: &path,
            kind: Kind::Psd,
            size: 1,
            mtime: 1,
        };
        let thumb = cache.ensure(&src, thumbs::Size::Thumb).unwrap();
        assert!(thumb.is_file());
        // One decode made both renditions.
        assert!(cache.get(&src, thumbs::Size::Preview).is_some());
        let mut d =
            jpeg_decoder::Decoder::new(std::io::Cursor::new(std::fs::read(&thumb).unwrap()));
        d.read_info().unwrap();
        assert_eq!(
            (d.info().unwrap().width, d.info().unwrap().height),
            (512, 341)
        );
    }

    fn heic_fixture(name: &str) -> Vec<u8> {
        std::fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/heic")
                .join(name),
        )
        .unwrap()
    }

    /// Thumbnails and previews of `path`, decoded back to (width, height, pixels).
    fn renditions(path: &Path, kind: Kind) -> Vec<(u32, u32, Vec<u8>)> {
        let cache = thumbs::Cache::new(path.parent().unwrap().join("cache")).unwrap();
        let src = thumbs::Source {
            path,
            kind,
            size: 1,
            mtime: 1,
        };
        [thumbs::Size::Thumb, thumbs::Size::Preview]
            .into_iter()
            .map(|size| {
                let jpeg = std::fs::read(cache.ensure(&src, size).unwrap()).unwrap();
                let mut d = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg));
                let pixels = d.decode().unwrap();
                let info = d.info().unwrap();
                (info.width as u32, info.height as u32, pixels)
            })
            .collect()
    }

    #[test]
    fn heic_photos_get_true_colour_previews() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0001.HEIC");
        std::fs::write(&path, heic_fixture("rgb-strips-96.heic")).unwrap();
        let m = meta::read(&path, Kind::Heif).unwrap();
        assert_eq!((m.width, m.height, m.orientation), (96, 32, 1));

        let image = decode::image(&path, Kind::Heif).unwrap();
        assert_eq!((image.width, image.height), (96, 32));
        let at = |x: usize| &image.data[(16 * 96 + x) * 3..][..3];
        for (x, want) in [(16, [255, 0, 0]), (48, [0, 255, 0]), (80, [0, 0, 255])] {
            let got = at(x);
            assert!(
                got.iter()
                    .zip(want)
                    .all(|(&g, w): (&u8, i32)| (g as i32 - w).abs() < 24),
                "strip at x={x}: {got:?}"
            );
        }
        for (w, h, _) in renditions(&path, Kind::Heif) {
            assert_eq!((w, h), (96, 32));
        }
    }

    /// iPhones store a photo as a grid of 512px tiles; they must line up.
    #[test]
    fn tiled_heic_photos_are_assembled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("IMG_0002.HEIC");
        std::fs::write(&path, heic_fixture("checker-1024.heic")).unwrap();
        let [thumb, preview] = renditions(&path, Kind::Heif).try_into().unwrap();
        assert_eq!((thumb.0, thumb.1), (512, 512));
        assert_eq!((preview.0, preview.1), (1024, 1024));
        // 64px cells, top-left white, on both sides of the tile seams.
        let px = |x: usize, y: usize| preview.2[(y * 1024 + x) * 3];
        for (x, y, white) in [
            (32, 32, true),
            (96, 32, false),
            (544, 32, true),
            (32, 544, true),
            (608, 608, true),
            (544, 608, false),
        ] {
            assert_eq!(px(x, y) > 128, white, "cell at {x},{y}");
        }
    }

    /// Phones and transfer tools sometimes save HEIC or PNG pictures as .jpg.
    #[test]
    fn pictures_saved_under_the_wrong_name_still_show() {
        let dir = tempfile::tempdir().unwrap();
        let heic = dir.path().join("IMG_0003.jpg");
        std::fs::write(&heic, heic_fixture("checker-1024.heic")).unwrap();
        let m = meta::read(&heic, Kind::Jpeg).unwrap();
        assert_eq!((m.width, m.height), (1024, 1024));
        assert_eq!(renditions(&heic, Kind::Jpeg)[1].0, 1024);

        let png = dir.path().join("Screenshot.jpg");
        let mut bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut bytes, 40, 30);
            enc.set_color(png::ColorType::Rgba);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[200u8, 40, 90, 255].repeat(40 * 30))
                .unwrap();
        }
        std::fs::write(&png, bytes).unwrap();
        let m = meta::read(&png, Kind::Jpeg).unwrap();
        assert_eq!((m.width, m.height), (40, 30));
        let [thumb, _] = renditions(&png, Kind::Jpeg).try_into().unwrap();
        assert_eq!((thumb.0, thumb.1), (40, 30));
        assert!((thumb.2[0] as i32 - 200).abs() < 12);
    }

    #[test]
    fn iphone_live_photos_screenshots_and_proraw_are_indexed_and_shown() {
        let dir = tempfile::tempdir().unwrap();
        let roll = dir.path().join("iCloud Photos");
        std::fs::create_dir_all(&roll).unwrap();
        std::fs::write(
            roll.join("IMG_0005.HEIC"),
            heic_fixture("rgb-strips-96.heic"),
        )
        .unwrap();
        std::fs::write(roll.join("IMG_0005.MOV"), video::tests::iphone_clip()).unwrap();
        let mut screenshot = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut screenshot, 40, 30);
            enc.set_color(png::ColorType::Rgb);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&[30u8, 200, 90].repeat(40 * 30))
                .unwrap();
        }
        std::fs::write(roll.join("IMG_0007.PNG"), &screenshot).unwrap();
        // A ProRAW shot in portrait: its 64×48 preview is stored sideways.
        let mut preview = Vec::new();
        jpeg_encoder::Encoder::new(&mut preview, 90)
            .encode(
                &[220u8, 60, 40].repeat(64 * 48),
                64,
                48,
                jpeg_encoder::ColorType::Rgb,
            )
            .unwrap();
        std::fs::write(roll.join("IMG_0006.DNG"), dng::tests::sample(&preview, 6)).unwrap();

        let lib = Library::open_in_memory().unwrap();
        let source = lib.add_source(&roll).unwrap();
        let summary = scan::scan_source(&lib, &source, false, &|_| {}).unwrap();
        assert_eq!((summary.added, summary.failed), (4, 0));
        let photos = lib.photos().unwrap();
        let find = |name: &str| photos.iter().find(|p| p.name == name).unwrap();

        // The Live Photo's movie is stacked under its picture.
        let (still, movie) = (find("IMG_0005.HEIC"), find("IMG_0005.MOV"));
        assert_eq!(movie.kind, Kind::Video);
        assert_eq!(movie.pair_id, Some(still.id));
        assert_eq!(still.pair_id, Some(movie.id));
        assert_eq!(movie.duration, Some(1.5));
        assert_eq!(movie.camera.as_deref(), Some("iPhone 16 Pro"));

        let raw = find("IMG_0006.DNG");
        assert_eq!((raw.kind, raw.width, raw.height), (Kind::Dng, 3024, 4032));
        let [thumb, _] = renditions(&roll.join("IMG_0006.DNG"), Kind::Dng)
            .try_into()
            .unwrap();
        assert_eq!((thumb.0, thumb.1), (48, 64), "turned upright");
        assert!((thumb.2[0] as i32 - 220).abs() < 16);

        let shot = find("IMG_0007.PNG");
        assert_eq!((shot.kind, shot.width, shot.height), (Kind::Png, 40, 30));
        let [thumb, _] = renditions(&roll.join("IMG_0007.PNG"), Kind::Png)
            .try_into()
            .unwrap();
        assert_eq!((thumb.0, thumb.1), (40, 30));
        assert!((thumb.2[1] as i32 - 200).abs() < 12);
    }

    #[test]
    fn renders_upright_thumbnails_and_caches_them() {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = test_jpeg_with(1200, 800, Some("2026:10:21 09:15:30"), 6);
        let path = dir.path().join("DSCF0002.JPG");
        std::fs::write(&path, &jpeg).unwrap();

        let cache = thumbs::Cache::new(dir.path().join("cache")).unwrap();
        let src = thumbs::Source {
            path: &path,
            kind: Kind::Jpeg,
            size: jpeg.len() as u64,
            mtime: 1,
        };
        let thumb = cache.ensure(&src, thumbs::Size::Thumb).unwrap();
        let mut d =
            jpeg_decoder::Decoder::new(std::io::Cursor::new(std::fs::read(&thumb).unwrap()));
        d.read_info().unwrap();
        let info = d.info().unwrap();
        assert_eq!((info.width, info.height), (341, 512));
        assert_eq!(cache.get(&src, thumbs::Size::Thumb), Some(thumb));

        // A RAF goes through its embedded JPEG.
        let raf = dir.path().join("DSCF0002.RAF");
        std::fs::write(&raf, raf::wrap_jpeg_for_tests(&jpeg)).unwrap();
        let src = thumbs::Source {
            path: &raf,
            kind: Kind::Raf,
            size: 1,
            mtime: 1,
        };
        assert!(cache.ensure(&src, thumbs::Size::Preview).is_ok());
        assert!(cache.ensure(&src, thumbs::Size::Thumb).is_ok());
        assert!(cache.disk_usage() > 0);
    }
}
