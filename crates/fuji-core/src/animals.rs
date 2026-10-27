//! Finding cats and dogs in photos, entirely on this PC.
//!
//! YOLOX-S (8-bit, from OpenCV's model zoo) finds animals and boxes them.
//! DINOv2 (small, Meta's), a model that sees fine detail, describes each
//! pet's picture: coat colour, pattern, markings and shape. Measured on
//! Oxford-IIIT Pet it tells cats apart with a much wider margin than the
//! smart search model, and it needs nothing else switched on. Pictures are
//! compared with the average pet taken away ([`distinctive`]); see
//! [`crate::people`], which groups and names pets the way it does people.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use fast_image_resize::{self as fr, images::Image};
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;
use serde::Serialize;

use crate::Error;
use crate::decode::Rgb;
use crate::embed::{download_files, environment, model_err};

const DETECTOR: &str = "object_detection_yolox_2022nov_int8.onnx";
const URL: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/object_detection_yolox/object_detection_yolox_2022nov_int8.onnx";
/// The detector (9 MB) and DINOv2 (88 MB) together, for the UI.
pub const DOWNLOAD_MB: u32 = 97;
/// Stored with each photo searched: the detector, and the model that tells
/// pets apart.
pub const MODEL_ID: &str = "yolox-2022nov-int8+dinov2-small-box+coat";

const SIDE: usize = 640;
/// Detections less sure than this are dropped.
const MIN_SCORE: f32 = 0.45;
/// Pets smaller than this (pixels across, in the image searched) are too
/// small to tell apart.
const MIN_SIZE: f32 = 48.0;
/// COCO class numbers.
const CAT: usize = 15;
const DOG: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Species {
    Cat,
    Dog,
}

impl Species {
    pub fn as_str(self) -> &'static str {
        match self {
            Species::Cat => "cat",
            Species::Dog => "dog",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Animal {
    pub species: Species,
    /// Box as fractions of the image: x, y, width, height.
    pub bbox: [f32; 4],
    pub score: f32,
}

fn path(dir: &Path) -> PathBuf {
    dir.join("pets").join(DETECTOR)
}

pub fn is_downloaded(dir: &Path) -> bool {
    path(dir).is_file() && dir.join("pets").join(IDENTIFIER).is_file()
}

/// Fetches the detector and DINOv2 (~97 MB). `progress(done, total)` in bytes.
pub fn download(dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), Error> {
    download_files(
        &[
            (URL, path(dir)),
            (IDENTIFIER_URL, dir.join("pets").join(IDENTIFIER)),
        ],
        progress,
    )
}

pub struct Detector {
    session: Mutex<Session>,
}

impl Detector {
    pub fn load(dir: &Path, threads: usize) -> Result<Self, Error> {
        let env = environment()?;
        let file = path(dir);
        let fail = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", file.display()));
        let builder = Session::builder(&env).map_err(|e| fail(&e))?;
        let mut builder = builder
            .with_intra_threads(threads.max(1))
            .map_err(|e| fail(&e))?;
        Ok(Self {
            session: Mutex::new(builder.commit_from_file(&file).map_err(|e| fail(&e))?),
        })
    }

    /// Every cat and dog in an upright sRGB image, surest first.
    pub fn animals(&self, image: &Rgb) -> Result<Vec<Animal>, Error> {
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

        // Top-left of a grey (114) 640×640 square; BGR planes, 0–255.
        let plane = SIDE * SIDE;
        let mut input = vec![114f32; 3 * plane];
        for (i, px) in small.buffer().as_chunks::<3>().0.iter().enumerate() {
            let at = (i / nw as usize) * SIDE + i % nw as usize;
            input[at] = px[2] as f32;
            input[plane + at] = px[1] as f32;
            input[2 * plane + at] = px[0] as f32;
        }
        let tensor = Tensor::from_array(([1usize, 3, SIDE, SIDE], input)).map_err(model_err)?;
        let mut session = self.session.lock();
        let outputs = session
            .run(ort::inputs!["images" => tensor])
            .map_err(model_err)?;
        let (_, out) = outputs[0].try_extract_tensor::<f32>().map_err(model_err)?;

        let mut found = Vec::new();
        let mut row = 0;
        for stride in [8usize, 16, 32] {
            let cells = SIDE / stride;
            for i in 0..cells * cells {
                let d = &out[row * 85..row * 85 + 85];
                row += 1;
                let objectness = d[4];
                let (species, class) = if d[5 + CAT] >= d[5 + DOG] {
                    (Species::Cat, d[5 + CAT])
                } else {
                    (Species::Dog, d[5 + DOG])
                };
                let score = objectness * class;
                if score < MIN_SCORE {
                    continue;
                }
                let s = stride as f32;
                let (cx, cy) = (
                    (d[0] + (i % cells) as f32) * s,
                    (d[1] + (i / cells) as f32) * s,
                );
                let (w, h) = (d[2].exp() * s, d[3].exp() * s);
                if w.min(h) < MIN_SIZE * scale {
                    continue;
                }
                found.push((
                    species,
                    [
                        (cx - w / 2.0) / scale,
                        (cy - h / 2.0) / scale,
                        w / scale,
                        h / scale,
                    ],
                    score,
                ));
            }
        }
        // Keep the surest of overlapping boxes.
        found.sort_by(|a, b| b.2.total_cmp(&a.2));
        let mut kept: Vec<(Species, [f32; 4], f32)> = Vec::new();
        for f in found {
            if kept.len() < 20 && kept.iter().all(|k| iou(&k.1, &f.1) < 0.45) {
                kept.push(f);
            }
        }
        let (w, h) = (image.width as f32, image.height as f32);
        Ok(kept
            .into_iter()
            .map(|(species, [x, y, bw, bh], score)| {
                let (x0, y0) = (x.max(0.0), y.max(0.0));
                let (x1, y1) = ((x + bw).min(w), (y + bh).min(h));
                Animal {
                    species,
                    bbox: [
                        x0 / w,
                        y0 / h,
                        (x1 - x0).max(0.0) / w,
                        (y1 - y0).max(0.0) / h,
                    ],
                    score,
                }
            })
            .collect())
    }
}

fn iou(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a[2] * a[3] + b[2] * b[3] - inter).max(1e-6)
}

/// What pets have in common as DINOv2 sees them: the average pet (as half
/// floats), learned from held-out Oxford-IIIT Pet photos by
/// `real_pets_are_told_apart`, which checks it.
const BASIS: &[u8] = include_bytes!("pet_mean.bin");

/// The average pet: DINOv2's description, then its coat's colours.
fn basis() -> &'static Vec<f32> {
    static BASIS_F32: OnceLock<Vec<f32>> = OnceLock::new();
    BASIS_F32.get_or_init(|| {
        BASIS
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| half::f16::from_le_bytes(*b).to_f32())
            .collect()
    })
}

