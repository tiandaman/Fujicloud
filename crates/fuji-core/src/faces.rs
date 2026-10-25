//! Finding and recognising faces, entirely on this PC.
//!
//! Two small open models from OpenCV's model zoo: YuNet finds faces and the
//! five points of each (eyes, nose, mouth corners), and SFace turns a face,
//! turned and scaled to a standard 112×112 pose, into 128 numbers. Faces of
//! the same person land close together, so people can be grouped and named
//! (see [`crate::people`]).

use std::path::{Path, PathBuf};

use fast_image_resize::{self as fr, images::Image};
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;

use crate::Error;
use crate::decode::Rgb;
use crate::embed::{download_files, environment, model_err};

/// Stored with each face, so a future model can tell its own faces apart.
pub const MODEL_ID: &str = "yunet-2023mar+sface-2021dec";

const DETECTOR: &str = "face_detection_yunet_2023mar.onnx";
const RECOGNISER: &str = "face_recognition_sface_2021dec.onnx";
const BASE_URL: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/";
/// Both models together, for the UI.
pub const DOWNLOAD_MB: u32 = 39;

/// YuNet's fixed input size.
const SIDE: usize = 640;
/// Detections less sure than this are dropped.
const MIN_SCORE: f32 = 0.8;
/// Faces smaller than this (pixels across, in the image searched) are too
/// blurry to recognise.
const MIN_FACE: f32 = 28.0;
const MAX_FACES: usize = 40;

/// Where SFace expects the eyes, nose and mouth corners (112×112).
const TEMPLATE: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// Box as fractions of the image: x, y, width, height.
    pub bbox: [f32; 4],
    pub score: f32,
    /// L2-normalised, so similarity is a dot product.
    pub embedding: Vec<f32>,
}

/// A face found by the detector, in pixels.
#[derive(Debug, Clone, Copy)]
struct Detection {
    bbox: [f32; 4],
    points: [[f32; 2]; 5],
    score: f32,
}

fn dir_for(dir: &Path) -> PathBuf {
    dir.join("faces")
}

pub fn is_downloaded(dir: &Path) -> bool {
    [DETECTOR, RECOGNISER]
        .iter()
        .all(|f| dir_for(dir).join(f).is_file())
}

/// Fetches both models into `dir` (~39 MB). `progress(done, total)` in bytes.
pub fn download(dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), Error> {
    let urls = [
        format!("{BASE_URL}face_detection_yunet/{DETECTOR}"),
        format!("{BASE_URL}face_recognition_sface/{RECOGNISER}"),
    ];
    let files: Vec<(&str, PathBuf)> = urls
        .iter()
        .zip([DETECTOR, RECOGNISER])
        .map(|(url, name)| (url.as_str(), dir_for(dir).join(name)))
        .collect();
    download_files(&files, progress)
}

pub struct FaceModels {
    detector: Mutex<Session>,
    recogniser: Mutex<Session>,
}

impl FaceModels {
    /// Loads the downloaded models (CPU; they're small and quick).
    pub fn load(dir: &Path, threads: usize) -> Result<Self, Error> {
        let env = environment()?;
        let session = |name: &str| -> Result<Session, Error> {
            let path = dir_for(dir).join(name);
            let fail = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", path.display()));
            let builder = Session::builder(&env).map_err(|e| fail(&e))?;
            let mut builder = builder
                .with_intra_threads(threads.max(1))
                .map_err(|e| fail(&e))?;
            builder.commit_from_file(&path).map_err(|e| fail(&e))
        };
        Ok(Self {
            detector: Mutex::new(session(DETECTOR)?),
            recogniser: Mutex::new(session(RECOGNISER)?),
        })
    }

    /// Every recognisable face in an upright sRGB image.
    pub fn faces(&self, image: &Rgb) -> Result<Vec<Face>, Error> {
        let (w, h) = (image.width as f32, image.height as f32);
        self.detect(image)?
            .into_iter()
            .filter(|d| d.bbox[2].min(d.bbox[3]) >= MIN_FACE)
            .map(|d| {
                let embedding = self.embed(&align(image, &d.points))?;
                let [x, y, bw, bh] = d.bbox;
                Ok(Face {
                    bbox: [
                        (x / w).clamp(0.0, 1.0),
                        (y / h).clamp(0.0, 1.0),
                        (bw / w).min(1.0),
                        (bh / h).min(1.0),
                    ],
                    score: d.score,
                    embedding,
                })
            })
            .collect()
    }

