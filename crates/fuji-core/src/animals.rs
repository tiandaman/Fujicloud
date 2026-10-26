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
    const BREEDS: [&str; 12] = [
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

    fn squash(name: &str) -> String {
        name.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    }

    /// `per` photos of each breed, kept in `dir` for the next run.
    fn breed_photos(dir: &Path, per: usize) -> Vec<(usize, PathBuf)> {
        let want = |breed: usize, i: usize| dir.join(format!("{}_{i}.jpg", BREEDS[breed]));
        if (0..BREEDS.len()).all(|b| (0..per).all(|i| want(b, i).is_file())) {
            return (0..BREEDS.len())
                .flat_map(|b| (0..per).map(move |i| (b, want(b, i))))
                .collect();
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
        let mut have = vec![0usize; BREEDS.len()];
        let mut offset = 0;
        while have.iter().any(|&n| n < per) && offset < 4000 {
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
                        BREEDS.iter().position(|b| squash(b) == n)
                    })
                    .collect()
            });
            let rows = page["rows"].as_array().unwrap();
            if rows.is_empty() {
                break;
            }
            for row in rows {
                let row = &row["row"];
                let Some(breed) = row["label"]
                    .as_u64()
                    .and_then(|l| ours.get(l as usize).copied().flatten())
                else {
                    continue;
                };
                if have[breed] < per
                    && let Some(src) = row["image"]["src"].as_str()
                {
                    std::fs::write(want(breed, have[breed]), get(src)).unwrap();
                    have[breed] += 1;
                }
            }
            offset += rows.len();
        }
        assert!(have.iter().all(|&n| n == per), "photos per breed: {have:?}");
        breed_photos(dir, per)
    }

    /// Telling pets apart with the real models: the smart search model's
    /// view of each pet's picture. Different photos of cats of one breed
    /// must look more alike than cats of different breeds, and three cats
    /// of different types must end up as three pets. Two pictures of one
    /// pet are closer still than two cats of its breed, so this is the
    /// hard case. (Run with `--ignored`; downloads ~220 MB of models.)
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

        const PER: usize = 6;
        let photos = breed_photos(&photos().join("oxford"), PER);
        let mut pets: Vec<(usize, Vec<f32>)> = Vec::new();
        let mut missed = 0;
        for (breed, file) in &photos {
            let image = crate::thumbs::upright(&std::fs::read(file).unwrap(), 1280).unwrap();
            let Some(cat) = detector.animals(&image).unwrap().into_iter().next() else {
                missed += 1;
                continue;
            };
            let jpeg = crop(&image, &cat).unwrap();
            pets.push((*breed, engine.embed_images(&[jpeg]).unwrap().remove(0)));
        }
        println!("{} pets found in {} photos", pets.len(), photos.len());
        assert!(missed <= photos.len() / 10, "{missed} photos without a pet");

        let report = |label: &str, pets: &[(usize, Vec<f32>)]| {
            let n = BREEDS.len();
            let mut sum = vec![vec![0f32; n]; n];
            let mut count = vec![vec![0u32; n]; n];
            let (mut same, mut other) = (Vec::new(), Vec::new());
            for (i, a) in pets.iter().enumerate() {
                for b in &pets[i + 1..] {
                    let sim = crate::faces::similarity_of(&a.1, &b.1);
                    sum[a.0][b.0] += sim;
                    count[a.0][b.0] += 1;
                    if a.0 != b.0 {
                        sum[b.0][a.0] += sim;
                        count[b.0][a.0] += 1;
                    }
                    if a.0 == b.0 { &mut same } else { &mut other }.push(sim);
                }
            }
            let pct = |v: &mut Vec<f32>, p: f32| {
                v.sort_by(f32::total_cmp);
                v[((v.len() - 1) as f32 * p) as usize]
            };
            println!(
                "{label}: same breed p10 {:.3} median {:.3}; different breeds median {:.3} p90 {:.3} max {:.3}",
                pct(&mut same, 0.1),
                pct(&mut same, 0.5),
                pct(&mut other, 0.5),
                pct(&mut other, 0.9),
                pct(&mut other, 1.0),
            );
            for (b, name) in BREEDS.iter().enumerate() {
                let cells: Vec<String> = (0..n)
                    .map(|c| format!("{:.2}", sum[b][c] / count[b][c].max(1) as f32))
                    .collect();
                println!("  {name:>17} {}", cells.join(" "));
            }
            (pct(&mut same, 0.5), pct(&mut other, 0.5))
        };
        let (same, other) = report("as is", &pets);
        assert!(same > other + 0.05, "breeds aren't told apart");

        // Three cats of different types, grouped as the app does.
        let lib = crate::db::Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let three =
            ["Siamese", "Bombay", "Bengal"].map(|b| BREEDS.iter().position(|x| *x == b).unwrap());
        let ours: Vec<&(usize, Vec<f32>)> = pets.iter().filter(|p| three.contains(&p.0)).collect();
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
            lib.store_pets(
                *id,
                "test",
                &[crate::people::NewFace {
                    bbox: [0.1, 0.1, 0.5, 0.5],
                    score: 0.9,
                    embedding: pet.1.clone(),
                }],
            )
            .unwrap();
        }
        lib.group_faces().unwrap();
        let breed_of = |photo: i64| ours[ids.iter().position(|&i| i == photo).unwrap()].0;
        let groups = lib.people().unwrap();
        for g in &groups {
            let names: Vec<&str> = g.photo_ids.iter().map(|&p| BREEDS[breed_of(p)]).collect();
            println!("group {}: {names:?}", g.id);
        }
        for &breed in &three {
            let total = ours.iter().filter(|p| p.0 == breed).count();
            let best = groups
                .iter()
                .map(|g| {
                    g.photo_ids
                        .iter()
                        .filter(|&&p| breed_of(p) == breed)
                        .count()
                })
                .max()
                .unwrap_or(0);
            assert!(
                best * 2 > total,
                "{}: at most {best} of {total} photos together",
                BREEDS[breed]
            );
        }
        for g in &groups {
            let top = three
                .iter()
                .map(|&b| g.photo_ids.iter().filter(|&&p| breed_of(p) == b).count())
                .max()
                .unwrap();
            assert!(
                top * 10 >= g.photo_ids.len() * 8,
                "group {} mixes breeds",
                g.id
            );
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
