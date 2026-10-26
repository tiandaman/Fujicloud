//! Face recognition orchestration: download the two small models on
//! request, look for faces in every photo in the background (from its
//! cached preview or thumbnail, so nothing extra is read over the network),
//! and group them into people.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fuji_core::decode::Rgb;
use fuji_core::faces::{self, FaceModels, MODEL_ID};
use fuji_core::people::{NewFace, Subject};
use fuji_core::thumbs::{self, Size};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;

/// Faces are looked for at this size: enough for faces a few percent of the
/// picture across, without decoding the full preview.
const SEARCH_EDGE: u32 = 1280;

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
    /// Bytes while downloading, photos while looking for faces.
    pub done: u64,
    pub total: u64,
    pub download_mb: u32,
    /// Photos searched so far.
    pub scanned: usize,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Faces {
    models: Mutex<Option<Arc<FaceModels>>>,
    status: Mutex<Status>,
    running: AtomicBool,
    again: AtomicBool,
    /// Photos that couldn't be read; not retried this session.
    failed: Mutex<HashSet<i64>>,
}

impl Faces {
    pub fn status(&self, state: &AppState) -> Status {
        let mut s = self.status.lock().clone();
        s.enabled = state.settings.read().faces;
        s.download_mb = faces::DOWNLOAD_MB;
        s.scanned = state.lib.faces_scanned(MODEL_ID).unwrap_or(0);
        if !s.enabled {
            s.phase = Phase::Off;
        } else if s.phase == Phase::Off {
            s.phase = if faces::is_downloaded(&state.models_dir) {
                Phase::Ready
            } else {
                Phase::Downloading
            };
        }
        s
    }

    fn update(&self, app: &AppHandle, state: &AppState, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock());
        let _ = app.emit("faces", self.status(state));
    }

    fn models(&self, state: &AppState) -> Result<Arc<FaceModels>, String> {
        let mut slot = self.models.lock();
        if let Some(m) = slot.as_ref() {
            return Ok(m.clone());
        }
        // Leave most of the CPU for browsing.
        let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).max(1));
        let models =
            Arc::new(FaceModels::load(&state.models_dir, threads).map_err(|e| e.to_string())?);
        *slot = Some(models.clone());
        Ok(models)
    }

    pub fn disable(&self, app: &AppHandle, state: &AppState) {
        *self.models.lock() = None;
        self.update(app, state, |s| *s = Status::default());
    }
}

/// Downloads the models if needed, then looks for faces in every photo not
/// searched yet. Extra calls while a run is going make it look once more.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    if !state.settings.read().faces {
        return;
    }
    let faces = &state.faces;
    if faces.running.swap(true, Ordering::AcqRel) {
        faces.again.store(true, Ordering::Release);
        return;
    }
    std::thread::Builder::new()
        .name("fuji-faces".into())
        .spawn(move || {
            let faces = &state.faces;
            loop {
                faces.again.store(false, Ordering::Release);
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app, &state)))
                        .unwrap_or_else(|_| {
                            Err(
                        "Face recognition hit an internal error. Details are in the log file."
                            .into(),
                    )
                        });
                if let Err(e) = result {
                    log::warn!("faces: {e}");
                    faces.update(&app, &state, |s| {
                        s.phase = Phase::Error;
                        s.error = Some(e);
                    });
                    break;
                }
                if !faces.again.load(Ordering::Acquire) {
                    break;
                }
            }
            faces.running.store(false, Ordering::Release);
        })
        .expect("spawn face recognition thread");
}

fn run(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let faces = &state.faces;
    if !faces::is_downloaded(&state.models_dir) {
        faces.update(app, state, |s| {
            s.phase = Phase::Downloading;
            s.error = None;
        });
        let last = Mutex::new(Instant::now());
        faces::download(&state.models_dir, &|done, total| {
            let mut last = last.lock();
            if last.elapsed() > Duration::from_millis(250) || (total > 0 && done == total) {
                *last = Instant::now();
                faces.update(app, state, |s| (s.done, s.total) = (done, total));
            }
        })
        .map_err(|e| {
            format!(
                "{e}. Check that this PC can reach huggingface.co, then switch face recognition off and on to try again."
            )
        })?;
    }
    let models = faces.models(state)?;

    let failed = faces.failed.lock().clone();
    let todo: Vec<i64> = state
        .lib
        .faces_todo(MODEL_ID)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|id| !failed.contains(id))
        .collect();
    let total = todo.len() as u64;
    faces.update(app, state, |s| {
        s.phase = Phase::Indexing;
        (s.done, s.total, s.error) = (0, total, None);
    });

    let mut regroup = Regroup::new(Subject::Person);
    for (n, &id) in todo.iter().enumerate() {
        if !state.settings.read().faces {
            return Ok(());
        }
        match picture(state, id) {
            Picture::Ready(image) => {
                let found = fuji_core::guard(|| models.faces(&image));
                match found {
                    Ok(found) => {
                        let found: Vec<NewFace> = found
                            .into_iter()
                            .map(|f| NewFace {
                                bbox: f.bbox,
                                score: f.score,
                                embedding: f.embedding,
                            })
                            .collect();
                        state
                            .lib
                            .store_faces(id, MODEL_ID, &found)
                            .map_err(|e| e.to_string())?;
                    }
                    Err(e) => {
                        log::warn!("faces in photo {id}: {e}");
                        faces.failed.lock().insert(id);
                    }
                }
            }
            // Looked at once its source is back.
            Picture::Offline => {}
            Picture::Failed => {
                faces.failed.lock().insert(id);
            }
        }
        // New people appear as the library is worked through.
        regroup.run(app, state, false)?;
        if n % 10 == 9 || n + 1 == todo.len() {
            faces.update(app, state, |s| s.done = n as u64 + 1);
        }
    }
    regroup.run(app, state, true)?;
    faces.update(app, state, |s| {
        s.phase = Phase::Ready;
        (s.done, s.total) = (total, total);
    });
    Ok(())
}

