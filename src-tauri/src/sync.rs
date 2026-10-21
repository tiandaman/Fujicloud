//! Camera → laptop → Shield sync.
//!
//! The camera sends shots over Wi-Fi into a local "inbox" folder (FUJIFILM
//! PC AutoSave, or any app that drops files into a folder). We watch it,
//! wait for each file to finish arriving, then copy it into the library on
//! the Shield's SSD several files at a time. Thumbnails and previews are
//! rendered from the local copy *while* the network copy runs, so a new shot
//! is fully browsable the moment it lands.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fuji_core::thumbs::{self, Renditions};
use fuji_core::{Kind, ingest, raf, scan};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use rayon::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;

/// How long a file's size must stay unchanged before we treat it as complete.
const SETTLE: Duration = Duration::from_millis(1500);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub active: bool,
    pub inbox: String,
    pub library: String,
    pub copying: usize,
    pub done: usize,
    pub total: usize,
    pub last_imported: usize,
    pub error: Option<String>,
}

pub struct InboxSync {
    _watcher: RecommendedWatcher,
    stop: Arc<AtomicBool>,
}

impl Drop for InboxSync {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn start(
    app: AppHandle,
    state: Arc<AppState>,
    inbox: PathBuf,
    library: PathBuf,
) -> notify::Result<InboxSync> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    })?;
    watcher.watch(&inbox, RecursiveMode::Recursive)?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();
    std::thread::Builder::new()
        .name("fuji-sync".into())
        .spawn(move || run(app, state, inbox, library, rx, stop_flag))
        .expect("spawn sync thread");
    Ok(InboxSync {
        _watcher: watcher,
        stop,
    })
}

fn run(
    app: AppHandle,
    state: Arc<AppState>,
    inbox: PathBuf,
    library: PathBuf,
    rx: mpsc::Receiver<notify::Event>,
    stop: Arc<AtomicBool>,
) {
    let workers = state.settings.read().parallel_copies.clamp(1, 16);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|i| format!("fuji-copy-{i}"))
        .build()
        .expect("copy pool");
    let mut status = SyncStatus {
        active: true,
        inbox: inbox.to_string_lossy().into_owned(),
        library: library.to_string_lossy().into_owned(),
        copying: 0,
        done: 0,
        total: 0,
        last_imported: 0,
        error: None,
    };
    let _ = app.emit("sync", &status);

    // Anything that arrived while the app was closed.
    let mut pending: HashMap<PathBuf, (u64, Instant)> = scan::walk(&inbox)
        .into_iter()
        .map(|f| (f.path, (f.size, Instant::now())))
        .collect();

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(event) if is_interesting(&event.kind) => {
                for path in event.paths {
                    if Kind::from_path(&path).is_some()
                        && let Ok(md) = std::fs::metadata(&path)
                    {
                        pending.insert(path, (md.len(), Instant::now()));
                    }
                }
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        let mut ready = Vec::new();
        pending.retain(|path, (size, seen)| {
            if now.duration_since(*seen) < SETTLE {
                return true;
            }
            match std::fs::metadata(path) {
                Ok(md) if md.len() == *size && md.len() > 0 => {
                    ready.push(path.clone());
                    false
                }
                Ok(md) => {
                    // Still being written.
                    (*size, *seen) = (md.len(), now);
                    true
                }
                Err(_) => false,
            }
        });
        if ready.is_empty() {
            continue;
        }

        // RAW before JPEG keeps pairs together in one batch more often, and
        // sorting by name keeps the copy order predictable.
        ready.sort();
        status.total = ready.len();
        status.done = 0;
        status.copying = ready.len();
        status.error = None;
        let _ = app.emit("sync", &status);

        match pool.install(|| import_batch(&app, &state, &ready, &library, &status)) {
            Ok(imported) => {
                status.last_imported = imported;
                // Make the new shots findable by description too.
                crate::smart::spawn(app.clone(), state.clone());
            }
            Err(e) => status.error = Some(e),
        }
        status.copying = 0;
        status.done = status.total;
        let _ = app.emit("sync", &status);
    }
}

fn is_interesting(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
    )
}

fn import_batch(
    app: &AppHandle,
    state: &AppState,
    files: &[PathBuf],
    library: &Path,
    status: &SyncStatus,
) -> Result<usize, String> {
    let source = match state.lib.source_for(library).map_err(|e| e.to_string())? {
        Some(s) => s,
        None => state.lib.add_source(library).map_err(|e| e.to_string())?,
    };
    let done = AtomicUsize::new(0);
    let copied = AtomicUsize::new(0);

    let landed: Vec<PathBuf> = files
        .par_iter()
        .filter_map(|file| {
            let result = import_one(state, file, library);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            let _ = app.emit(
                "sync",
                SyncStatus {
                    done: n,
                    copying: files.len() - n,
                    ..status.clone()
                },
            );
            match result {
                Ok(Some((dest, was_new))) => {
                    if was_new {
                        copied.fetch_add(1, Ordering::Relaxed);
                    }
                    Some(dest)
                }
                Ok(None) => None,
                Err(e) => {
                    log::warn!("import {}: {e}", file.display());
                    None
                }
            }
        })
        .collect();

    if !landed.is_empty() {
        scan::index_files(&state.lib, source.id, &landed).map_err(|e| e.to_string())?;
        let _ = app.emit("library-changed", ());
    }
    Ok(copied.into_inner())
}

/// Returns the library path the file now lives at, and whether we copied it
/// (`None` when it was already imported and indexed earlier).
fn import_one(
    state: &AppState,
    file: &Path,
    library: &Path,
) -> Result<Option<(PathBuf, bool)>, fuji_core::Error> {
    match ingest::target(file, library)? {
        ingest::Target::Exists(dest) => Ok((!state.lib.contains(&dest)?).then_some((dest, false))),
        ingest::Target::New(dest) => {
            let kind = Kind::from_path(file).expect("inbox only queues photos");
            let (renditions, copied) =
                rayon::join(|| render_local(file, kind), || ingest::copy(file, &dest));
            copied?;
            if let (Some(renditions), Some(found)) = (renditions, scan::found_file(&dest)) {
                let src = thumbs::Source {
                    path: &dest,
                    kind,
                    size: found.size,
                    mtime: found.mtime,
                };
                if let Err(e) = state.cache.store(&src, &renditions) {
                    log::warn!("caching {}: {e}", dest.display());
                }
            }
            Ok(Some((dest, true)))
        }
    }
}

fn render_local(file: &Path, kind: Kind) -> Option<Renditions> {
    let jpeg = match kind {
        Kind::Jpeg => std::fs::read(file).ok()?,
        Kind::Raf => raf::read_embedded_jpeg(file).ok()?,
        _ => return None,
    };
    thumbs::render_all(&jpeg).ok()
}
