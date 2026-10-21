//! Smart search orchestration: download the model on request, embed every
//! photo in the background from its cached thumbnail, and answer queries.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use fuji_core::embed::{Engine, SIGLIP_BASE as MODEL};
use fuji_core::search::{self, Index};
use fuji_core::thumbs::{self, Size};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;

/// Photos embedded per model call.
const BATCH: usize = 8;

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
}

impl Smart {
    pub fn status(&self, state: &AppState) -> Status {
        let mut s = self.status.lock().clone();
        s.enabled = state.settings.read().smart_search;
        s.download_mb = MODEL.download_mb;
        s.indexed = state.lib.embedding_count(MODEL.id).unwrap_or(0);
        if !s.enabled {
            s.phase = Phase::Off;
        } else if s.phase == Phase::Off {
            s.phase = if MODEL.is_downloaded(&state.models_dir) {
                Phase::Ready
            } else {
                Phase::Downloading
            };
        }
        s
    }

    fn update(&self, app: &AppHandle, state: &AppState, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock());
        let _ = app.emit("smart", self.status(state));
    }

    fn engine(&self, state: &AppState) -> Result<Arc<Engine>, String> {
        let mut slot = self.engine.lock();
        if let Some(engine) = slot.as_ref() {
            return Ok(engine.clone());
        }
        let threads = std::thread::available_parallelism().map_or(2, |n| n.get() / 2);
        let engine =
            Arc::new(Engine::load(&state.models_dir, &MODEL, threads).map_err(|e| e.to_string())?);
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
        Ok(self.index(state)?.search(&q, 600, 0.07))
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

fn run(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let smart = &state.smart;
    if !MODEL.is_downloaded(&state.models_dir) {
        smart.update(app, state, |s| {
            s.phase = Phase::Downloading;
            s.error = None;
        });
        let last = Mutex::new(std::time::Instant::now());
        MODEL
            .download(&state.models_dir, &|done, total| {
                let mut last = last.lock();
                if last.elapsed().as_millis() > 250 || (total > 0 && done == total) {
                    *last = std::time::Instant::now();
                    smart.update(app, state, |s| (s.done, s.total) = (done, total));
                }
            })
            .map_err(|e| e.to_string())?;
    }

    smart.update(app, state, |s| s.phase = Phase::Loading);
    let engine = smart.engine(state)?;

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

    for (n, chunk) in todo.chunks(BATCH).enumerate() {
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
        if n % 40 == 39 {
            *smart.index.lock() = None;
        }
        let done = ((n + 1) * BATCH).min(todo.len()) as u64;
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
