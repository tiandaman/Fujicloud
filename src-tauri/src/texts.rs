//! Reading the text in photos in the background, so it can be searched:
//! downloads the small reader on request, then works through every photo
//! not read yet.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fuji_core::text::{self, MODEL_ID, Reader};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;
use crate::faces::Picture;
use fuji_core::thumbs::{self, Size};

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Off,
    Downloading,
    Indexing,
    Ready,
    Error,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub phase: Phase,
    /// Bytes while downloading, photos while reading.
    pub done: u64,
    pub total: u64,
    pub download_mb: u32,
    /// Photos read so far, and how many had something written in them.
    pub read: usize,
    pub with_text: usize,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Texts {
    reader: Mutex<Option<Arc<Reader>>>,
    status: Mutex<Status>,
    running: AtomicBool,
    again: AtomicBool,
    /// Photos that couldn't be read; not retried this session.
    failed: Mutex<HashSet<i64>>,
}

impl Texts {
    pub fn status(&self, state: &AppState) -> Status {
        let mut s = self.status.lock().clone();
        s.enabled = state.settings.read().read_text;
        s.download_mb = text::DOWNLOAD_MB;
        (s.read, s.with_text) = state.lib.texts_read(MODEL_ID).unwrap_or((0, 0));
        if !s.enabled {
            s.phase = Phase::Off;
        } else if s.phase == Phase::Off {
            s.phase = if text::is_downloaded(&state.models_dir) {
                Phase::Ready
            } else {
                Phase::Downloading
            };
        }
        s
    }

    fn update(&self, app: &AppHandle, state: &AppState, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock());
        let _ = app.emit("texts", self.status(state));
    }

    fn reader(&self, state: &AppState) -> Result<Arc<Reader>, String> {
        let mut slot = self.reader.lock();
        if let Some(r) = slot.as_ref() {
            return Ok(r.clone());
        }
        let threads = crate::cpu::threads(crate::cpu::Analysis::Text);
        let reader = Arc::new(Reader::load(&state.models_dir, threads).map_err(|e| e.to_string())?);
        *slot = Some(reader.clone());
        Ok(reader)
    }

    /// Photos that couldn't be read this session.
    pub fn failures(&self) -> usize {
        self.failed.lock().len()
    }

    /// Lets photos that couldn't be read be tried again.
    pub fn forget_failures(&self) {
        self.failed.lock().clear();
    }

    pub fn disable(&self, app: &AppHandle, state: &AppState) {
        *self.reader.lock() = None;
        self.update(app, state, |s| *s = Status::default());
    }
}

/// Downloads the reader if needed, then reads every photo not read yet.
/// Extra calls while a run is going make it look once more.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    if !state.settings.read().read_text {
        return;
    }
    let texts = &state.texts;
    if texts.running.swap(true, Ordering::AcqRel) {
        texts.again.store(true, Ordering::Release);
        return;
    }
    std::thread::Builder::new()
        .name("fuji-text".into())
        .spawn(move || {
            let texts = &state.texts;
            loop {
                texts.again.store(false, Ordering::Release);
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app, &state)))
                        .unwrap_or_else(|_| {
                            Err(
                                "Reading text hit an internal error. Details are in the log file."
                                    .into(),
                            )
                        });
                if let Err(e) = result {
                    log::warn!("text: {e}");
                    texts.update(&app, &state, |s| {
                        s.phase = Phase::Error;
                        s.error = Some(e);
                    });
                    break;
                }
                if !texts.again.load(Ordering::Acquire) {
                    break;
                }
            }
            // Nothing left to read: give the memory back until there is.
            *texts.reader.lock() = None;
            texts.running.store(false, Ordering::Release);
        })
        .expect("spawn text thread");
}

