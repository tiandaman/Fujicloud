//! On-device "search by what's in the photo".
//!
//! A SigLIP model maps images and text into the same vector space: a photo of
//! a beach at sunset lands near the words "beach at sunset". Each photo is
//! embedded once from its cached thumbnail (no extra network reads); a query
//! is embedded in ~10 ms and compared against every photo with
//! [`crate::search::Index`]. Nothing leaves the PC.
//!
//! The model is downloaded on first use. Photos are analysed on the GPU via
//! DirectML on Windows (any DirectX 12 card: NVIDIA, AMD, Intel) with a
//! half-precision vision model, or on the CPU with an 8-bit one. Typed
//! queries always run on the CPU: one short sentence doesn't need a GPU.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fast_image_resize::{self as fr, images::Image};
use ort::session::{Session, builder::SessionBuilder};
use ort::value::{Tensor, TensorElementType, ValueType};
use parking_lot::Mutex;
use tokenizers::Tokenizer;

use crate::Error;

/// Where inference runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Cpu,
    /// DirectML (Windows). `None` lets it pick the fastest GPU; an index
    /// forces a DXGI adapter (tests use this to run on the software one).
    Gpu(Option<i32>),
}

impl Device {
    pub fn is_gpu(self) -> bool {
        matches!(self, Device::Gpu(_))
    }
}

/// The graphics card smart search would use, if it can use one at all.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gpu {
    pub name: String,
    pub vram_mb: u64,
}

/// The fastest hardware GPU, as DirectML's "high performance" preference
/// would choose it (the discrete card on laptops that also have an
/// integrated one). Software adapters don't count.
#[cfg(windows)]
pub fn best_gpu() -> Option<Gpu> {
    use windows::Win32::Graphics::Dxgi::{
        CreateDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE,
        IDXGIAdapter1, IDXGIFactory6,
    };
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory1().ok()?;
        for i in 0..8 {
            let adapter: IDXGIAdapter1 = factory
                .EnumAdapterByGpuPreference(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE)
                .ok()?;
            let desc = adapter.GetDesc1().ok()?;
            // Microsoft's own adapters (WARP, the basic display driver in
            // VMs and on PCs without a graphics driver) aren't GPUs.
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || desc.VendorId == 0x1414 {
                continue;
            }
            let len = desc
                .Description
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(desc.Description.len());
            return Some(Gpu {
                name: String::from_utf16_lossy(&desc.Description[..len])
                    .trim()
                    .to_owned(),
                vram_mb: (desc.DedicatedVideoMemory / (1 << 20)) as u64,
            });
        }
        None
    }
}

#[cfg(not(windows))]
pub fn best_gpu() -> Option<Gpu> {
    None
}

/// Where a model's files come from and how it expects its inputs.
#[derive(Debug)]
pub struct ModelSpec {
    /// Stored next to each embedding. Every variant below maps into the same
    /// vector space, so switching CPU/GPU doesn't re-index the library.
    pub id: &'static str,
    pub base_url: &'static str,
    /// 8-bit weights: smallest download, fastest on CPUs.
    pub vision_cpu: &'static str,
    /// Half-precision first (GPUs are built for it), full precision if the
    /// repository doesn't have it.
    pub vision_gpu: &'static [&'static str],
    pub text: &'static str,
    pub tokenizer: &'static str,
    /// Approximate total download, for the UI.
    pub download_mb_cpu: u32,
    pub download_mb_gpu: u32,
    pub image_size: u32,
    pub mean: [f32; 3],
    pub std: [f32; 3],
    /// SigLIP was trained with text padded to exactly this many tokens.
    pub text_len: usize,
}

/// Google's SigLIP (base, 224px), as exported to ONNX for transformers.js.
pub const SIGLIP_BASE: ModelSpec = ModelSpec {
    id: "siglip-base-patch16-224",
    base_url: "https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/",
    vision_cpu: "onnx/vision_model_quantized.onnx",
    vision_gpu: &["onnx/vision_model_fp16.onnx", "onnx/vision_model.onnx"],
    text: "onnx/text_model_quantized.onnx",
    tokenizer: "tokenizer.json",
    download_mb_cpu: 210,
    download_mb_gpu: 300,
    image_size: 224,
    mean: [0.5, 0.5, 0.5],
    std: [0.5, 0.5, 0.5],
    text_len: 64,
};