    fn detect(&self, image: &Rgb) -> Result<Vec<Detection>, Error> {
        let scale = SIDE as f32 / image.width.max(image.height) as f32;
        let (nw, nh) = (
            ((image.width as f32 * scale).round() as u32).clamp(1, SIDE as u32),
            ((image.height as f32 * scale).round() as u32).clamp(1, SIDE as u32),
        );
        let src = Image::from_vec_u8(
            image.width,
            image.height,
            image.data.clone(),
            fr::PixelType::U8x3,
        )
        .map_err(model_err)?;
        let mut small = Image::new(nw, nh, fr::PixelType::U8x3);
        fr::Resizer::new()
            .resize(
                &src,
                &mut small,
                &fr::ResizeOptions::new()
                    .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::Bilinear)),
            )
            .map_err(model_err)?;

        // Top-left of a black 640×640 square; BGR planes, 0–255.
        let plane = SIDE * SIDE;
        let mut input = vec![0f32; 3 * plane];
        for (i, px) in small.buffer().as_chunks::<3>().0.iter().enumerate() {
            let (x, y) = (i % nw as usize, i / nw as usize);
            let at = y * SIDE + x;
            input[at] = px[2] as f32;
            input[plane + at] = px[1] as f32;
            input[2 * plane + at] = px[0] as f32;
        }
        let tensor = Tensor::from_array(([1usize, 3, SIDE, SIDE], input)).map_err(model_err)?;
        let mut session = self.detector.lock();
        let outputs = session
            .run(ort::inputs!["input" => tensor])
            .map_err(model_err)?;
        let get = |name: &str| -> Result<Vec<f32>, Error> {
            let (_, data) = outputs[name]
                .try_extract_tensor::<f32>()
                .map_err(model_err)?;
            Ok(data.to_vec())
        };

        let mut found = Vec::new();
        for stride in [8usize, 16, 32] {
            let (cls, obj) = (
                get(&format!("cls_{stride}"))?,
                get(&format!("obj_{stride}"))?,
            );
            let (bbox, kps) = (
                get(&format!("bbox_{stride}"))?,
                get(&format!("kps_{stride}"))?,
            );
            let cols = SIDE / stride;
            for (i, (c, o)) in cls.iter().zip(&obj).enumerate() {
                let score = (c.clamp(0.0, 1.0) * o.clamp(0.0, 1.0)).sqrt();
                if score < MIN_SCORE {
                    continue;
                }
                let (col, row) = ((i % cols) as f32, (i / cols) as f32);
                let s = stride as f32;
                let b = &bbox[i * 4..i * 4 + 4];
                let (cx, cy) = ((col + b[0]) * s, (row + b[1]) * s);
                let (bw, bh) = (b[2].exp() * s, b[3].exp() * s);
                let k = &kps[i * 10..i * 10 + 10];
                let points = std::array::from_fn(|n| {
                    [
                        (k[2 * n] + col) * s / scale,
                        (k[2 * n + 1] + row) * s / scale,
                    ]
                });
                found.push(Detection {
                    bbox: [
                        (cx - bw / 2.0) / scale,
                        (cy - bh / 2.0) / scale,
                        bw / scale,
                        bh / scale,
                    ],
                    points,
                    score,
                });
            }
        }
        Ok(suppress(found))
    }

    fn embed(&self, face: &[u8]) -> Result<Vec<f32>, Error> {
        // RGB planes, 0–255.
        let plane = 112 * 112;
        let mut input = vec![0f32; 3 * plane];
        for (i, px) in face.as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                input[c * plane + i] = px[c] as f32;
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, 112, 112], input)).map_err(model_err)?;
        let mut session = self.recogniser.lock();
        let outputs = session
            .run(ort::inputs!["data" => tensor])
            .map_err(model_err)?;
        let (_, v) = outputs[0].try_extract_tensor::<f32>().map_err(model_err)?;
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        Ok(v.iter().map(|x| x / norm).collect())
    }
}

