//! Pets orchestration: download the small animal detector on request, look
//! for cats and dogs in every photo in the background, and tell them apart
//! with the smart search model (which this switches on too).

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fuji_core::animals::{self, Detector, MODEL_ID};
use fuji_core::embed::{Device, SIGLIP_BASE};
use fuji_core::people::NewFace;
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::AppState;
use crate::faces::{Picture, picture};

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    #[default]
    Off,
    Downloading,
    /// For the smart search model to download.
    Waiting,
    Indexing,
    Ready,
    Error,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub enabled: bool,
    pub phase: Phase,
    /// Bytes while downloading, photos while looking for pets.
    pub done: u64,
    pub total: u64,
    pub download_mb: u32,
    /// Photos searched so far.
    pub scanned: usize,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Pets {
    detector: Mutex<Option<Arc<Detector>>>,
    status: Mutex<Status>,
    running: AtomicBool,
    again: AtomicBool,
    /// Photos that couldn't be read; not retried this session.
    failed: Mutex<HashSet<i64>>,
}

fn smart_model_ready(state: &AppState) -> bool {
    SIGLIP_BASE.is_downloaded(&state.models_dir, Device::Cpu)
        || SIGLIP_BASE.is_downloaded(&state.models_dir, Device::Gpu(None))
}

impl Pets {
    pub fn status(&self, state: &AppState) -> Status {
        let mut s = self.status.lock().clone();
        s.enabled = state.settings.read().pets;
        s.download_mb = animals::DOWNLOAD_MB;
        s.scanned = state.lib.pets_scanned(MODEL_ID).unwrap_or(0);
        if !s.enabled {
            s.phase = Phase::Off;
        } else if s.phase == Phase::Off {
            s.phase = if !animals::is_downloaded(&state.models_dir) {
                Phase::Downloading
            } else if !smart_model_ready(state) {
                Phase::Waiting
            } else {
                Phase::Ready
            };
        }
        s
    }

    fn update(&self, app: &AppHandle, state: &AppState, f: impl FnOnce(&mut Status)) {
        f(&mut self.status.lock());
        let _ = app.emit("pets", self.status(state));
    }

    fn detector(&self, state: &AppState) -> Result<Arc<Detector>, String> {
        let mut slot = self.detector.lock();
        if let Some(d) = slot.as_ref() {
            return Ok(d.clone());
        }
        // Leave most of the CPU for browsing.
        let threads = std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).max(1));
        let detector =
            Arc::new(Detector::load(&state.models_dir, threads).map_err(|e| e.to_string())?);
        *slot = Some(detector.clone());
        Ok(detector)
    }

    pub fn disable(&self, app: &AppHandle, state: &AppState) {
        *self.detector.lock() = None;
        self.update(app, state, |s| *s = Status::default());
    }
}

/// Downloads the detector if needed, then looks for pets in every photo not
/// searched yet. Extra calls while a run is going make it look once more.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    if !state.settings.read().pets {
        return;
    }
    let pets = &state.pets;
    if pets.running.swap(true, Ordering::AcqRel) {
        pets.again.store(true, Ordering::Release);
        return;
    }
    std::thread::Builder::new()
        .name("fuji-pets".into())
        .spawn(move || {
            let pets = &state.pets;
            loop {
                pets.again.store(false, Ordering::Release);
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(&app, &state)))
                        .unwrap_or_else(|_| {
                            Err(
                                "Finding pets hit an internal error. Details are in the log file."
                                    .into(),
                            )
                        });
                if let Err(e) = result {
                    log::warn!("pets: {e}");
                    pets.update(&app, &state, |s| {
                        s.phase = Phase::Error;
                        s.error = Some(e);
                    });
                    break;
                }
                if !pets.again.load(Ordering::Acquire) {
                    break;
                }
            }
            pets.running.store(false, Ordering::Release);
        })
        .expect("spawn pets thread");
}

fn run(app: &AppHandle, state: &AppState) -> Result<(), String> {
    let pets = &state.pets;
    if !animals::is_downloaded(&state.models_dir) {
        pets.update(app, state, |s| {
            s.phase = Phase::Downloading;
            s.error = None;
        });
        let last = Mutex::new(Instant::now());
        animals::download(&state.models_dir, &|done, total| {
            let mut last = last.lock();
            if last.elapsed() > Duration::from_millis(250) || (total > 0 && done == total) {
                *last = Instant::now();
                pets.update(app, state, |s| (s.done, s.total) = (done, total));
            }
        })
        .map_err(|e| {
            format!(
                "{e}. Check that this PC can reach media.githubusercontent.com, then switch pets off and on to try again."
            )
        })?;
    }
    // Smart search tells pets apart; it starts this again once it's ready.
    if !smart_model_ready(state) {
        pets.update(app, state, |s| {
            s.phase = Phase::Waiting;
            s.error = None;
        });
        return Ok(());
    }
    let detector = pets.detector(state)?;
    let engine = state.smart.engine(state)?;

    let failed = pets.failed.lock().clone();
    let todo: Vec<i64> = state
        .lib
        .pets_todo(MODEL_ID)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|id| !failed.contains(id))
        .collect();
    let total = todo.len() as u64;
    pets.update(app, state, |s| {
        s.phase = Phase::Indexing;
        (s.done, s.total, s.error) = (0, total, None);
    });

    let mut last_group = Instant::now();
    for (n, &id) in todo.iter().enumerate() {
        if !state.settings.read().pets {
            return Ok(());
        }
        match picture(state, id) {
            Picture::Ready(image) => {
                let found = fuji_core::guard(|| {
                    let animals = detector.animals(&image)?;
                    let crops = animals
                        .iter()
                        .map(|a| animals::crop(&image, a))
                        .collect::<Result<Vec<_>, _>>()?;
                    let vectors = if crops.is_empty() {
                        Vec::new()
                    } else {
                        engine.embed_images(&crops)?
                    };
                    Ok(animals
                        .into_iter()
                        .zip(vectors)
                        .map(|(a, embedding)| NewFace {
                            bbox: a.bbox,
                            score: a.score,
                            embedding,
                        })
                        .collect::<Vec<_>>())
                });
                match found {
                    Ok(found) => state
                        .lib
                        .store_pets(id, MODEL_ID, &found)
                        .map_err(|e| e.to_string())?,
                    Err(e) => {
                        log::warn!("pets in photo {id}: {e}");
                        pets.failed.lock().insert(id);
                    }
                }
            }
            // Looked at once its source is back.
            Picture::Offline => {}
            Picture::Failed => {
                pets.failed.lock().insert(id);
            }
        }
        // New pets appear as the library is worked through.
        if last_group.elapsed() > Duration::from_secs(20) {
            last_group = Instant::now();
            crate::faces::group(app, state)?;
        }
        if n % 10 == 9 || n + 1 == todo.len() {
            pets.update(app, state, |s| s.done = n as u64 + 1);
        }
    }
    crate::faces::group(app, state)?;
    pets.update(app, state, |s| {
        s.phase = Phase::Ready;
        (s.done, s.total) = (total, total);
    });
    Ok(())
}
