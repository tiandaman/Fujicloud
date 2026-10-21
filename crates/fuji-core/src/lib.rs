//! Core of Fujicloud: everything that touches files lives here, independent
//! of the desktop shell, so it can be tested and benchmarked on its own.

pub mod db;
pub mod fuji;
pub mod ingest;
pub mod meta;
pub mod raf;
pub mod scan;
pub mod thumbs;
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
}

impl Kind {
    pub fn from_path(path: &Path) -> Option<Kind> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "jpg" | "jpeg" => Kind::Jpeg,
            "raf" => Kind::Raf,
            "hif" | "heif" | "heic" => Kind::Heif,
            "tif" | "tiff" => Kind::Tiff,
            _ => return None,
        })
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "jpeg" => Kind::Jpeg,
            "raf" => Kind::Raf,
            "heif" => Kind::Heif,
            "tiff" => Kind::Tiff,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Jpeg => "jpeg",
            Kind::Raf => "raf",
            Kind::Heif => "heif",
            Kind::Tiff => "tiff",
        }
    }

    pub fn is_raw(self) -> bool {
        self == Kind::Raf
    }

    /// Whether we can build thumbnails and previews for this kind.
    pub fn has_jpeg_preview(self) -> bool {
        matches!(self, Kind::Jpeg | Kind::Raf)
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
    use exif::{Field, In, Rational, Tag, Value};

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
    if let Some(taken) = taken {
        let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
        let makernote = fuji::build_makernote_for_tests(&[(0x1003, 0), (0x1401, 0x800)]);
        let fields = [
            Field {
                tag: Tag::Make,
                ifd_num: In::PRIMARY,
                value: ascii("FUJIFILM"),
            },
            Field {
                tag: Tag::Model,
                ifd_num: In::PRIMARY,
                value: ascii("X-T5"),
            },
            Field {
                tag: Tag::Orientation,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![orientation]),
            },
            Field {
                tag: Tag::DateTimeOriginal,
                ifd_num: In::PRIMARY,
                value: ascii(taken),
            },
            Field {
                tag: Tag::LensModel,
                ifd_num: In::PRIMARY,
                value: ascii("XF23mmF1.4 R LM WR"),
            },
            Field {
                tag: Tag::FNumber,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![Rational { num: 28, denom: 10 }]),
            },
            Field {
                tag: Tag::ExposureTime,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![Rational { num: 1, denom: 250 }]),
            },
            Field {
                tag: Tag::FocalLength,
                ifd_num: In::PRIMARY,
                value: Value::Rational(vec![Rational { num: 23, denom: 1 }]),
            },
            Field {
                tag: Tag::PhotographicSensitivity,
                ifd_num: In::PRIMARY,
                value: Value::Short(vec![160]),
            },
            Field {
                tag: Tag::MakerNote,
                ifd_num: In::PRIMARY,
                value: Value::Undefined(makernote, 0),
            },
        ];
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

#[cfg(test)]
mod tests {
    use super::*;

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