/// Keeps the best of overlapping detections.
fn suppress(mut found: Vec<Detection>) -> Vec<Detection> {
    found.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut kept: Vec<Detection> = Vec::new();
    for d in found {
        if kept.len() < MAX_FACES && kept.iter().all(|k| iou(&k.bbox, &d.bbox) < 0.3) {
            kept.push(d);
        }
    }
    kept
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a[2] * a[3] + b[2] * b[3] - inter).max(1e-6)
}

/// The rotation, scale and shift that best moves `from` onto `to` (least
/// squares, no mirroring): `[a, b, tx, ty]` for x' = a·x − b·y + tx,
/// y' = b·x + a·y + ty.
fn similarity(from: &[[f32; 2]; 5], to: &[[f32; 2]; 5]) -> [f32; 4] {
    let mean = |p: &[[f32; 2]; 5]| {
        let s = p.iter().fold([0.0, 0.0], |m, q| [m[0] + q[0], m[1] + q[1]]);
        [s[0] / 5.0, s[1] / 5.0]
    };
    let (ms, md) = (mean(from), mean(to));
    let (mut dot, mut cross, mut norm) = (0.0, 0.0, 0.0);
    for (s, d) in from.iter().zip(to) {
        let (sx, sy) = (s[0] - ms[0], s[1] - ms[1]);
        let (dx, dy) = (d[0] - md[0], d[1] - md[1]);
        dot += sx * dx + sy * dy;
        cross += sx * dy - sy * dx;
        norm += sx * sx + sy * sy;
    }
    let (a, b) = (dot / norm.max(1e-6), cross / norm.max(1e-6));
    [
        a,
        b,
        md[0] - (a * ms[0] - b * ms[1]),
        md[1] - (b * ms[0] + a * ms[1]),
    ]
}

/// The face turned and scaled into SFace's 112×112 pose (RGB).
fn align(image: &Rgb, points: &[[f32; 2]; 5]) -> Vec<u8> {
    let [a, b, tx, ty] = similarity(points, &TEMPLATE);
    let det = (a * a + b * b).max(1e-12);
    let (w, h) = (image.width as i64, image.height as i64);
    let pixel = |x: i64, y: i64, c: usize| -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            image.data[((y * w + x) * 3) as usize + c] as f32
        }
    };
    let mut out = vec![0u8; 112 * 112 * 3];
    for v in 0..112 {
        for u in 0..112 {
            // Back from the template to the photo.
            let (dx, dy) = (u as f32 - tx, v as f32 - ty);
            let x = (a * dx + b * dy) / det;
            let y = (-b * dx + a * dy) / det;
            let (x0, y0) = (x.floor() as i64, y.floor() as i64);
            let (fx, fy) = (x - x0 as f32, y - y0 as f32);
            for c in 0..3 {
                let top = pixel(x0, y0, c) * (1.0 - fx) + pixel(x0 + 1, y0, c) * fx;
                let bottom = pixel(x0, y0 + 1, c) * (1.0 - fx) + pixel(x0 + 1, y0 + 1, c) * fx;
                out[(v * 112 + u) * 3 + c] = (top * (1.0 - fy) + bottom * fy).round() as u8;
            }
        }
    }
    out
}

