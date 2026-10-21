//! Smart search orchestration: download the model on request, embed every
//! photo in the background from its cached thumbnail, and answer queries.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use fuji_core::embed::{self, Device, Engine, Gpu, SIGLIP_BASE as MODEL};
use fuji_core::search::{self, Index};
use fuji_core::thumbs::{self, Size};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Off,
    Downloading,
    Loading,
    Indexing,
    Ready,
    Error,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub phase: Phase,
    /// Bytes while downloading, photos while indexing.
    pub done: u64,
    pub total: u64,
    pub indexed: i64,
    pub download_mb: u32,
    pub error: Option<String>,
    /// The graphics card found on this PC, if any.
    pub gpu: Option<Gpu>,
    /// Whether the user allows using it.
    pub use_gpu: bool,
    /// What photos are being analysed on right now, e.g. "GPU" or "CPU".
    pub running_on: Option<String>,
    /// Why the GPU isn't being used, if it was wanted but failed.
    pub gpu_error: Option<String>,
}

#[derive(Default)]
pub struct Smart {
    engine: Mutex<Option<Arc<Engine>>>,
    index: Mutex<Option<Arc<Index>>>,
    status: Mutex<Status>,
    running: AtomicBool,
    again: AtomicBool,
    /// Photos whose thumbnail couldn't be read; not retried this session.
    failed: Mutex<HashSet<i64>>,
    gpu: OnceLock<Option<Gpu>>,
    /// DirectML failed to start; stay on the CPU until settings change.
    gpu_error: Mutex<Option<String>>,
}

impl Smart {
    pub fn status(&self, state: &AppState) -> Status {
        let mut s = self.status.lock().clone();
        let settings = state.settings.read();
        s.enabled = settings.smart_search;
        s.use_gpu = settings.smart_gpu;
        drop(settings);
        s.gpu = self.gpu().cloned();
        s.gpu_error = self.gpu_error.lock().clone();
        s.download_mb = MODEL.download_mb(self.preferred(state));
        s.indexed = state.lib.embedding_count(MODEL.id).unwrap_or(0);
        // Never wait here: status is polled from the UI while a model loads.
        s.running_on = self
            .engine
            .try_lock()
            .and_then(|e| e.as_ref().map(|e| e.device()))
            .map(|d| match d {
                Device::Gpu(_) => "GPU".to_owned(),
                Device::Cpu => "CPU".to_owned(),
            });
        if !s.enabled {
            s.phase = Phase::Off;
        } else if s.phase == Phase::Off {
            s.phase = if MODEL.is_downloaded(&state.models_dir, self.preferred(state)) {
                Phase::Ready
            } else {
                Phase::Downloading
            };
        }
        s
    }

    /// The GPU, detected once (DXGI on Windows).
    fn gpu(&self) -> Option<&Gpu> {
        self.gpu.get_or_init(embed::best_gpu).as_ref()
    }

    /// Where we'd like to run: the GPU if there is one, it's allowed, and it
    /// hasn't failed to start this session.
    fn preferred(&self, state: &AppState) -> Device {
        if state.settings.read().smart_gpu
            && self.gpu().is_some()
            && self.gpu_error.lock().is_none()
        {
            Device::Gpu(None)
        } else {
            Device::Cpu
        }
    }

    /// Forgets the loaded model so the next use picks the device afresh.
    pub fn reset_device(&self) {
        *self.engine.lock() = None;
        *self.gpu_error.lock() = None;
    }

    fn update(&self, app: &AppHandle, state: &AppState, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock());
        let _ = app.emit("smart", self.status(state));
    }

    /// The loaded model: on the GPU when preferred and DirectML starts,
    /// otherwise on the CPU (if that variant is downloaded).
    fn engine(&self, state: &AppState) -> Result<Arc<Engine>, String> {
        let mut slot = self.engine.lock();
        if let Some(engine) = slot.as_ref() {
            return Ok(engine.clone());
        }
        let dir = &state.models_dir;
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 2);
        let mut engine = None;
        if self.preferred(state).is_gpu() && MODEL.is_downloaded(dir, Device::Gpu(None)) {
            match Engine::load(dir, &MODEL, Device::Gpu(None), threads) {
                Ok(e) => engine = Some(e),
                Err(e) => {
                    log::warn!("GPU unavailable, using the CPU: {e}");
                    *self.gpu_error.lock() = Some(e.to_string());
                }
            }
        }
        let engine = match engine {
            Some(e) => e,
            None if MODEL.is_downloaded(dir, Device::Cpu) => {
                Engine::load(dir, &MODEL, Device::Cpu, threads).map_err(|e| e.to_string())?
            }
            None => return Err("The smart search model isn't downloaded yet.".into()),
        };
        let engine = Arc::new(engine);
        *slot = Some(engine.clone());
        Ok(engine)
    }

    fn index(&self, state: &AppState) -> Result<Arc<Index>, String> {
        let mut slot = self.index.lock();
        if let Some(index) = slot.as_ref() {
            return Ok(index.clone());
        }
        let index = Arc::new(Index::new(
            state.lib.embeddings(MODEL.id).map_err(|e| e.to_string())?,
        ));
        *slot = Some(index.clone());
        Ok(index)
    }

    /// Best matches for a typed description, best first.
    pub fn search(&self, state: &AppState, query: &str) -> Result<Vec<(i64, f32)>, String> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let engine = self.engine(state)?;
        let q = engine.embed_text(query).map_err(|e| e.to_string())?;
        // SigLIP's text↔image scores are small and tightly bunched; keep the
        // ones close to the best match.
        // Measured with the real model: matching image/text pairs score
        // ~0.11–0.12 and non-matches 0.02–0.06, so 0.05 below the best
        // keeps real matches and drops the rest.
        Ok(self.index(state)?.search(&q, 600, 0.05))
    }

    /// Photos that look like `id`, excluding itself.
    pub fn similar(&self, state: &AppState, id: i64) -> Result<Vec<(i64, f32)>, String> {
        let index = self.index(state)?;
        let Some(v) = index.vector(id) else {
            return Err("This photo hasn't been analysed yet. Try again in a moment.".into());
        };
        let mut hits = index.search(&v, 301, 0.3);
        hits.retain(|(other, _)| *other != id);
        Ok(hits)
    }

    pub fn disable(&self, app: &AppHandle, state: &AppState) {
        *self.engine.lock() = None;
        *self.index.lock() = None;
        self.update(app, state, |s| *s = Status::default());
    }
}