/// How much the coat's colours count against DINOv2's view of the pet.
/// Measured on Oxford-IIIT Pet: a fifth tells look-alike cats apart better
/// (warm, spotted breeds: groups 83% pure rather than 67%; all breeds 75%
/// rather than 67%), more makes it worse.
const COAT_SHARE: f32 = 0.2;

/// A pet's description (from [`Identifier::pet`]) for comparing one pet
/// with another: each part with the average pet taken away, the coat
/// counting for [`COAT_SHARE`].
pub fn distinctive(v: &[f32]) -> Vec<f32> {
    let mean = basis();
    if v.len() != mean.len() || v.len() <= COAT_BINS {
        return v.to_vec();
    }
    let split = v.len() - COAT_BINS;
    let part = |range: std::ops::Range<usize>, weight: f32| {
        let d: Vec<f32> = v[range.clone()]
            .iter()
            .zip(&mean[range])
            .map(|(x, m)| x - m)
            .collect();
        let n = d.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        d.into_iter().map(move |x| x / n * weight)
    };
    part(0..split, (1.0 - COAT_SHARE).sqrt())
        .chain(part(split..v.len(), COAT_SHARE.sqrt()))
        .collect()
}

/// DINOv2 (small), which tells one pet from another.
const IDENTIFIER: &str = "dinov2_small.onnx";
const IDENTIFIER_URL: &str =
    "https://huggingface.co/Xenova/dinov2-small/resolve/main/onnx/model.onnx";

pub struct Identifier {
    session: Mutex<Session>,
}

impl Identifier {
    pub fn load(dir: &Path, threads: usize) -> Result<Self, Error> {
        Self::load_file(&dir.join("pets").join(IDENTIFIER), threads)
    }

    /// A DINOv2 export of any size (the tests compare them).
    pub fn load_file(file: &Path, threads: usize) -> Result<Self, Error> {
        let env = environment()?;
        let fail = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", file.display()));
        let builder = Session::builder(&env).map_err(|e| fail(&e))?;
        let mut builder = builder
            .with_intra_threads(threads.max(1))
            .map_err(|e| fail(&e))?;
        Ok(Self {
            session: Mutex::new(builder.commit_from_file(file).map_err(|e| fail(&e))?),
        })
    }

    /// A pet described, as stored: DINOv2's view of just the pet (its class
    /// token, L2-normalised), then its coat's colours ([`coat`]).
    pub fn pet(&self, image: &Rgb, animal: &Animal) -> Result<Vec<f32>, Error> {
        let mut v = self.describe(&crop(image, animal)?)?;
        v.extend(coat(image, animal.bbox));
        Ok(v)
    }

