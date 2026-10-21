//! On-device "search by what's in the photo".
//!
//! A SigLIP model maps images and text into the same vector space: a photo of
//! a beach at sunset lands near the words "beach at sunset". Each photo is
//! embedded once from its cached thumbnail (no extra network reads); a query
//! is embedded in ~10 ms and compared against every photo with
//! [`crate::search::Index`]. Nothing leaves the PC.
//!
//! The model is downloaded on first use. Runs on ONNX Runtime's CPU provider
//! with int8-quantised weights.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use fast_image_resize::{self as fr, images::Image};
use ort::session::Session;
use ort::value::Tensor;
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use crate::Error;

/// Where a model's files come from and how it expects its inputs.
#[derive(Debug)]
pub struct ModelSpec {
    /// Stored next to each embedding, so a model change re-indexes.
    pub id: &'static str,
    pub base_url: &'static str,
    pub vision: &'static str,
    pub text: &'static str,
    pub tokenizer: &'static str,
    /// Approximate total download, for the UI.
    pub download_mb: u32,
    pub image_size: u32,
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// SigLIP was trained with text padded to exactly this many tokens.
    pub text_len: usize,
}

/// Google's SigLIP (base, 224px), as exported to ONNX for transformers.js.
pub const SIGLIP_BASE: ModelSpec = ModelSpec {
    id: "siglip-base-patch16-224-q8",
    base_url: "https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/",
    vision: "onnx/vision_model_quantized.onnx",
    text: "onnx/text_model_quantized.onnx",
    tokenizer: "tokenizer.json",
    download_mb: 210,
    image_size: 224,
    mean: [0.5, 0.5, 0.5],
    std: [0.5, 0.5, 0.5],
    text_len: 64,
};

impl ModelSpec {
    pub fn files(&self) -> [&'static str; 3] {
        [self.vision, self.text, self.tokenizer]
    }

    fn local(&self, dir: &Path, remote: &str) -> PathBuf {
        dir.join(self.id)
            .join(remote.rsplit('/').next().unwrap_or(remote))
    }

    pub fn is_downloaded(&self, dir: &Path) -> bool {
        self.files().iter().all(|f| self.local(dir, f).is_file())
    }

    /// Downloads whatever is missing. `progress(done, total)` is in bytes;
    /// `total` is 0 until the server reports sizes.
    pub fn download(&self, dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), Error> {
        let mut done = 0u64;
        let mut total = 0u64;
        let mut sizes = Vec::new();
        for file in self.files() {
            let path = self.local(dir, file);
            if path.is_file() {
                let len = fs::metadata(&path)?.len();
                done += len;
                total += len;
                sizes.push(None);
                continue;
            }
            let response = ureq::get(format!("{}{}", self.base_url, file))
                .call()
                .map_err(|e| Error::Model(format!("downloading {file}: {e}")))?;
            let len = response
                .headers()
                .get("content-length")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
            total += len;
            sizes.push(Some((path, response)));
        }
        progress(done, total);
        for (path, response) in sizes.into_iter().flatten() {
            fs::create_dir_all(path.parent().unwrap())?;
            let part = path.with_extension("part");
            let mut out = fs::File::create(&part)?;
            let mut reader = response.into_body().into_reader();
            let mut buf = vec![0u8; 1 << 20];
            loop {
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                done += n as u64;
                progress(done, total);
            }
            out.sync_all()?;
            drop(out);
            fs::rename(&part, &path)?;
        }
        Ok(())
    }
}

pub struct Engine {
    spec: &'static ModelSpec,
    vision: Mutex<Session>,
    text: Mutex<Session>,
    tokenizer: Tokenizer,
    pad_id: u32,
}

fn model_err(e: impl std::fmt::Display) -> Error {
    Error::Model(e.to_string())
}

impl Engine {
    /// Loads a downloaded model. `threads` caps ONNX Runtime's CPU use so
    /// background indexing leaves room for browsing.
    pub fn load(dir: &Path, spec: &'static ModelSpec, threads: usize) -> Result<Self, Error> {
        Self::load_files(
            &spec.local(dir, spec.vision),
            &spec.local(dir, spec.text),
            &spec.local(dir, spec.tokenizer),
            spec,
            threads,
        )
    }

