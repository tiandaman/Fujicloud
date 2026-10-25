//! Camera/phone → laptop → Shield sync.
//!
//! The camera sends shots over Wi-Fi into a local "inbox" folder (FUJIFILM
//! PC AutoSave), and iCloud for Windows puts iPhone shots in another. We
//! watch them, wait for each file to finish arriving, then copy it into the
//! library on the Shield's SSD several files at a time, checking every copy
//! against the original. Thumbnails and previews are rendered from the local
//! file *while* the network copy runs, so a new shot is browsable the moment
//! it lands. When the library can't be reached, files wait and are retried.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use fuji_core::thumbs::{self, Renditions};
use fuji_core::{Kind, decode, ingest, raf, scan};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use rayon::prelude::*;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;
use crate::settings::{Inbox, is_cloud_folder};

/// How long a file's size must stay unchanged before we treat it as complete.
const SETTLE: Duration = Duration::from_millis(1500);
/// First wait before retrying files that couldn't be copied; doubles up to
/// [`MAX_RETRY`].
const RETRY: Duration = Duration::from_secs(30);
const MAX_RETRY: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub active: bool,
    pub inboxes: Vec<String>,
    pub library: String,
    pub copying: usize,
    pub done: usize,
    pub total: usize,
    pub last_imported: usize,
    /// Files waiting for the library to be reachable again.
    pub waiting: usize,
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
    inboxes: Vec<Inbox>,
    library: PathBuf,
) -> notify::Result<InboxSync> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Ok(event) = event {
            let _ = tx.send(event);
        }
    })?;
    let mut watched = Vec::new();
    for inbox in inboxes {
        match watcher.watch(&inbox.path, RecursiveMode::Recursive) {
            Ok(()) => watched.push(inbox),
            Err(e) => log::warn!("can't watch {}: {e}", inbox.path.display()),
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = stop.clone();
    std::thread::Builder::new()
        .name("fuji-sync".into())
        .spawn(move || run(app, state, watched, library, rx, stop_flag))
        .expect("spawn sync thread");
    Ok(InboxSync {
        _watcher: watcher,
        stop,
    })
}

/// A file waiting to be imported.
struct Waiting {
    size: u64,
    /// When it last changed (still arriving) or when to try it again.
    due: Instant,
    /// Retries so far, for backing off.
    tries: u32,
}

fn run(
    app: AppHandle,
    state: Arc<AppState>,
    inboxes: Vec<Inbox>,
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
        inboxes: inboxes
            .iter()
            .map(|i| i.path.to_string_lossy().into_owned())
            .collect(),
        library: library.to_string_lossy().into_owned(),
        copying: 0,
        done: 0,
        total: 0,
        last_imported: 0,
        waiting: 0,
        error: None,
    };
    let _ = app.emit("sync", &status);

    // Anything that arrived while the app was closed, minus what was
    // already imported (an iCloud folder can hold years of photos).
    let now = Instant::now();
    let mut pending: HashMap<PathBuf, Waiting> = inboxes
        .iter()
        .flat_map(|inbox| scan::walk(&inbox.path))
        .filter(|f| {
            !state
                .lib
                .was_imported(&f.path, f.size, f.mtime)
                .unwrap_or(false)
        })
        .map(|f| {
            (
                f.path,
                Waiting {
                    size: f.size,
                    due: now,
                    tries: 0,
                },
            )
        })
        .collect();

    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(Duration::from_millis(400)) {
            Ok(event) if is_interesting(&event.kind) => {
                for path in event.paths {
                    if Kind::from_path(&path).is_some()
                        && let Ok(md) = std::fs::metadata(&path)
                    {
                        pending.insert(
                            path,
                            Waiting {
                                size: md.len(),
                                due: Instant::now() + SETTLE,
                                tries: 0,
                            },
                        );
                    }
                }
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let now = Instant::now();
        let mut ready = Vec::new();
        pending.retain(|path, w| {
            if now < w.due {
                return true;
            }
            match std::fs::metadata(path) {
                Ok(md) if md.len() == w.size && md.len() > 0 => {
                    ready.push((path.clone(), w.tries));
                    false
                }
                Ok(md) => {
                    // Still being written.
                    (w.size, w.due) = (md.len(), now + SETTLE);
                    true
                }
                Err(_) => false,
            }
        });
        if ready.is_empty() {
            continue;
        }

        // Don't hammer an unreachable share file by file: one check, then
        // everything waits for the next try.
        let retry_later = |pending: &mut HashMap<PathBuf, Waiting>, files: Vec<(PathBuf, u32)>| {
            for (path, tries) in files {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                let wait = (RETRY * 2u32.pow(tries.min(4))).min(MAX_RETRY);
                pending.insert(
                    path,
                    Waiting {
                        size,
                        due: Instant::now() + wait,
                        tries: tries + 1,
                    },
                );
            }
        };
        if !library.is_dir() && std::fs::create_dir_all(&library).is_err() {
            retry_later(&mut pending, ready);
            status.waiting = pending.values().filter(|w| w.tries > 0).count();
            status.error = Some(format!(
                "Can't reach {}. {} will be copied when it's back.",
                library.display(),
                fuji_plural(status.waiting, "photo")
            ));
            let _ = app.emit("sync", &status);
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

        let files: Vec<PathBuf> = ready.iter().map(|(p, _)| p.clone()).collect();
        let (imported, failed) =
            pool.install(|| import_batch(&app, &state, &inboxes, &files, &library, &status));
        let failed: Vec<(PathBuf, u32)> = ready
            .into_iter()
            .filter(|(p, _)| failed.contains(p))
            .collect();
        status.waiting = failed.len();
        if !failed.is_empty() {
            status.error = Some(format!(
                "{} couldn't be copied yet and will be tried again.",
                fuji_plural(failed.len(), "photo")
            ));
            retry_later(&mut pending, failed);
        }
        status.last_imported = imported;
        if imported > 0 {
            // Make the new shots findable by description too.
            crate::smart::spawn(app.clone(), state.clone());
        }
        status.copying = 0;
        status.done = status.total;
        let _ = app.emit("sync", &status);
    }
}

fn fuji_plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn is_interesting(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Any
    )
}

/// Imports a batch; returns how many were newly copied, and the files that
/// failed (to retry).
fn import_batch(
    app: &AppHandle,
    state: &AppState,
    inboxes: &[Inbox],
    files: &[PathBuf],
    library: &Path,
    status: &SyncStatus,
) -> (usize, Vec<PathBuf>) {
    let source = match state.lib.source_for(library) {
        Ok(Some(s)) => s,
        _ => match state.lib.add_source(library) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("library {}: {e}", library.display());
                return (0, files.to_vec());
            }
        },
    };
    let done = AtomicUsize::new(0);
    let copied = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());

    let landed: Vec<PathBuf> = files
        .par_iter()
        .filter_map(|file| {
            let result = fuji_core::guard(|| import_one(state, file, library));
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
                Ok((dest, was_new)) => {
                    if was_new {
                        copied.fetch_add(1, Ordering::Relaxed);
                    }
                    let cleanup = inboxes.iter().any(|i| {
                        i.cleanup && file.starts_with(&i.path) && !is_cloud_folder(&i.path)
                    });
                    if cleanup {
                        tidy_inbox(file, &dest);
                    }
                    (!state.lib.contains(&dest).unwrap_or(false) || was_new).then_some(dest)
                }
                Err(e) => {
                    log::warn!("import {}: {e}", file.display());
                    failed.lock().push(file.clone());
                    None
                }
            }
        })
        .collect();

    if !landed.is_empty() {
        if let Err(e) = scan::index_files(&state.lib, source.id, &landed) {
            log::warn!("indexing imports: {e}");
        }
        let _ = app.emit("library-changed", ());
    }
    (copied.into_inner(), failed.into_inner())
}