impl ModelSpec {
    fn local(&self, dir: &Path, remote: &str) -> PathBuf {
        dir.join(self.id)
            .join(remote.rsplit('/').next().unwrap_or(remote))
    }

    pub fn download_mb(&self, device: Device) -> u32 {
        if device.is_gpu() {
            self.download_mb_gpu
        } else {
            self.download_mb_cpu
        }
    }

    /// The vision model file for `device`, if downloaded.
    fn vision_file(&self, dir: &Path, device: Device) -> Option<PathBuf> {
        if device.is_gpu() {
            self.vision_gpu
                .iter()
                .map(|f| self.local(dir, f))
                .find(|p| p.is_file())
        } else {
            Some(self.local(dir, self.vision_cpu)).filter(|p| p.is_file())
        }
    }

    pub fn is_downloaded(&self, dir: &Path, device: Device) -> bool {
        self.vision_file(dir, device).is_some()
            && self.local(dir, self.text).is_file()
            && self.local(dir, self.tokenizer).is_file()
    }

    /// Downloads whatever `device` needs that is missing. `progress(done,
    /// total)` is in bytes; `total` is 0 until the server reports sizes.
    pub fn download(
        &self,
        dir: &Path,
        device: Device,
        progress: &dyn Fn(u64, u64),
    ) -> Result<(), Error> {
        let _one = DOWNLOAD.lock();
        let mut wanted: Vec<&[&str]> = vec![
            std::slice::from_ref(&self.text),
            std::slice::from_ref(&self.tokenizer),
        ];
        wanted.push(if device.is_gpu() {
            self.vision_gpu
        } else {
            std::slice::from_ref(&self.vision_cpu)
        });

        // Size everything up first so progress has a total, and so a
        // missing variant falls back to the next one before any big transfer.
        let agent = agent(STALL);
        let mut done = 0u64;
        let mut total = 0u64;
        let mut pending = Vec::new();
        'files: for candidates in wanted {
            if let Some(path) = candidates
                .iter()
                .map(|f| self.local(dir, f))
                .find(|p| p.is_file())
            {
                let len = fs::metadata(&path)?.len();
                done += len;
                total += len;
                continue;
            }
            let mut last_err = None;
            for file in candidates {
                let url = format!("{}{}", self.base_url, file);
                match remote_len(&agent, &url) {
                    Ok(len) => {
                        total += len;
                        pending.push((self.local(dir, file), url, len));
                        continue 'files;
                    }
                    Err(e) => last_err = Some(format!("downloading {file}: {e}")),
                }
            }
            return Err(Error::Model(last_err.unwrap_or_default()));
        }
        progress(done, total);
        for (path, url, len) in pending {
            fetch(&agent, &url, &path, len, &mut |have| {
                progress(done + have, total)
            })?;
            done += fs::metadata(&path)?.len();
        }
        Ok(())
    }
}

/// One download at a time, so a second caller finds the files the first one
/// fetched instead of racing it for the same paths.
static DOWNLOAD: Mutex<()> = Mutex::new(());

/// Downloads each `(url, path)` that isn't there yet, resuming dropped
/// transfers. `progress(done, total)` is in bytes.
pub(crate) fn download_files(
    files: &[(&str, PathBuf)],
    progress: &dyn Fn(u64, u64),
) -> Result<(), Error> {
    let _one = DOWNLOAD.lock();
    let agent = agent(STALL);
    let (mut done, mut total) = (0u64, 0u64);
    let mut pending = Vec::new();
    for (url, path) in files {
        if path.is_file() {
            let len = fs::metadata(path)?.len();
            (done, total) = (done + len, total + len);
            continue;
        }
        let len =
            remote_len(&agent, url).map_err(|e| Error::Model(format!("downloading {url}: {e}")))?;
        total += len;
        pending.push((*url, path, len));
    }
    progress(done, total);
    for (url, path, len) in pending {
        fetch(&agent, url, path, len, &mut |have| {
            progress(done + have, total)
        })?;
        done += fs::metadata(path)?.len();
    }
    Ok(())
}

/// How long a download may go without receiving anything before the
/// connection is dropped and the transfer resumed. Without a limit, a
/// stalled connection waits forever.
const STALL: Duration = Duration::from_secs(45);