/// Cosine similarity of two embeddings.
pub fn similarity_of(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alignment_maps_the_landmarks_onto_the_template() {
        // The template itself, turned 30°, doubled in size and moved.
        let (s, c) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        let moved =
            TEMPLATE.map(|[x, y]| [2.0 * (c * x - s * y) + 40.0, 2.0 * (s * x + c * y) + 10.0]);
        let [a, b, tx, ty] = similarity(&moved, &TEMPLATE);
        for (m, t) in moved.iter().zip(&TEMPLATE) {
            let x = a * m[0] - b * m[1] + tx;
            let y = b * m[0] + a * m[1] + ty;
            assert!(
                (x - t[0]).abs() < 1e-3 && (y - t[1]).abs() < 1e-3,
                "{x},{y} vs {t:?}"
            );
        }
    }

    #[test]
    fn overlapping_detections_keep_the_surest() {
        let d = |x: f32, score: f32| Detection {
            bbox: [x, 0.0, 100.0, 100.0],
            points: [[0.0; 2]; 5],
            score,
        };
        let kept = suppress(vec![d(0.0, 0.85), d(10.0, 0.95), d(300.0, 0.9)]);
        let xs: Vec<f32> = kept.iter().map(|k| k.bbox[0]).collect();
        assert_eq!(xs, [10.0, 300.0]);
    }

    /// The real models on real photos (downloaded; run with `--ignored`).
    /// `FUJICLOUD_FACE_TEST` names a folder with obama.jpg, obama2.jpg,
    /// biden.jpg and two_people.jpg from the face_recognition project's
    /// examples, or they're downloaded there.
    #[test]
    #[ignore]
    fn real_faces_are_found_and_told_apart() {
        let dir = PathBuf::from(
            std::env::var("FUJICLOUD_MODEL_DIR").unwrap_or_else(|_| ".models".into()),
        );
        download(&dir, &|_, _| {}).expect("download models");
        let models = FaceModels::load(&dir, 4).unwrap();
        let photos = PathBuf::from(
            std::env::var("FUJICLOUD_FACE_TEST").unwrap_or_else(|_| ".models/face-test".into()),
        );
        let base = "https://raw.githubusercontent.com/ageitgey/face_recognition/master/examples/";
        let files: Vec<(String, PathBuf)> =
            ["obama.jpg", "obama2.jpg", "biden.jpg", "two_people.jpg"]
                .iter()
                .map(|f| (format!("{base}{f}"), photos.join(f)))
                .collect();
        let refs: Vec<(&str, PathBuf)> =
            files.iter().map(|(u, p)| (u.as_str(), p.clone())).collect();
        download_files(&refs, &|_, _| {}).expect("download photos");

        let faces_in = |name: &str| {
            let jpeg = std::fs::read(photos.join(name)).unwrap();
            let image = crate::thumbs::upright(&jpeg, 1600).unwrap();
            models.faces(&image).unwrap()
        };
        let (a, b, c) = (
            faces_in("obama.jpg"),
            faces_in("obama2.jpg"),
            faces_in("biden.jpg"),
        );
        let both = faces_in("two_people.jpg");
        println!(
            "faces: {} {} {} {}; boxes {:?}",
            a.len(),
            b.len(),
            c.len(),
            both.len(),
            both.iter().map(|f| f.bbox).collect::<Vec<_>>()
        );
        assert_eq!((a.len(), b.len(), c.len(), both.len()), (1, 1, 1, 2));
        let same = similarity_of(&a[0].embedding, &b[0].embedding);
        let other = similarity_of(&a[0].embedding, &c[0].embedding);
        // In the two-person photo, each face matches its own portrait best.
        let cross: Vec<[f32; 2]> = both
            .iter()
            .map(|f| {
                [
                    similarity_of(&f.embedding, &a[0].embedding),
                    similarity_of(&f.embedding, &c[0].embedding),
                ]
            })
            .collect();
        println!(
            "same person {same:.3}, different {other:.3}, two people vs (obama, biden) {cross:?}"
        );
        assert!(
            same >= crate::people::SAME_PERSON,
            "same person scored {same}"
        );
        assert!(
            other < crate::people::SAME_PERSON,
            "different people scored {other}"
        );
        assert!(cross.iter().any(|s| s[0] > s[1]) && cross.iter().any(|s| s[1] > s[0]));

        // From cached thumbnails (512 px), as most of a library is indexed.
        let small = |name: &str| {
            let jpeg = std::fs::read(photos.join(name)).unwrap();
            models
                .faces(&crate::thumbs::upright(&jpeg, 512).unwrap())
                .unwrap()
        };
        let (a, b, c) = (small("obama.jpg"), small("obama2.jpg"), small("biden.jpg"));
        let same = similarity_of(&a[0].embedding, &b[0].embedding);
        let other = similarity_of(&a[0].embedding, &c[0].embedding);
        println!("from thumbnails: same person {same:.3}, different {other:.3}");
        assert!(same >= crate::people::SAME_PERSON && other < crate::people::SAME_PERSON);
    }
}