    /// A pet's picture (as [`crop`] makes it) described: DINOv2's class
    /// token, L2-normalised.
    pub fn describe(&self, jpeg: &[u8]) -> Result<Vec<f32>, Error> {
        let (tokens, dim) = self.tokens(jpeg)?;
        let cls = tokens
            .get(..dim)
            .ok_or_else(|| model_err("no class token"))?;
        let n = cls.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        Ok(cls.iter().map(|x| x / n).collect())
    }

    /// DINOv2's tokens for a pet's picture (as [`crop`] makes it): the class
    /// token, then one per 14-pixel patch, each `dim` long.
    pub fn tokens(&self, jpeg: &[u8]) -> Result<(Vec<f32>, usize), Error> {
        const SIZE: u32 = 224;
        const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
        const STD: [f32; 3] = [0.229, 0.224, 0.225];
        let rgb = crate::thumbs::decode_rendition(jpeg, SIZE)?;
        let src = Image::from_vec_u8(rgb.width, rgb.height, rgb.data, fr::PixelType::U8x3)
            .map_err(model_err)?;
        let mut dst = Image::new(SIZE, SIZE, fr::PixelType::U8x3);
        fr::Resizer::new()
            .resize(
                &src,
                &mut dst,
                &fr::ResizeOptions::new()
                    .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom)),
            )
            .map_err(model_err)?;
        let plane = (SIZE * SIZE) as usize;
        let mut input = vec![0f32; 3 * plane];
        for (i, px) in dst.buffer().as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                input[c * plane + i] = (px[c] as f32 / 255.0 - MEAN[c]) / STD[c];
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, SIZE as usize, SIZE as usize], input))
            .map_err(model_err)?;
        let mut session = self.session.lock();
        let name = session.inputs()[0].name().to_owned();
        let outputs = session
            .run(ort::inputs![name => tensor])
            .map_err(model_err)?;
        let hidden = outputs.get("last_hidden_state").unwrap_or(&outputs[0]);
        let (shape, data) = hidden.try_extract_tensor::<f32>().map_err(model_err)?;
        let dim = *shape.last().unwrap_or(&1) as usize;
        Ok((data.to_vec(), dim))
    }
}

/// The picture of a pet DINOv2 describes: just what's in its box, the rest
/// neutral grey, so a cat lying against another, or the bed they share,
/// doesn't count towards who it is.
pub fn crop(image: &Rgb, animal: &Animal) -> Result<Vec<u8>, Error> {
    crate::thumbs::crop_box(image, animal.bbox, 224)
}

/// Bins of [`coat`]: twelve hues, then black, grey and white.
pub const COAT_BINS: usize = 15;