    pub fn load_files(
        vision: &Path,
        text: &Path,
        tokenizer: &Path,
        spec: &'static ModelSpec,
        threads: usize,
    ) -> Result<Self, Error> {
        let env = ort::init()
            .with_name("fujicloud")
            .build()
            .map_err(model_err)?;
        let session = |path: &Path| -> Result<Session, Error> {
            let fail = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", path.display()));
            let builder = Session::builder(&env).map_err(|e| fail(&e))?;
            let mut builder = builder
                .with_intra_threads(threads.max(1))
                .map_err(|e| fail(&e))?;
            builder.commit_from_file(path).map_err(|e| fail(&e))
        };
        let tokenizer = Tokenizer::from_file(tokenizer).map_err(model_err)?;
        let pad_id = tokenizer
            .get_padding()
            .map(|p| p.pad_id)
            .or_else(|| tokenizer.token_to_id("</s>"))
            .unwrap_or(1);
        Ok(Self {
            spec,
            vision: Mutex::new(session(vision)?),
            text: Mutex::new(session(text)?),
            tokenizer,
            pad_id,
        })
    }

    pub fn spec(&self) -> &'static ModelSpec {
        self.spec
    }

    /// Embeds a batch of JPEGs (thumbnails are plenty). Each result is
    /// L2-normalised, so cosine similarity is a dot product.
    pub fn embed_images(&self, jpegs: &[Vec<u8>]) -> Result<Vec<Vec<f32>>, Error> {
        let size = self.spec.image_size as usize;
        let mut pixels = Vec::with_capacity(jpegs.len() * 3 * size * size);
        for jpeg in jpegs {
            pixels.extend(self.preprocess(jpeg)?);
        }
        let input =
            Tensor::from_array(([jpegs.len(), 3, size, size], pixels)).map_err(model_err)?;
        let mut session = self.vision.lock();
        let name = session.inputs()[0].name().to_owned();
        let outputs = session
            .run(ort::inputs![name => input])
            .map_err(model_err)?;
        embeddings_from(&outputs, jpegs.len())
    }

    pub fn embed_text(&self, query: &str) -> Result<Vec<f32>, Error> {
        let text = prompt(query);
        let encoding = self
            .tokenizer
            .encode(text.as_str(), true)
            .map_err(model_err)?;
        let mut ids: Vec<i64> = encoding.get_ids().iter().map(|&i| i as i64).collect();
        ids.truncate(self.spec.text_len);
        ids.resize(self.spec.text_len, self.pad_id as i64);

        let mut session = self.text.lock();
        let names: Vec<String> = session
            .inputs()
            .iter()
            .map(|i| i.name().to_owned())
            .collect();
        let shape = [1usize, self.spec.text_len];
        let mut inputs = Vec::new();
        for name in names {
            // SigLIP attends to padding too; if the export wants a mask, all ones.
            let values = if name.contains("mask") {
                vec![1i64; self.spec.text_len]
            } else {
                ids.clone()
            };
            inputs.push((
                name,
                Tensor::from_array((shape, values)).map_err(model_err)?,
            ));
        }
        let outputs = session.run(inputs).map_err(model_err)?;
        Ok(embeddings_from(&outputs, 1)?.remove(0))
    }

    /// JPEG → square RGB at the model's input size, normalised, CHW.
    fn preprocess(&self, jpeg: &[u8]) -> Result<Vec<f32>, Error> {
        let size = self.spec.image_size;
        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(jpeg));
        decoder.scale(size as u16, size as u16).map_err(model_err)?;
        let raw = decoder.decode().map_err(model_err)?;
        let info = decoder.info().ok_or_else(|| model_err("no frame header"))?;
        let rgb = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => raw,
            jpeg_decoder::PixelFormat::L8 => raw.iter().flat_map(|&l| [l, l, l]).collect(),
            other => return Err(model_err(format!("unsupported pixel format {other:?}"))),
        };
        let src = Image::from_vec_u8(
            info.width as u32,
            info.height as u32,
            rgb,
            fr::PixelType::U8x3,
        )
        .map_err(model_err)?;
        // SigLIP squashes to a square rather than cropping.
        let mut dst = Image::new(size, size, fr::PixelType::U8x3);
        let options = fr::ResizeOptions::new()
            .resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom));
        fr::Resizer::new()
            .resize(&src, &mut dst, &options)
            .map_err(model_err)?;

        let plane = (size * size) as usize;
        let mut out = vec![0f32; 3 * plane];
        for (i, px) in dst.buffer().as_chunks::<3>().0.iter().enumerate() {
            for c in 0..3 {
                out[c * plane + i] = (px[c] as f32 / 255.0 - self.spec.mean[c]) / self.spec.std[c];
            }
        }
        Ok(out)
    }
}

/// Short queries work better phrased the way captions are.
fn prompt(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '\'' || c == '-' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .to_lowercase();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.starts_with("a photo") || cleaned.starts_with("photo of") {
        cleaned
    } else {
        format!("a photo of {cleaned}")
    }
}