/// Downloads the model if needed, then embeds every photo that isn't yet.
/// Safe to call often (after each scan or import): extra calls while a run
/// is in progress just make it look for new photos once more at the end.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    if !state.settings.read().smart_search {
        return;
    }
    let smart = &state.smart;
    if smart.running.swap(true, Ordering::AcqRel) {
        smart.again.store(true, Ordering::Release);
        return;
    }
    std::thread::Builder::new()
        .name("fuji-smart".into())
        .spawn(move || {
            let smart = &state.smart;
            loop {
                smart.again.store(false, Ordering::Release);
                if let Err(e) = run(&app, &state) {
                    log::warn!("smart search: {e}");
                    smart.update(&app, &state, |s| {
                        s.phase = Phase::Error;
                        s.error = Some(e);
                    });
                    break;
                }
                if !smart.again.load(Ordering::Acquire) {
                    break;
                }
            }
            smart.running.store(false, Ordering::Release);
        })
        .expect("spawn smart search thread");
}

fn download(app: &AppHandle, state: &AppState, device: Device) -> Result<(), String> {
    let smart = &state.smart;
    if MODEL.is_downloaded(&state.models_dir, device) {
        return Ok(());
    }
    smart.update(app, state, |s| {
        s.phase = Phase::Downloading;
        s.error = None;
    });
    let last = Mutex::new(std::time::Instant::now());
    MODEL
        .download(&state.models_dir, device, &|done, total| {
            let mut last = last.lock();
            if last.elapsed().as_millis() > 250 || (total > 0 && done == total) {
                *last = std::time::Instant::now();
                smart.update(app, state, |s| (s.done, s.total) = (done, total));
            }
        })
        .map_err(|e| e.to_string())
}

fn run(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let smart = &state.smart;
    download(app, state, smart.preferred(state))?;
    smart.update(app, state, |s| s.phase = Phase::Loading);
    let engine = match smart.engine(state) {
        Ok(engine) => engine,
        // DirectML didn't start: fetch the CPU variant and carry on there.
        Err(_) if smart.gpu_error.lock().is_some() => {
            download(app, state, Device::Cpu)?;
            smart.update(app, state, |s| s.phase = Phase::Loading);
            smart.engine(state)?
        }
        Err(e) => return Err(e),
    };
    let batch = engine.batch_size();

    let failed = smart.failed.lock().clone();
    let todo: Vec<i64> = state
        .lib
        .missing_embeddings(MODEL.id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|id| !failed.contains(id))
        .collect();
    let total = todo.len() as u64;
    smart.update(app, state, |s| {
        s.phase = Phase::Indexing;
        (s.done, s.total) = (0, total);
    });

    for (n, chunk) in todo.chunks(batch).enumerate() {
        if !state.settings.read().smart_search {
            return Ok(());
        }
        let mut ids = Vec::with_capacity(chunk.len());
        let mut jpegs = Vec::with_capacity(chunk.len());
        for &id in chunk {
            match thumbnail(state, id) {
                Some(bytes) => {
                    ids.push(id);
                    jpegs.push(bytes);
                }
                None => {
                    smart.failed.lock().insert(id);
                }
            }
        }
        let vectors = match engine.embed_images(&jpegs) {
            Ok(v) => v,
            // One bad image shouldn't sink its batch: retry individually.
            Err(_) => {
                let mut ok = Vec::new();
                let mut kept = Vec::new();
                for (id, jpeg) in ids.iter().zip(&jpegs) {
                    match engine.embed_images(std::slice::from_ref(jpeg)) {
                        Ok(mut v) => {
                            kept.push(*id);
                            ok.push(v.remove(0));
                        }
                        Err(_) => {
                            smart.failed.lock().insert(*id);
                        }
                    }
                }
                ids = kept;
                ok
            }
        };
        let rows: Vec<(i64, Vec<u8>)> = ids
            .into_iter()
            .zip(vectors.iter().map(|v| search::quantize(v)))
            .collect();
        state
            .lib
            .store_embeddings(MODEL.id, &rows)
            .map_err(|e| e.to_string())?;

        // New results become searchable every few hundred photos.
        if (n + 1) * batch % 320 < batch {
            *smart.index.lock() = None;
        }
        let done = ((n + 1) * batch).min(todo.len()) as u64;
        smart.update(app, state, |s| s.done = done);
    }
    *smart.index.lock() = None;
    smart.update(app, state, |s| {
        s.phase = Phase::Ready;
        (s.done, s.total) = (total, total);
    });
    Ok(())
}

fn thumbnail(state: &AppState, id: i64) -> Option<Vec<u8>> {
    let (path, kind, size, mtime) = state.lib.file_of(id).ok()?;
    let src = thumbs::Source {
        path: &path,
        kind,
        size,
        mtime,
    };
    let cached = state.cache.ensure(&src, Size::Thumb).ok()?;
    std::fs::read(cached).ok()
}