/// Copies one file in (unless it's already there) and records it. Returns
/// where it now lives in the library and whether it was copied just now.
fn import_one(
    state: &AppState,
    file: &Path,
    library: &Path,
) -> Result<(PathBuf, bool), fuji_core::Error> {
    let md = std::fs::metadata(file)?;
    let mtime = md
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64);
    let (dest, was_new) = match ingest::target(file, library)? {
        ingest::Target::Exists(dest) => (dest, false),
        ingest::Target::New(dest) => {
            let kind = Kind::from_path(file).expect("inboxes only queue photos");
            let (renditions, copied) = rayon::join(
                || render_local(file, kind),
                || ingest::copy_verified(file, &dest),
            );
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
            (dest, true)
        }
    };
    state.lib.mark_imported(file, md.len(), mtime, &dest)?;
    Ok((dest, was_new))
}

/// Moves an inbox file to the Recycle Bin once the library holds an
/// identical copy.
fn tidy_inbox(file: &Path, dest: &Path) {
    match ingest::same_content(file, dest) {
        Ok(true) => {
            if let Err(e) = trash::delete(file) {
                log::warn!("couldn't move {} to the Recycle Bin: {e}", file.display());
            }
        }
        Ok(false) => log::warn!("kept {}: the library's copy differs", file.display()),
        Err(e) => log::warn!("kept {}: {e}", file.display()),
    }
}

/// Thumbnails and previews from the fast local file, made while it uploads.
fn render_local(file: &Path, kind: Kind) -> Option<Renditions> {
    let jpeg = match kind {
        Kind::Jpeg => std::fs::read(file).ok()?,
        Kind::Raf => raf::read_embedded_jpeg(file).ok()?,
        Kind::Heif => {
            let image = decode::image(file, kind).ok()?;
            return thumbs::render_rgb_all(image).ok();
        }
        _ => return None,
    };
    thumbs::render_all(&jpeg).ok()
}