fn run(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let texts = &state.texts;
    if !text::is_downloaded(&state.models_dir) {
        texts.update(app, state, |s| {
            s.phase = Phase::Downloading;
            s.error = None;
        });
        let last = Mutex::new(Instant::now());
        text::download(&state.models_dir, &|done, total| {
            let mut last = last.lock();
            if last.elapsed() > Duration::from_millis(250) || (total > 0 && done == total) {
                *last = Instant::now();
                texts.update(app, state, |s| (s.done, s.total) = (done, total));
            }
        })
        .map_err(|e| {
            format!(
                "{e}. Check that this PC can reach huggingface.co, then switch reading text off and on to try again."
            )
        })?;
    }
    // Loaded only once there's a photo to read: runs after every scan mostly
    // find nothing new.
    let mut reader = None;

    let failed = texts.failed.lock().clone();
    let mut todo: Vec<i64> = state
        .lib
        .text_todo(MODEL_ID)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|id| !failed.contains(id))
        .collect();
    // And again, the ones read from a thumbnail that have since been opened
    // (so a bigger copy is kept): small writing shows in that.
    let fresh = todo.len();
    todo.extend(
        state
            .lib
            .text_read_small(MODEL_ID)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|id| !failed.contains(id) && has_preview(state, *id)),
    );
    if todo.len() > fresh {
        log::info!(
            "text: reading {} photos again from their bigger copies",
            todo.len() - fresh
        );
    }
    let total = todo.len() as u64;
    texts.update(app, state, |s| {
        s.phase = Phase::Indexing;
        (s.done, s.total, s.error) = (0, total, None);
    });

    for (n, &id) in todo.iter().enumerate() {
        if !state.settings.read().read_text {
            return Ok(());
        }
        match picture(state, id) {
            Picture::Ready(image) => {
                let reader = match &reader {
                    Some(r) => r,
                    None => reader.insert(texts.reader(state)?),
                };
                let edge = image.width.max(image.height);
                match fuji_core::guard(|| reader.read(&image)) {
                    Ok(lines) => {
                        state
                            .lib
                            .store_text(id, MODEL_ID, &lines, edge)
                            .map_err(|e| e.to_string())?;
                        // So other PCs needn't read it again. (Most photos
                        // have nothing written in them: no need to say so.)
                        if !lines.is_empty() {
                            crate::sidecars::save(&[id]);
                        }
                    }
                    Err(e) => {
                        log::warn!("text in photo {id}: {e}");
                        texts.failed.lock().insert(id);
                    }
                }
            }
            // Read once its source is back.
            Picture::Offline => {}
            Picture::Failed => {
                texts.failed.lock().insert(id);
            }
        }
        if n % 10 == 9 || n + 1 == todo.len() {
            texts.update(app, state, |s| s.done = n as u64 + 1);
        }
    }
    texts.update(app, state, |s| {
        s.phase = Phase::Ready;
        (s.done, s.total) = (total, total);
    });
    Ok(())
}

/// The picture edge photos are read at: enough for small writing.
const READ_EDGE: u32 = 1280;

fn source_of(state: &AppState, id: i64) -> Option<(std::path::PathBuf, fuji_core::Kind, u64, i64)> {
    state.lib.file_of(id).ok()
}

/// Whether the photo's bigger copy (made when it's opened) is kept.
fn has_preview(state: &AppState, id: i64) -> bool {
    source_of(state, id).is_some_and(|(path, kind, size, mtime)| {
        let src = thumbs::Source {
            path: &path,
            kind,
            size,
            mtime,
        };
        state.cache.get(&src, Size::Preview).is_some()
    })
}

/// The photo as an upright picture to read: its bigger copy when there is
/// one, else the thumbnail. A screenshot's bigger copy is made if need be:
/// its writing is small and it's what's there to read (and the file's small).
fn picture(state: &AppState, id: i64) -> Picture {
    let Some((path, kind, size, mtime)) = source_of(state, id) else {
        return Picture::Failed;
    };
    let src = thumbs::Source {
        path: &path,
        kind,
        size,
        mtime,
    };
    let offline = state.reach.covers(&path);
    let screenshot = kind == fuji_core::Kind::Png && !offline;
    let cached = state
        .cache
        .get(&src, Size::Preview)
        .or_else(|| {
            screenshot
                .then(|| state.cache.ensure(&src, Size::Preview).ok())
                .flatten()
        })
        .or_else(|| state.cache.get(&src, Size::Thumb));
    let file = match cached {
        Some(file) => file,
        None if offline => return Picture::Offline,
        None => match state.cache.ensure(&src, Size::Thumb) {
            Ok(file) => file,
            Err(_) => return Picture::Failed,
        },
    };
    std::fs::read(file)
        .ok()
        .and_then(|jpeg| thumbs::decode_rendition(&jpeg, READ_EDGE).ok())
        .map_or(Picture::Failed, Picture::Ready)
}
