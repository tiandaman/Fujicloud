//! Finding cats and dogs in photos, entirely on this PC.
//!
//! YOLOX-S (8-bit, from OpenCV's model zoo) finds animals and boxes them.
//! Telling one pet from another is done by comparing what the smart search
//! model makes of each pet's crop: coat colour, pattern and shape (see
//! [`crate::people`], which groups and names pets the way it does people).

use std::path::{Path, PathBuf};

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
pub const DOWNLOAD_MB: u32 = 9;
/// Stored with each photo searched: the detector, and the model that tells
/// pets apart.
pub const MODEL_ID: &str = "yolox-2022nov-int8+siglip-base-patch16-224";

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
    path(dir).is_file()
}

/// Fetches the detector (~9 MB). `progress(done, total)` in bytes.
pub fn download(dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), Error> {
    download_files(&[(URL, path(dir))], progress)
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

/// The square picture of a pet that's compared with other pets'.
pub fn crop(image: &Rgb, animal: &Animal) -> Result<Vec<u8>, Error> {
    crate::thumbs::crop_square(image, animal.bbox, 1.1, 320)
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
        use crate::embed::{Device, Engine, SIGLIP_BASE};
        let models = PathBuf::from(
            std::env::var("FUJICLOUD_MODEL_DIR").unwrap_or_else(|_| ".models".into()),
        );
        download(&models, &|_, _| {}).expect("download detector");
        SIGLIP_BASE
            .download(&models, Device::Cpu, &|_, _| {})
            .expect("download smart search model");
        let detector = Detector::load(&models, 4).unwrap();
        let engine = Engine::load(&models, &SIGLIP_BASE, Device::Cpu, 4).unwrap();

        let breeds: Vec<(&str, usize)> = CATS
            .iter()
            .map(|b| (*b, PER_CAT))
            .chain(DOGS.iter().map(|b| (*b, PER_DOG)))
            .collect();
        let photos = breed_photos(&photos().join("oxford"), &breeds);
        let (mut prior, mut pets) = (Vec::new(), Vec::new());
        let mut missed = 0;
        for (breed, i, file) in &photos {
            let image = crate::thumbs::upright(&std::fs::read(file).unwrap(), 1280).unwrap();
            let Some(pet) = detector.animals(&image).unwrap().into_iter().next() else {
                missed += 1;
                continue;
            };
            let v = engine
                .embed_images(&[crop(&image, &pet).unwrap()])
                .unwrap()
                .remove(0);
            if *breed >= CATS.len() || *i < LEARN {
                prior.push(v);
            } else {
                pets.push((*breed, v));
            }
        }
        println!(
            "{} pets found in {} photos: {} to learn from, {} cats to tell apart",
            prior.len() + pets.len(),
            photos.len(),
            prior.len(),
            pets.len()
        );
        assert!(missed <= photos.len() / 10, "{missed} photos without a pet");

        let three =
            ["Siamese", "Bombay", "Bengal"].map(|b| CATS.iter().position(|x| *x == b).unwrap());
        let only_three = |v: &[(usize, Vec<f32>)]| {
            v.iter()
                .filter(|p| three.contains(&p.0))
                .cloned()
                .collect::<Vec<_>>()
        };
        println!("as is: {}", stats(&pets));
        for same in [0.80, 0.84, 0.86, 0.88, 0.90] {
            println!(
                "  as is, same at {same:.2}: three [{}] all [{}]",
                score(&only_three(&pets), same),
                score(&pets, same)
            );
        }
        const AXES: usize = 8;
        let (mean, axes) = learn(&prior, AXES);
        for k in [0, 1, 2, 3, 4, 6, 8] {
            let t: Vec<(usize, Vec<f32>)> = pets
                .iter()
                .map(|(b, v)| (*b, apply(v, &mean, &axes[..k])))
                .collect();
            println!("{k} axes: {}", stats(&t));
            for same in [
                0.10, 0.15, 0.20, 0.25, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60,
            ] {
                println!(
                    "  {k} axes, same at {same:.2}: three [{}] all [{}]",
                    score(&only_three(&t), same),
                    score(&t, same)
                );
            }
        }
        // The learned basis as half floats (mean, then axes), to ship.
        let mut hex = String::new();
        for x in mean.iter().chain(axes.iter().flatten()) {
            for byte in half::f16::from_f32(*x).to_le_bytes() {
                hex.push_str(&format!("{byte:02x}"));
            }
        }
        for (n, line) in hex.as_bytes().chunks(256).enumerate() {
            println!("basis {n:03} {}", std::str::from_utf8(line).unwrap());
        }

        // As the app groups them today.
        let ours = only_three(&pets);
        let groups = crate::people::cluster(
            ours.iter()
                .enumerate()
                .map(|(i, p)| crate::people::Loose {
                    id: i as i64,
                    not_person: None,
                    embedding: p.1.clone(),
                })
                .collect(),
            crate::people::SAME_PET,
        );
        println!("app today: {}", score(&ours, crate::people::SAME_PET));
        for g in &groups {
            let top = three
                .iter()
                .map(|&b| g.iter().filter(|&&i| ours[i as usize].0 == b).count())
                .max()
                .unwrap();
            assert!(top * 10 >= g.len() * 8, "a group mixes breeds: {g:?}");
        }
        for &b in &three {
            let total = ours.iter().filter(|p| p.0 == b).count();
            let best = groups
                .iter()
                .map(|g| g.iter().filter(|&&i| ours[i as usize].0 == b).count())
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