/// Picks the pooled embedding output and L2-normalises each row.
fn embeddings_from(
    outputs: &ort::session::SessionOutputs<'_>,
    rows: usize,
) -> Result<Vec<Vec<f32>>, Error> {
    let preferred = ["image_embeds", "text_embeds", "pooler_output"];
    let name = preferred
        .iter()
        .copied()
        .find(|name| outputs.contains_key(name))
        .or_else(|| {
            outputs.keys().find(|k| {
                outputs[*k]
                    .try_extract_tensor::<f32>()
                    .is_ok_and(|(shape, _)| shape.len() == 2)
            })
        })
        .ok_or_else(|| model_err("model has no pooled embedding output"))?;
    let value = &outputs[name];
    let (shape, data) = value.try_extract_tensor::<f32>().map_err(model_err)?;
    if shape.len() != 2 || shape[0] as usize != rows {
        return Err(model_err(format!("unexpected embedding shape {shape:?}")));
    }
    let dim = shape[1] as usize;
    Ok(data
        .chunks(dim)
        .map(|row| {
            let norm = row.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
            row.iter().map(|x| x / norm).collect()
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/tiny-model")
            .join(name)
    }

    /// A stand-in model with the same inputs and outputs as the SigLIP
    /// export, small enough to commit: it checks the plumbing, not quality.
    fn tiny() -> Engine {
        static SPEC: ModelSpec = ModelSpec {
            id: "tiny",
            ..SIGLIP_BASE
        };
        Engine::load_files(
            &fixture("vision.onnx"),
            &fixture("text.onnx"),
            &fixture("tokenizer.json"),
            &SPEC,
            1,
        )
        .expect("tiny model loads")
    }

    fn solid(rgb: [u8; 3]) -> Vec<u8> {
        let mut out = Vec::new();
        let pixels: Vec<u8> = std::iter::repeat_n(rgb, 64 * 48).flatten().collect();
        jpeg_encoder::Encoder::new(&mut out, 95)
            .encode(&pixels, 64, 48, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        out
    }

    #[test]
    fn prompts() {
        assert_eq!(prompt("  Beach, at SUNSET! "), "a photo of beach at sunset");
        assert_eq!(prompt("a photo of a dog"), "a photo of a dog");
    }

    #[test]
    fn embeds_batches_of_images_and_text() {
        let engine = tiny();
        let vecs = engine
            .embed_images(&[solid([255, 0, 0]), solid([0, 0, 255]), solid([255, 0, 0])])
            .unwrap();
        assert_eq!(vecs.len(), 3);
        assert_eq!(vecs[0].len(), 8);
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        assert!((dot(&vecs[0], &vecs[0]) - 1.0).abs() < 1e-4, "normalised");
        assert!(dot(&vecs[0], &vecs[2]) > 0.999, "same image, same vector");
        assert!(dot(&vecs[0], &vecs[1]) < 0.99, "different images differ");

        let a = engine.embed_text("red").unwrap();
        let b = engine.embed_text("blue").unwrap();
        assert_eq!(a.len(), 8);
        assert!(dot(&a, &b) < 0.9999);
    }

    /// The real model, end to end. Downloads ~200 MB, so it only runs when
    /// asked: `FUJICLOUD_MODEL_DIR=… cargo test -p fuji-core -- --ignored`.
    #[test]
    #[ignore]
    fn real_siglip_matches_colours_and_meaning() {
        let dir = std::env::var("FUJICLOUD_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("fujicloud-models"));
        SIGLIP_BASE.download(&dir, &|_, _| {}).expect("download");
        let engine = Engine::load(&dir, &SIGLIP_BASE, 4).expect("load");

        let images = engine
            .embed_images(&[
                solid([220, 30, 30]),
                solid([30, 60, 220]),
                solid([40, 180, 60]),
            ])
            .unwrap();
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        for (i, colour) in ["red", "blue", "green"].iter().enumerate() {
            let q = engine
                .embed_text(&format!("a plain {colour} background"))
                .unwrap();
            let scores: Vec<f32> = images.iter().map(|v| dot(v, &q)).collect();
            println!("{colour}: {scores:?}");
            let best = scores
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0;
            assert_eq!(best, i, "{colour} should match its own image");
        }

        let dog = engine.embed_text("a dog playing in the park").unwrap();
        let puppy = engine.embed_text("a puppy running on grass").unwrap();
        let city = engine.embed_text("skyscrapers at night").unwrap();
        println!(
            "dog~puppy {} dog~city {}",
            dot(&dog, &puppy),
            dot(&dog, &city)
        );
        assert!(dot(&dog, &puppy) > dot(&dog, &city));
    }
}