fn agent(stall: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_resolve(Some(Duration::from_secs(30)))
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_recv_response(Some(stall))
        .timeout_per_read(Some(stall))
        .build()
        .into()
}

/// A file's size, from a one-byte ranged request (a HEAD wouldn't survive
/// the redirect to a signed CDN link).
fn remote_len(agent: &ureq::Agent, url: &str) -> Result<u64, ureq::Error> {
    // A dropped connection or a server hiccup is worth another go; a file
    // that isn't there (404) isn't, so the caller can try the next variant.
    const ATTEMPTS: u32 = 5;
    let mut attempt = 0;
    loop {
        attempt += 1;
        match size_of(agent, url) {
            Err(ureq::Error::StatusCode(code)) if code < 500 => {
                return Err(ureq::Error::StatusCode(code));
            }
            Err(_) if attempt < ATTEMPTS => {
                std::thread::sleep(Duration::from_millis(250 << attempt));
            }
            result => return result,
        }
    }
}

fn size_of(agent: &ureq::Agent, url: &str) -> Result<u64, ureq::Error> {
    let response = agent.get(url).header("Range", "bytes=0-0").call()?;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    };
    let len = match response.status().as_u16() {
        206 => header("content-range").and_then(|v| v.rsplit('/').next()?.parse().ok()),
        _ => header("content-length").and_then(|v| v.parse().ok()),
    };
    Ok(len.unwrap_or(0))
}

/// Downloads `url` to `path`, picking up where it left off if the
/// connection drops or stalls, or the app was closed part-way. `progress`
/// gets the bytes received so far.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    path: &Path,
    len: u64,
    progress: &mut dyn FnMut(u64),
) -> Result<(), Error> {
    /// Tries in a row that receive nothing before giving up.
    const ATTEMPTS: u32 = 5;
    fs::create_dir_all(path.parent().unwrap())?;
    remove_old_parts(path);
    // A fixed name, so the next run can resume it; the lock keeps a second
    // copy of the app from writing to it at the same time.
    let part = path.with_extension("part");
    let mut out = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&part)?;
    if let Err(fs::TryLockError::WouldBlock) = out.try_lock() {
        return Err(Error::Model(
            "Another copy of Fujicloud is downloading the model.".into(),
        ));
    }
    let mut have = out.metadata()?.len();
    if len == 0 || have > len {
        // Nothing to check a leftover against: start over.
        out.set_len(0)?;
        have = 0;
    }
    out.seek(SeekFrom::Start(have))?;
    progress(have);
    let mut fruitless = 0;
    while len == 0 || have < len {
        let before = have;
        let result = fetch_from(agent, url, &mut out, &mut have, progress);
        if result.is_ok() && len == 0 {
            break;
        }
        if have > before {
            fruitless = 0;
            continue;
        }
        fruitless += 1;
        if fruitless >= ATTEMPTS {
            return Err(result
                .err()
                .unwrap_or_else(|| Error::Model(format!("{url}: got {have} of {len} bytes"))));
        }
        std::thread::sleep(Duration::from_millis(500 << fruitless));
    }
    out.sync_all()?;
    drop(out);
    fs::rename(&part, path)?;
    Ok(())
}