/// A pet's coat colours, from the middle of its box (its edges are mostly
/// background): how much of it is each of twelve hues, black, grey and
/// white, as a unit vector of square roots (so cosine compares shares).
/// Orange all over and orange-and-white differ here far more than to a
/// model that mostly sees shape and fur.
pub fn coat(image: &Rgb, bbox: [f32; 4]) -> Vec<f32> {
    let (w, h) = (image.width as f32, image.height as f32);
    let [x, y, bw, bh] = bbox;
    let x0 = ((x + bw * 0.15) * w).clamp(0.0, w - 1.0) as u32;
    let x1 = ((x + bw * 0.85) * w).clamp(x0 as f32 + 1.0, w) as u32;
    let y0 = ((y + bh * 0.15) * h).clamp(0.0, h - 1.0) as u32;
    let y1 = ((y + bh * 0.85) * h).clamp(y0 as f32 + 1.0, h) as u32;
    // About 10,000 samples, however big the photo.
    let step = (((x1 - x0) as f32 * (y1 - y0) as f32 / 10_000.0).sqrt() as u32).max(1);
    let mut bins = [0f32; COAT_BINS];
    for py in (y0..y1).step_by(step as usize) {
        for px in (x0..x1).step_by(step as usize) {
            let i = (py as usize * image.width as usize + px as usize) * 3;
            let Some(p) = image.data.get(i..i + 3) else {
                continue;
            };
            let [r, g, b] = [p[0], p[1], p[2]].map(|c| c as f32 / 255.0);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
            let bin = if max < 0.2 {
                12
            } else if sat < 0.2 {
                if max > 0.7 { 14 } else { 13 }
            } else {
                let d = max - min;
                let hue = if max == r {
                    ((g - b) / d).rem_euclid(6.0)
                } else if max == g {
                    (b - r) / d + 2.0
                } else {
                    (r - g) / d + 4.0
                } * 60.0;
                ((hue / 30.0) as usize).min(11)
            };
            bins[bin] += 1.0;
        }
    }
    let total: f32 = bins.iter().sum::<f32>().max(1.0);
    let v: Vec<f32> = bins.iter().map(|c| (c / total).sqrt()).collect();
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
    v.iter().map(|x| x / n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photos() -> PathBuf {
        PathBuf::from(
            std::env::var("FUJICLOUD_PET_TEST").unwrap_or_else(|_| ".models/pet-test".into()),
        )
    }

    /// ImageNet sample photos of five cats and two dogs.
    pub(crate) const SAMPLES: [&str; 7] = [
        "n02123045_tabby.JPEG",
        "n02123159_tiger_cat.JPEG",
        "n02123394_Persian_cat.JPEG",
        "n02123597_Siamese_cat.JPEG",
        "n02124075_Egyptian_cat.JPEG",
        "n02099601_golden_retriever.JPEG",
        "n02085620_Chihuahua.JPEG",
    ];

    pub(crate) fn fetch_samples() -> PathBuf {
        let dir = photos();
        let base = "https://raw.githubusercontent.com/EliSchwartz/imagenet-sample-images/master/";
        let urls: Vec<String> = SAMPLES.iter().map(|f| format!("{base}{f}")).collect();
        let files: Vec<(&str, PathBuf)> = urls
            .iter()
            .zip(SAMPLES)
            .map(|(u, f)| (u.as_str(), dir.join(f)))
            .collect();
        download_files(&files, &|_, _| {}).expect("download photos");
        dir
    }

    /// A cat painted `main`, with `white` of it white (its chest and paws).
    fn painted(main: [u8; 3], white: f32) -> Rgb {
        let (width, height) = (200u32, 160u32);
        let data = (0..width * height)
            .flat_map(|i| {
                let (x, y) = (i % width, i / width);
                if (y as f32) > height as f32 * (1.0 - white) && x > 40 && x < 160 {
                    [245, 243, 240]
                } else {
                    main
                }
            })
            .collect();
        Rgb {
            data,
            width,
            height,
        }
    }

    #[test]
    fn orange_and_white_is_told_from_all_orange() {
        let (whole, cosine) = ([0.0, 0.0, 1.0, 1.0], |a: &[f32], b: &[f32]| -> f32 {
            a.iter().zip(b).map(|(x, y)| x * y).sum()
        });
        let orange = coat(&painted([214, 128, 52], 0.0), whole);
        let darker = coat(&painted([190, 110, 45], 0.0), whole);
        let patched = coat(&painted([214, 128, 52], 0.45), whole);
        assert_eq!(orange.len(), COAT_BINS);
        assert!(
            cosine(&orange, &darker) > 0.95,
            "one orange cat, two lights"
        );
        assert!(
            cosine(&orange, &patched) < 0.85,
            "{}",
            cosine(&orange, &patched)
        );
    }

    #[test]
    fn a_box_is_cropped_alone() {
        let image = painted([214, 128, 52], 0.0);
        let jpeg = crate::thumbs::crop_box(&image, [0.5, 0.25, 0.5, 0.25], 64).unwrap();
        let (pixels, w, h) = crate::thumbs::decode_scaled(&jpeg, 64).unwrap();
        assert_eq!((w, h), (64, 64));
        // Wider than tall: grey above and below, the cat in the middle.
        let at = |x: usize, y: usize| &pixels[(y * 64 + x) * 3..(y * 64 + x) * 3 + 3];
        assert!(
            at(32, 2)
                .iter()
                .zip([124, 116, 104])
                .all(|(a, b)| a.abs_diff(b) < 12)
        );
        assert!(at(32, 32)[0] > 180 && at(32, 32)[2] < 90);
        assert!(crate::thumbs::crop_box(&image, [1.0, 1.0, 0.0, 0.0], 32).is_ok());
    }

    #[test]
    fn what_pets_share_is_taken_away() {
        assert_eq!(basis().len(), 384 + COAT_BINS);
        let v: Vec<f32> = (0..384 + COAT_BINS)
            .map(|i| ((i * 37) % 101) as f32 / 100.0 - 0.5)
            .collect();
        let d = distinctive(&v);
        let length = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((length(&d) - 1.0).abs() < 1e-3);
        // DINOv2's part and the coat's, weighed as set.
        assert!((length(&d[..384]).powi(2) - (1.0 - COAT_SHARE)).abs() < 1e-3);
        assert!((length(&d[384..]).powi(2) - COAT_SHARE).abs() < 1e-3);
        // Descriptions of another size (another model) are left as they are.
        assert_eq!(distinctive(&[1.0, 0.0]), [1.0, 0.0]);
    }

    /// The real detector on real photos (run with `--ignored`).
    #[test]
    #[ignore]
    fn real_cats_and_dogs_are_found() {
        let models = PathBuf::from(
            std::env::var("FUJICLOUD_MODEL_DIR").unwrap_or_else(|_| ".models".into()),
        );
        download(&models, &|_, _| {}).expect("download model");
        let detector = Detector::load(&models, 4).unwrap();
        let dir = fetch_samples();
        for name in SAMPLES {
            let jpeg = std::fs::read(dir.join(name)).unwrap();
            let image = crate::thumbs::upright(&jpeg, 1280).unwrap();
            let found = detector.animals(&image).unwrap();
            println!("{name}: {found:?}");
            // Cat or dog can be confused (the Siamese here passes for a dog);
            // what matters is that the pet is found and boxed.
            assert!(
                found.first().is_some_and(|a| a.bbox[2] * a.bbox[3] > 0.05),
                "{name}: {found:?}"
            );
        }
        // No pets in a portrait.
        let none = image_of_no_pet();
        assert!(detector.animals(&none).unwrap().is_empty());
    }

    /// Oxford-IIIT Pet's cat breeds (CC BY-SA 4.0), from Hugging Face.
    const CATS: [&str; 12] = [
        "Abyssinian",
        "Bengal",
        "Birman",
        "Bombay",
        "British_Shorthair",
        "Egyptian_Mau",
        "Maine_Coon",
        "Persian",
        "Ragdoll",
        "Russian_Blue",
        "Siamese",
        "Sphynx",
    ];
    /// Some of its dog breeds: only for learning what pets have in common.
    const DOGS: [&str; 8] = [
        "beagle",
        "boxer",
        "chihuahua",
        "pomeranian",
        "pug",
        "samoyed",
        "shiba_inu",
        "yorkshire_terrier",
    ];
    /// Photos of each cat breed: the first few teach what pets have in
    /// common, the rest test telling them apart.
    const PER_CAT: usize = 10;
    const LEARN: usize = 4;
    const PER_DOG: usize = 4;

    fn squash(name: &str) -> String {
        name.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    }

    /// `per` photos of each breed named, kept in `dir` for the next run.
    /// Returns (breed, photo number, file) in the order of `breeds`.
    fn breed_photos(dir: &Path, breeds: &[(&str, usize)]) -> Vec<(usize, usize, PathBuf)> {
        let want = |b: usize, i: usize| dir.join(format!("{}_{i}.jpg", breeds[b].0));
        let all = || {
            breeds
                .iter()
                .enumerate()
                .flat_map(|(b, (_, per))| (0..*per).map(move |i| (b, i, want(b, i))))
                .collect::<Vec<_>>()
        };
        if all().iter().all(|f| f.2.is_file()) {
            return all();
        }
        std::fs::create_dir_all(dir).unwrap();
        let get = |url: &str| -> Vec<u8> {
            for attempt in 1..=4 {
                match ureq::get(url).call() {
                    Ok(mut r) => {
                        return r
                            .body_mut()
                            .with_config()
                            .limit(20 << 20)
                            .read_to_vec()
                            .unwrap();
                    }
                    Err(e) if attempt < 4 => {
                        println!("{url}: {e}, again");
                        std::thread::sleep(std::time::Duration::from_secs(2 * attempt));
                    }
                    Err(e) => panic!("{url}: {e}"),
                }
            }
            unreachable!()
        };
        let api = "https://datasets-server.huggingface.co/rows?dataset=timm%2Foxford-iiit-pet&config=default&split=test";
        let mut ours: Option<Vec<Option<usize>>> = None;
        let mut have = vec![0usize; breeds.len()];
        let mut offset = 0;
        while have.iter().zip(breeds).any(|(n, b)| *n < b.1) && offset < 4000 {
            let page: serde_json::Value =
                serde_json::from_slice(&get(&format!("{api}&offset={offset}&length=100"))).unwrap();
            // The label feature's class names, matched to ours.
            let ours = ours.get_or_insert_with(|| {
                let names = page["features"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|f| f["name"] == "label")
                    .and_then(|f| f["type"]["names"].as_array())
                    .expect("label names");
                names
                    .iter()
                    .map(|n| {
                        let n = squash(n.as_str().unwrap_or_default());
                        breeds.iter().position(|b| squash(b.0) == n)
                    })
                    .collect()
            });
            let rows = page["rows"].as_array().unwrap();
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let row = &row["row"];
                let Some(b) = row["label"]
                    .as_u64()
                    .and_then(|l| ours.get(l as usize).copied().flatten())
                else {
                    continue;
                };
                if have[b] < breeds[b].1
                    && let Some(src) = row["image"]["src"].as_str()
                {
                    std::fs::write(want(b, have[b]), get(src)).unwrap();
                    have[b] += 1;
                }
            }
            offset += rows.len();
        }
        assert!(
            have.iter().zip(breeds).all(|(n, b)| *n == b.1),
            "photos per breed: {have:?}"
        );
        all()
    }

    fn unit(mut v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    fn dot(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    /// What pets have in common: their average, and the main ways they
    /// vary (principal axes, by power iteration).
    fn learn(vectors: &[Vec<f32>], axes: usize) -> (Vec<f32>, Vec<Vec<f32>>) {
        let d = vectors[0].len();
        let n = vectors.len() as f32;
        let mean: Vec<f32> = (0..d)
            .map(|j| vectors.iter().map(|v| v[j]).sum::<f32>() / n)
            .collect();
        let x: Vec<Vec<f32>> = vectors
            .iter()
            .map(|v| v.iter().zip(&mean).map(|(a, m)| a - m).collect())
            .collect();
        let mut found: Vec<Vec<f32>> = Vec::new();
        for a in 0..axes {
            let mut u = unit(
                (0..d)
                    .map(|j| ((j * 7 + a * 13) % 11) as f32 - 5.0)
                    .collect(),
            );
            for _ in 0..300 {
                let mut w = vec![0f32; d];
                for row in &x {
                    let p = dot(row, &u);
                    w.iter_mut().zip(row).for_each(|(w, r)| *w += r * p);
                }
                for prev in &found {
                    let c = dot(&w, prev);
                    w.iter_mut().zip(prev).for_each(|(w, p)| *w -= c * p);
                }
                u = unit(w);
            }
            found.push(u);
        }
        (mean, found)
    }

    fn apply(v: &[f32], mean: &[f32], axes: &[Vec<f32>]) -> Vec<f32> {
        let mut d: Vec<f32> = v.iter().zip(mean).map(|(a, m)| a - m).collect();
        for u in axes {
            let c = dot(&d, u);
            d.iter_mut().zip(u).for_each(|(d, u)| *d -= c * u);
        }
        unit(d)
    }

    /// Groups pets as the app does and scores the result: how pure the
    /// groups are, and how much of each breed stays together.
    fn score(pets: &[(usize, Vec<f32>)], same: f32) -> String {
        let loose = pets
            .iter()
            .enumerate()
            .map(|(i, p)| crate::people::Loose {
                id: i as i64,
                not_person: None,
                embedding: p.1.clone(),
            })
            .collect();
        let groups = crate::people::cluster(loose, same);
        let grouped: usize = groups.iter().map(Vec::len).sum();
        let mut right = 0;
        for g in &groups {
            let mut counts = std::collections::HashMap::new();
            for &i in g {
                *counts.entry(pets[i as usize].0).or_insert(0) += 1;
            }
            right += counts.values().max().unwrap();
        }
        let breeds: std::collections::BTreeSet<usize> = pets.iter().map(|p| p.0).collect();
        let together: f32 = breeds
            .iter()
            .map(|&b| {
                let total = pets.iter().filter(|p| p.0 == b).count();
                let best = groups
                    .iter()
                    .map(|g| g.iter().filter(|&&i| pets[i as usize].0 == b).count())
                    .max()
                    .unwrap_or(0);
                best as f32 / total as f32
            })
            .sum::<f32>()
            / breeds.len() as f32;
        format!(
            "{:>2} groups, purity {:.2}, together {:.2}, alone {:>2}",
            groups.len(),
            right as f32 / grouped.max(1) as f32,
            together,
            pets.len() - grouped
        )
    }

    fn stats(pets: &[(usize, Vec<f32>)]) -> String {
        let (mut same, mut other) = (Vec::new(), Vec::new());
        for (i, a) in pets.iter().enumerate() {
            for b in &pets[i + 1..] {
                let sim = dot(&a.1, &b.1);
                if a.0 == b.0 { &mut same } else { &mut other }.push(sim);
            }
        }
        let pct = |v: &mut Vec<f32>, p: f32| {
            v.sort_by(f32::total_cmp);
            v[((v.len() - 1) as f32 * p) as usize]
        };
        format!(
            "same breed p10 {:.3} median {:.3}; different breeds median {:.3} p90 {:.3}",
            pct(&mut same, 0.1),
            pct(&mut same, 0.5),
            pct(&mut other, 0.5),
            pct(&mut other, 0.9),
        )
    }

    /// Telling pets apart with the real models: the smart search model's
    /// view of each pet's picture. Cats of one breed must look more alike
    /// than cats of different breeds, and three cats of different types
    /// must end up as three pets. (Two pictures of one pet are closer still
    /// than two cats of its breed.) Run with `--ignored`; downloads ~220 MB
    /// of models and ~150 photos.
    #[test]
    #[ignore]
    fn real_pets_are_told_apart() {
        let models = PathBuf::from(
            std::env::var("FUJICLOUD_MODEL_DIR").unwrap_or_else(|_| ".models".into()),
        );
        download(&models, &|_, _| {}).expect("download the detector and DINOv2");
        let detector = Detector::load(&models, 4).unwrap();
        let dino = Identifier::load(&models, 4).unwrap();
        // Bigger DINOv2s, to see whether they tell pets apart better.
        let trials: Vec<(&str, Identifier)> = [
            ("base", "model.onnx"),
            ("base-int8", "model_quantized.onnx"),
        ]
        .into_iter()
        .map(|(label, file)| {
            let path = models
                .join("pets-trial")
                .join(format!("dinov2-base-{file}"));
            let url = format!("https://huggingface.co/Xenova/dinov2-base/resolve/main/onnx/{file}");
            download_files(&[(url.as_str(), path.clone())], &|_, _| {})
                .expect("download DINOv2 base");
            (label, Identifier::load_file(&path, 4).unwrap())
        })
        .collect();
        let mut took = vec![std::time::Duration::ZERO; 1 + trials.len()];
        // Per pet: DINOv2 on the square crop the app has used, DINOv2 on
        // just what's in the box, the coat's colours, then each bigger
        // DINOv2 on just what's in the box.
        type Descriptors = Vec<Vec<f32>>;
        let mut prior_d: Vec<Descriptors> = Vec::new();
        let mut pets_d: Vec<(usize, Descriptors)> = Vec::new();

        let breeds: Vec<(&str, usize)> = CATS
            .iter()
            .map(|b| (*b, PER_CAT))
            .chain(DOGS.iter().map(|b| (*b, PER_DOG)))
            .collect();
        let photos = breed_photos(&photos().join("oxford"), &breeds);
        let mut missed = 0;
        for (breed, i, file) in &photos {
            let image = crate::thumbs::upright(&std::fs::read(file).unwrap(), 1280).unwrap();
            let Some(pet) = detector.animals(&image).unwrap().into_iter().next() else {
                missed += 1;
                continue;
            };
            let square = dino
                .describe(&crate::thumbs::crop_square(&image, pet.bbox, 1.1, 320).unwrap())
                .unwrap();
            let jpeg = crate::thumbs::crop_box(&image, pet.bbox, 224).unwrap();
            let start = std::time::Instant::now();
            let boxed = dino.describe(&jpeg).unwrap();
            took[0] += start.elapsed();
            let mut d = vec![square, boxed, coat(&image, pet.bbox)];
            for (n, (_, model)) in trials.iter().enumerate() {
                let start = std::time::Instant::now();
                d.push(model.describe(&jpeg).unwrap());
                took[n + 1] += start.elapsed();
            }
            if *breed >= CATS.len() || *i < LEARN {
                prior_d.push(d);
            } else {
                pets_d.push((*breed, d));
            }
        }
        println!(
            "{} pets found in {} photos: {} to learn from, {} cats to tell apart",
            prior_d.len() + pets_d.len(),
            photos.len(),
            prior_d.len(),
            pets_d.len()
        );
        assert!(missed <= photos.len() / 10, "{missed} photos without a pet");

        let set = |names: [&str; 3]| names.map(|b| CATS.iter().position(|x| *x == b).unwrap());
        let three = set(["Siamese", "Bombay", "Bengal"]);
        // Different cats that look alike: pale with dark points; warm and
        // spotted or ticked.
        let pointed = set(["Birman", "Ragdoll", "Siamese"]);
        let warm = set(["Abyssinian", "Bengal", "Egyptian_Mau"]);
        let only = |v: &[(usize, Vec<f32>)], which: [usize; 3]| {
            v.iter()
                .filter(|p| which.contains(&p.0))
                .cloned()
                .collect::<Vec<_>>()
        };
        let only_three = |v: &[(usize, Vec<f32>)]| only(v, three);
        let report = |label: &str, pets: &[(usize, Vec<f32>)]| {
            println!("{label}: {}", stats(pets));
            for same in [0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60] {
                println!(
                    "  {label} at {same:.2}: three [{}] pointed [{}] warm [{}] all [{}]",
                    score(&only_three(pets), same),
                    score(&only(pets, pointed), same),
                    score(&only(pets, warm), same),
                    score(pets, same)
                );
            }
        };
        // Each part with what pets share taken away, learned from other
        // photos than those it's tested on.
        let centred = |n: usize| {
            let prior: Vec<Vec<f32>> = prior_d.iter().map(|d| d[n].clone()).collect();
            let (mean, _) = learn(&prior, 0);
            let pets: Vec<(usize, Vec<f32>)> = pets_d
                .iter()
                .map(|(b, d)| (*b, apply(&d[n], &mean, &[])))
                .collect();
            (mean, pets)
        };
        let (_, square) = centred(0);
        let (box_mean, boxed) = centred(1);
        let (coat_mean, coats) = centred(2);
        report("square", &square);
        report("box", &boxed);
        report("coat", &coats);
        let count = (prior_d.len() + pets_d.len()) as u32;
        println!("small: {:?} a pet", took[0] / count);
        for (n, (label, _)) in trials.iter().enumerate() {
            println!("{label}: {:?} a pet", took[n + 1] / count);
            let (_, bigger) = centred(3 + n);
            report(&format!("{label} box"), &bigger);
            let mixed: Vec<(usize, Vec<f32>)> = bigger
                .iter()
                .zip(&coats)
                .map(|((b, d), (_, c))| {
                    let v = d
                        .iter()
                        .map(|x| x * 0.8f32.sqrt())
                        .chain(c.iter().map(|x| x * 0.2f32.sqrt()))
                        .collect();
                    (*b, v)
                })
                .collect();
            report(&format!("{label} box+coat 0.20"), &mixed);
        }
        for share in [0.2f32, 0.35] {
            let mixed: Vec<(usize, Vec<f32>)> = boxed
                .iter()
                .zip(&coats)
                .map(|((b, d), (_, c))| {
                    let v = d
                        .iter()
                        .map(|x| x * (1.0 - share).sqrt())
                        .chain(c.iter().map(|x| x * share.sqrt()))
                        .collect();
                    (*b, v)
                })
                .collect();
            report(&format!("box+coat {share:.2}"), &mixed);
        }

        // The average pet the app ships must be the one learned here; if not
        // (a new model, say), print the new one to ship.
        let learned = [box_mean, coat_mean].concat();
        assert_eq!(learned.len(), basis().len());
        let off = learned
            .iter()
            .zip(basis())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        println!("shipped average pet differs by at most {off:.5}");
        if off > 2e-3 {
            let hex: String = learned
                .iter()
                .flat_map(|x| half::f16::from_f32(*x).to_le_bytes())
                .map(|b| format!("{b:02x}"))
                .collect();
            for (i, line) in hex.as_bytes().chunks(256).enumerate() {
                println!("pet-mean {i:03} {}", std::str::from_utf8(line).unwrap());
            }
            panic!("pet_mean.bin is out of date: replace it with the pet-mean lines above");
        }

        // Pets described and grouped exactly as the app does: three cats of
        // different types must come out as three pets.
        // Described as `Identifier::pet` does: just the pet, then its coat.
        let described: Vec<(usize, Vec<f32>)> = pets_d
            .iter()
            .map(|(b, d)| (*b, [d[1].clone(), d[2].clone()].concat()))
            .collect();
        let shipped: Vec<(usize, Vec<f32>)> = described
            .iter()
            .map(|(b, v)| (*b, distinctive(v)))
            .collect();
        println!("shipped: {}", stats(&shipped));
        let ours = only_three(&described);
        let lib = crate::db::Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let rows: Vec<crate::db::NewPhoto> = (0..ours.len())
            .map(|i| crate::db::NewPhoto {
                path: PathBuf::from(format!("/lib/{i}.jpg")),
                kind: crate::Kind::Jpeg,
                size: 1,
                mtime: 1,
                meta: Default::default(),
                place: None,
            })
            .collect();
        lib.upsert(src.id, &rows).unwrap();
        let mut ids: Vec<i64> = lib.photos().unwrap().iter().map(|p| p.id).collect();
        ids.sort();
        for (id, pet) in ids.iter().zip(&ours) {
            let pet = crate::people::NewFace {
                bbox: [0.1, 0.1, 0.5, 0.5],
                score: 0.9,
                embedding: pet.1.clone(),
            };
            lib.store_pets(*id, "test", &[pet]).unwrap();
        }
        lib.group_faces().unwrap();
        let breed_of = |photo: i64| ours[ids.iter().position(|&i| i == photo).unwrap()].0;
        let groups = lib.people().unwrap();
        for g in &groups {
            let names: Vec<&str> = g.photo_ids.iter().map(|&p| CATS[breed_of(p)]).collect();
            println!("pet {}: {names:?}", g.id);
            let top = three
                .iter()
                .map(|&b| g.photo_ids.iter().filter(|&&p| breed_of(p) == b).count())
                .max()
                .unwrap();
            assert!(
                top * 10 >= g.photo_ids.len() * 8,
                "pet {} mixes breeds",
                g.id
            );
        }
        for &b in &three {
            let total = ours.iter().filter(|p| p.0 == b).count();
            let best = groups
                .iter()
                .map(|g| g.photo_ids.iter().filter(|&&p| breed_of(p) == b).count())
                .max()
                .unwrap_or(0);
            assert!(best * 2 > total, "{}: {best} of {total} together", CATS[b]);
        }
    }

    fn image_of_no_pet() -> Rgb {
        let (width, height) = (320u32, 240u32);
        let data = (0..width * height)
            .flat_map(|i| {
                let v = (i % width * 255 / width) as u8;
                [v, 120, 255 - v]
            })
            .collect();
        Rgb {
            data,
            width,
            height,
        }
    }
}