/// Files new faces under people and tells the window.
pub fn group(app: &AppHandle, state: &AppState) -> Result<(), String> {
    if state.lib.group_faces().map_err(|e| e.to_string())? {
        let _ = app.emit("people-changed", ());
    }
    Ok(())
}

/// Grouping while a search works through the library: every 20 s at first,
/// but never more than a fifth of the time, as on a big library a pass takes
/// seconds.
pub(crate) struct Regroup {
    subject: Subject,
    last: Instant,
    took: Duration,
}

impl Regroup {
    pub fn new(subject: Subject) -> Self {
        Self {
            subject,
            last: Instant::now(),
            took: Duration::ZERO,
        }
    }

    /// Groups if it's time, or `now`.
    pub fn run(&mut self, app: &AppHandle, state: &AppState, now: bool) -> Result<(), String> {
        if !now && self.last.elapsed() < Duration::from_secs(20).max(self.took * 4) {
            return Ok(());
        }
        let start = Instant::now();
        let changed = state
            .lib
            .group_of(self.subject)
            .map_err(|e| e.to_string())?;
        if changed {
            let _ = app.emit("people-changed", ());
        }
        (self.last, self.took) = (Instant::now(), start.elapsed());
        Ok(())
    }
}

pub(crate) enum Picture {
    Ready(Rgb),
    Offline,
    Failed,
}

/// The photo as an upright picture: from the cached preview when there is
/// one, else the thumbnail (made now if needed).
pub(crate) fn picture(state: &AppState, id: i64) -> Picture {
    let Ok((path, kind, size, mtime)) = state.lib.file_of(id) else {
        return Picture::Failed;
    };
    let src = thumbs::Source {
        path: &path,
        kind,
        size,
        mtime,
    };
    let cached = state
        .cache
        .get(&src, Size::Preview)
        .or_else(|| state.cache.get(&src, Size::Thumb));
    let file = match cached {
        Some(file) => file,
        None if state.reach.covers(&path) => return Picture::Offline,
        None => match state.cache.ensure(&src, Size::Thumb) {
            Ok(file) => file,
            Err(_) => return Picture::Failed,
        },
    };
    std::fs::read(file)
        .ok()
        .and_then(|jpeg| thumbs::decode_rendition(&jpeg, SEARCH_EDGE).ok())
        .map_or(Picture::Failed, Picture::Ready)
}

/// A square crop around a face (or pet), for avatars: from the preview if
/// cached, else the thumbnail.
pub fn crop(state: &AppState, face_id: i64) -> Result<Vec<u8>, fuji_core::Error> {
    let (photo, bbox, subject) = state.lib.face(face_id)?;
    let (path, kind, size, mtime) = state.lib.file_of(photo)?;
    let src = thumbs::Source {
        path: &path,
        kind,
        size,
        mtime,
    };
    let file = match state
        .cache
        .get(&src, Size::Preview)
        .or_else(|| state.cache.get(&src, Size::Thumb))
    {
        Some(file) => file,
        None if state.reach.covers(&path) => {
            return Err(std::io::Error::from(std::io::ErrorKind::NotConnected).into());
        }
        None => state.cache.ensure(&src, Size::Thumb)?,
    };
    let image = thumbs::decode_rendition(&std::fs::read(file)?, SEARCH_EDGE)?;
    // A face needs room around it; a pet's box already takes in the pet.
    let margin = match subject {
        Subject::Person => 1.6,
        Subject::Pet => 1.05,
    };
    thumbs::crop_square(&image, bbox, margin, 160)
}