/// Earlier versions named partial downloads `<name>.<process id>.part` and
/// left them behind when the app was closed.
fn remove_old_parts(path: &Path) {
    let (Some(dir), Some(stem)) = (path.parent(), path.file_stem().and_then(|s| s.to_str())) else {
        return;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name.to_str().and_then(|n| {
            n.strip_prefix(stem)?
                .strip_prefix('.')?
                .strip_suffix(".part")
        });
        if pid.is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn fetch_from(
    agent: &ureq::Agent,
    url: &str,
    out: &mut fs::File,
    have: &mut u64,
    progress: &mut dyn FnMut(u64),
) -> Result<(), Error> {
    let mut request = agent.get(url);
    if *have > 0 {
        request = request.header("Range", format!("bytes={have}-"));
    }
    let response = request
        .call()
        .map_err(|e| Error::Model(format!("downloading {url}: {e}")))?;
    if *have > 0 && response.status().as_u16() != 206 {
        // The server ignored the range and is sending the whole file again.
        out.set_len(0)?;
        out.seek(SeekFrom::Start(0))?;
        *have = 0;
    }
    let mut reader = response.into_body().into_reader();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        out.write_all(&buf[..n])?;
        *have += n as u64;
        progress(*have);
    }
}

pub struct Engine {
    spec: &'static ModelSpec,
    device: Device,
    vision: Mutex<Session>,
    /// Some exports (the fp16 one) take half-precision pixels.
    vision_f16: bool,
    text: Mutex<Session>,
    tokenizer: Tokenizer,
    pad_id: u32,
}

pub(crate) fn model_err(e: impl std::fmt::Display) -> Error {
    Error::Model(e.to_string())
}

impl Engine {
    /// Loads a downloaded model. On the GPU, failure to start DirectML is an
    /// error (callers fall back to the CPU); `threads` caps CPU use so
    /// background indexing leaves room for browsing.
    pub fn load(
        dir: &Path,
        spec: &'static ModelSpec,
        device: Device,
        threads: usize,
    ) -> Result<Self, Error> {
        let vision = spec
            .vision_file(dir, device)
            .ok_or_else(|| model_err("the model isn't downloaded"))?;
        Self::load_files(
            &vision,
            &spec.local(dir, spec.text),
            &spec.local(dir, spec.tokenizer),
            spec,
            device,
            threads,
        )
    }

    pub fn load_files(
        vision: &Path,
        text: &Path,
        tokenizer: &Path,
        spec: &'static ModelSpec,
        device: Device,
        threads: usize,
    ) -> Result<Self, Error> {
        let id = match device {
            Device::Gpu(id) => id,
            Device::Cpu => None,
        };
        Self::load_with(vision, text, tokenizer, spec, device, threads, &|b| {
            gpu_builder(b, id)
        })
    }

    /// `gpu` sets up the vision session when `device` is a GPU.
    fn load_with(
        vision: &Path,
        text: &Path,
        tokenizer: &Path,
        spec: &'static ModelSpec,
        device: Device,
        threads: usize,
        gpu: &dyn Fn(SessionBuilder) -> Result<SessionBuilder, String>,
    ) -> Result<Self, Error> {
        let env = environment()?;
        let session = |path: &Path, device: Device| -> Result<Session, Error> {
            let fail = |e: &dyn std::fmt::Display| Error::Model(format!("{}: {e}", path.display()));
            let builder = Session::builder(&env).map_err(|e| fail(&e))?;
            let mut builder = match device {
                Device::Cpu => builder
                    .with_intra_threads(threads.max(1))
                    .map_err(|e| fail(&e))?,
                Device::Gpu(_) => gpu(builder).map_err(|e| fail(&e))?,
            };
            builder.commit_from_file(path).map_err(|e| fail(&e))
        };
        let tokenizer = Tokenizer::from_file(tokenizer).map_err(model_err)?;
        let pad_id = tokenizer
            .get_padding()
            .map(|p| p.pad_id)
            .or_else(|| tokenizer.token_to_id("</s>"))
            .unwrap_or(1);
        let vision = session(vision, device)?;
        let vision_f16 = matches!(
            vision.inputs().first().map(|i| i.dtype()),
            Some(ValueType::Tensor {
                ty: TensorElementType::Float16,
                ..
            })
        );
        Ok(Self {
            spec,
            device,
            vision: Mutex::new(vision),
            vision_f16,
            // One query at a time: quicker on the CPU than a round trip to the GPU.
            text: Mutex::new(session(text, Device::Cpu)?),
            tokenizer,
            pad_id,
        })
    }

    pub fn spec(&self) -> &'static ModelSpec {
        self.spec
    }

    pub fn device(&self) -> Device {
        self.device
    }

    /// Photos per inference call: GPUs want big batches, CPUs don't care.
    pub fn batch_size(&self) -> usize {
        if self.device.is_gpu() { 32 } else { 8 }
    }

    /// Embeds a batch of JPEGs (thumbnails are plenty). Each result is
    /// L2-normalised, so cosine similarity is a dot product.
    pub fn embed_images(&self, jpegs: &[Vec<u8>]) -> Result<Vec<Vec<f32>>, Error> {
        let size = self.spec.image_size as usize;
        let mut pixels = Vec::with_capacity(jpegs.len() * 3 * size * size);
        for jpeg in jpegs {
            pixels.extend(self.preprocess(jpeg)?);
        }
        let shape = [jpegs.len(), 3, size, size];
        let input = if self.vision_f16 {
            let half: Vec<half::f16> = pixels.iter().map(|&x| half::f16::from_f32(x)).collect();
            Tensor::from_array((shape, half))
                .map_err(model_err)?
                .upcast()
        } else {
            Tensor::from_array((shape, pixels))
                .map_err(model_err)?
                .upcast()
        };
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

/// ONNX Runtime allows one environment per process, ever (even after the
/// first is dropped), so every engine shares this one: switching between
/// GPU and CPU loads a second engine.
pub(crate) fn environment() -> Result<ort::environment::Environment, Error> {
    static ENV: Mutex<Option<ort::environment::Environment>> = Mutex::new(None);
    let mut env = ENV.lock();
    if env.is_none() {
        *env = Some(
            ort::init()
                .with_name("fujicloud")
                // Microsoft's builds report usage (model names, run counts)
                // through Windows diagnostics; nothing leaves this PC.
                .with_telemetry(false)
                .build()
                .map_err(model_err)?,
        );
    }
    Ok(env.clone().unwrap())
}

/// Session options for DirectML: it needs sequential execution and no memory
/// pattern, and must fail loudly rather than quietly fall back to the CPU.
#[cfg(windows)]
fn gpu_builder(builder: SessionBuilder, device_id: Option<i32>) -> Result<SessionBuilder, String> {
    use ort::ep::directml::{DeviceFilter, DirectML, PerformancePreference};
    let ep = match device_id {
        Some(id) => DirectML::default().with_device_id(id),
        None => DirectML::default()
            .with_device_filter(DeviceFilter::Gpu)
            .with_performance_preference(PerformancePreference::HighPerformance),
    };
    dml_options(builder)?
        .with_execution_providers([ep.build().error_on_failure()])
        .map_err(|e| e.to_string())
}

/// DirectML can't reuse memory plans or run nodes in parallel.
#[cfg(windows)]
fn dml_options(builder: SessionBuilder) -> Result<SessionBuilder, String> {
    builder
        .with_memory_pattern(false)
        .map_err(|e| e.to_string())?
        .with_parallel_execution(false)
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn gpu_builder(_: SessionBuilder, _: Option<i32>) -> Result<SessionBuilder, String> {
    Err("GPU acceleration needs Windows (DirectML)".into())
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
            outputs.keys().find(|k| match outputs[*k].dtype() {
                ValueType::Tensor { shape, .. } => shape.len() == 2,
                _ => false,
            })
        })
        .ok_or_else(|| model_err("model has no pooled embedding output"))?;
    let value = &outputs[name];
    let (shape, data): (Vec<i64>, Vec<f32>) = match value.try_extract_tensor::<f32>() {
        Ok((shape, data)) => (shape.to_vec(), data.to_vec()),
        Err(_) => {
            let (shape, data) = value.try_extract_tensor::<half::f16>().map_err(model_err)?;
            (shape.to_vec(), data.iter().map(|x| x.to_f32()).collect())
        }
    };
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
            Device::Cpu,
            1,
        )
        .expect("tiny model loads")
    }

    fn solid(rgb: [u8; 3]) -> Vec<u8> {
        picture(|_, _| rgb)
    }

    /// A 64×48 JPEG with each pixel from `f(x, y)`.
    fn picture(f: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        let (w, h) = (64, 48);
        let pixels: Vec<u8> = (0..w * h).flat_map(|i| f(i % w, i / w)).collect();
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, 95)
            .encode(&pixels, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
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

    /// The app loads a second engine when it falls back from the GPU to the
    /// CPU, or when the GPU setting changes.
    #[test]
    fn engines_can_be_loaded_more_than_once() {
        let first = tiny();
        let second = tiny();
        drop(first);
        let third = tiny();
        let img = [solid([10, 200, 90])];
        let (a, b) = (
            second.embed_images(&img).unwrap(),
            third.embed_images(&img).unwrap(),
        );
        // Kernels may round differently from run to run.
        let diff = a[0]
            .iter()
            .zip(&b[0])
            .map(|(x, y)| (x - y).abs())
            .fold(0f32, f32::max);
        assert!(diff < 1e-5, "{a:?} vs {b:?}");
    }

    /// Colours and meanings a working image/text model must get right.
    fn check_real(engine: &Engine) {
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
            println!("{:?} {colour}: {scores:?}", engine.device());
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

        // Near-identical shots (the same scene a moment later: a pixel to the
        // side, a touch brighter) must reach the duplicate threshold, and a
        // different picture mustn't.
        let scene = |shift: f32, gain: f32| {
            picture(move |x, y| {
                let x = x as f32 + shift;
                let (dx, dy) = (x - 44.0, y as f32 - 12.0);
                let px: [f32; 3] = if dx * dx + dy * dy < 36.0 {
                    [255.0, 236.0, 170.0]
                } else if y as f32 > 30.0 + (x / 9.0).sin() * 4.0 {
                    [30.0, 70.0, 40.0]
                } else {
                    [110.0 + y as f32 * 3.0, 160.0 + y as f32 * 2.0, 230.0]
                };
                px.map(|c| (c * gain).min(255.0) as u8)
            })
        };
        let checks = picture(|x, y| {
            if (x / 8 + y / 8) % 2 == 0 {
                [0; 3]
            } else {
                [255; 3]
            }
        });
        let v = engine
            .embed_images(&[scene(0.0, 1.0), scene(1.0, 1.04), checks])
            .unwrap();
        let (near, other) = (dot(&v[0], &v[1]), dot(&v[0], &v[2]));
        println!("near-identical {near:.3}, different {other:.3}");
        assert!(
            near >= crate::dupes::NEAR_DUPLICATE,
            "near-identical scored {near}"
        );
        assert!(
            other < crate::dupes::NEAR_DUPLICATE,
            "a different picture scored {other}"
        );
    }

    /// A transfer that's cut off part-way resumes where it stopped.
    #[test]
    fn downloads_resume_after_a_dropped_connection() {
        use std::net::TcpListener;
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.onnx", listener.local_addr().unwrap());
        let served = body.clone();
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            for stream in listener.incoming().take(3) {
                let mut stream = stream.unwrap();
                let mut req = [0u8; 4096];
                let n = stream.read(&mut req).unwrap();
                let req = String::from_utf8_lossy(&req[..n]).to_lowercase();
                let from = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .map(|r| r.trim().to_owned());
                let head = |status: &str, extra: String, len: usize| {
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {len}\r\n{extra}Connection: close\r\n\r\n"
                    )
                };
                let all = served.len();
                match from.as_deref() {
                    Some("0-0") => {
                        let h = head(
                            "206 Partial Content",
                            format!("Content-Range: bytes 0-0/{all}\r\n"),
                            1,
                        );
                        stream.write_all(h.as_bytes()).unwrap();
                        stream.write_all(&served[..1]).unwrap();
                    }
                    // The first real attempt dies a third of the way in.
                    None => {
                        stream
                            .write_all(head("200 OK", String::new(), all).as_bytes())
                            .unwrap();
                        stream.write_all(&served[..all / 3]).unwrap();
                    }
                    Some(range) => {
                        let start: usize = range.trim_end_matches('-').parse().unwrap();
                        let extra = format!("Content-Range: bytes {start}-{}/{all}\r\n", all - 1);
                        let h = head("206 Partial Content", extra, all - start);
                        stream.write_all(h.as_bytes()).unwrap();
                        stream.write_all(&served[start..]).unwrap();
                    }
                }
                asked.push(from);
            }
            asked
        });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.onnx");
        let agent = agent(STALL);
        let len = remote_len(&agent, &url).unwrap();
        assert_eq!(len, body.len() as u64);
        let mut seen = 0;
        fetch(&agent, &url, &path, len, &mut |have| seen = have).unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        assert_eq!(seen, len);
        let asked = server.join().unwrap();
        assert_eq!(
            asked,
            [
                Some("0-0".into()),
                None,
                Some(format!("{}-", body.len() / 3))
            ]
        );
        // Nothing is left behind.
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// Asking for a file's size survives a dropped connection, but a missing
    /// file is reported straight away.
    #[test]
    fn sizing_retries_dropped_connections() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let mut paths = Vec::new();
            for (n, stream) in listener.incoming().take(4).enumerate() {
                let mut stream = stream.unwrap();
                let mut req = [0u8; 4096];
                let got = stream.read(&mut req).unwrap();
                let req = String::from_utf8_lossy(&req[..got]).into_owned();
                paths.push(req.split_whitespace().nth(1).unwrap_or("").to_owned());
                let reply = match n {
                    // Hang up without answering, twice.
                    0 | 1 => continue,
                    2 => {
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: 1\r\nContent-Range: bytes 0-0/123456\r\nConnection: close\r\n\r\nx"
                    }
                    _ => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                };
                stream.write_all(reply.as_bytes()).unwrap();
            }
            paths
        });
        let agent = agent(STALL);
        assert_eq!(
            remote_len(&agent, &format!("{base}/model.onnx")).unwrap(),
            123_456
        );
        assert!(matches!(
            remote_len(&agent, &format!("{base}/missing.onnx")),
            Err(ureq::Error::StatusCode(404))
        ));
        assert_eq!(
            server.join().unwrap(),
            ["/model.onnx", "/model.onnx", "/model.onnx", "/missing.onnx"]
        );
    }

    /// A connection that goes quiet is dropped and resumed rather than
    /// waited on forever, and a download left part-way by closing the app
    /// carries on from where it was.
    #[test]
    fn stalled_downloads_resume() {
        use std::net::TcpListener;
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 241) as u8).collect();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.onnx", listener.local_addr().unwrap());
        let served = body.clone();
        let server = std::thread::spawn(move || {
            let mut asked = Vec::new();
            let mut quiet = Vec::new();
            for (n, stream) in listener.incoming().take(2).enumerate() {
                let mut stream = stream.unwrap();
                let mut req = [0u8; 4096];
                let got = stream.read(&mut req).unwrap();
                let req = String::from_utf8_lossy(&req[..got]).to_lowercase();
                let start: usize = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.trim().trim_end_matches('-').parse().ok())
                    .unwrap();
                let all = served.len();
                let head = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{all}\r\n\r\n",
                    all - start,
                    all - 1
                );
                stream.write_all(head.as_bytes()).unwrap();
                if n == 0 {
                    // A little, then silence with the connection still open.
                    stream.write_all(&served[start..start + 1000]).unwrap();
                    quiet.push(stream);
                } else {
                    stream.write_all(&served[start..]).unwrap();
                }
                asked.push(start);
            }
            asked
        });

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("model.onnx");
        // Where the last run got to before the app was closed...
        fs::write(dir.path().join("model.part"), &body[..50_000]).unwrap();
        // ...and what earlier versions left behind.
        fs::write(dir.path().join("model.4321.part"), b"old").unwrap();

        let started = std::time::Instant::now();
        let len = body.len() as u64;
        fetch(
            &agent(Duration::from_secs(1)),
            &url,
            &path,
            len,
            &mut |_| {},
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the stall was noticed"
        );
        assert_eq!(fs::read(&path).unwrap(), body);
        assert_eq!(server.join().unwrap(), [50_000, 51_000]);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    fn model_dir() -> PathBuf {
        std::env::var("FUJICLOUD_MODEL_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("fujicloud-models"))
    }

    /// The real model, end to end. Downloads ~200 MB, so it only runs when
    /// asked: `FUJICLOUD_MODEL_DIR=… cargo test -p fuji-core -- --ignored`.
    #[test]
    #[ignore]
    fn real_siglip_matches_colours_and_meaning() {
        let dir = model_dir();
        SIGLIP_BASE
            .download(&dir, Device::Cpu, &|_, _| {})
            .expect("download");
        check_real(&Engine::load(&dir, &SIGLIP_BASE, Device::Cpu, 4).expect("load"));
    }

    /// The GPU path: DirectML with the half-precision model. On a machine
    /// with a graphics card this uses it, exactly as the app does. CI
    /// machines have none, so there it runs DirectML on WARP, Windows'
    /// software renderer: slow, but the same code path.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn real_siglip_on_directml() {
        let dir = model_dir();
        SIGLIP_BASE
            .download(&dir, Device::Gpu(None), &|_, _| {})
            .expect("download");
        let engine = if let Some(gpu) = best_gpu() {
            println!("DirectML on {gpu:?}");
            Engine::load(&dir, &SIGLIP_BASE, Device::Gpu(None), 4)
        } else {
            println!("DirectML on WARP (no GPU)");
            let spec = &SIGLIP_BASE;
            Engine::load_with(
                &spec.vision_file(&dir, Device::Gpu(None)).unwrap(),
                &spec.local(&dir, spec.text),
                &spec.local(&dir, spec.tokenizer),
                spec,
                Device::Gpu(None),
                4,
                &warp_directml,
            )
        }
        .expect("DirectML starts");
        println!("vision input is fp16: {}", engine.vision_f16);
        check_real(&engine);

        // The GPU's half-precision model and the CPU's 8-bit one round
        // differently, but a library indexed partly on each must still work:
        // every picture has to be closest to its own counterpart.
        SIGLIP_BASE
            .download(&dir, Device::Cpu, &|_, _| {})
            .expect("download");
        let cpu = Engine::load(&dir, &SIGLIP_BASE, Device::Cpu, 4).unwrap();
        let pictures = [
            solid([220, 30, 30]),
            solid([30, 60, 220]),
            solid([200, 120, 40]),
            // Sky over a dark hill, with a sun.
            picture(|x, y| {
                let (dx, dy) = (x as f32 - 44.0, y as f32 - 12.0);
                if dx * dx + dy * dy < 36.0 {
                    [255, 236, 170]
                } else if y as f32 > 30.0 + (x as f32 / 9.0).sin() * 4.0 {
                    [30, 70, 40]
                } else {
                    [110 + y as u8 * 3, 160 + y as u8 * 2, 230]
                }
            }),
            picture(|x, y| {
                if (x / 8 + y / 8) % 2 == 0 {
                    [0; 3]
                } else {
                    [255; 3]
                }
            }),
        ];
        let gpu = engine.embed_images(&pictures).unwrap();
        let cpu = cpu.embed_images(&pictures).unwrap();
        let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
        for (i, g) in gpu.iter().enumerate() {
            let sims: Vec<f32> = cpu.iter().map(|c| dot(g, c)).collect();
            println!("GPU picture {i} vs CPU: {sims:.3?}");
            let nearest = (0..sims.len())
                .max_by(|&a, &b| sims[a].total_cmp(&sims[b]))
                .unwrap();
            assert_eq!(nearest, i, "GPU picture {i} matched CPU picture {nearest}");
        }
    }

    /// ONNX Runtime refuses to pick a software adapter itself, so this makes
    /// the D3D12 and DirectML devices on WARP and hands them over.
    #[cfg(windows)]
    fn warp_directml(mut builder: SessionBuilder) -> Result<SessionBuilder, String> {
        use ort::AsPointer;
        use windows::Win32::AI::MachineLearning::DirectML::{
            DML_CREATE_DEVICE_FLAG_NONE, DML_FEATURE_LEVEL_5_0, DMLCreateDevice1, IDMLDevice,
        };
        use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_COMMAND_LIST_TYPE_DIRECT, D3D12_COMMAND_QUEUE_DESC,
            D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT, D3D12CreateDevice, ID3D12CommandQueue,
            ID3D12Device,
        };
        use windows::Win32::Graphics::Dxgi::{
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter, IDXGIFactory4,
        };
        use windows::core::Interface;

        let api = ort::ep::directml::api().ok_or("ONNX Runtime was built without DirectML")?;
        let err = |e: windows::core::Error| e.to_string();
        unsafe {
            let factory: IDXGIFactory4 =
                CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).map_err(err)?;
            let adapter: IDXGIAdapter = factory.EnumWarpAdapter().map_err(err)?;
            let mut device: Option<ID3D12Device> = None;
            D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_11_0, &mut device).map_err(err)?;
            let device = device.ok_or("no D3D12 device")?;
            let queue: ID3D12CommandQueue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                    Flags: D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT,
                    ..Default::default()
                })
                .map_err(err)?;
            let mut dml: Option<IDMLDevice> = None;
            DMLCreateDevice1(
                &device,
                DML_CREATE_DEVICE_FLAG_NONE,
                DML_FEATURE_LEVEL_5_0,
                &mut dml,
            )
            .map_err(err)?;
            let dml = dml.ok_or("no DirectML device")?;
            // ONNX Runtime takes its own references to both.
            ort::Error::result_from_status((api.SessionOptionsAppendExecutionProvider_DML1)(
                builder.ptr_mut(),
                dml.as_raw(),
                queue.as_raw(),
            ))
            .map_err(|e| e.to_string())?;
        }
        dml_options(builder)
    }
}
