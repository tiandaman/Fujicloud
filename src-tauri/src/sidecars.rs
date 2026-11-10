//! Saves XMP sidecars in the background, one batch at a time, so rating
//! quickly through a shoot never waits on the network. Sidecars that can't
//! be written (the Shield is off) stay marked and are retried by [`flush`].

use std::sync::mpsc;
use std::sync::{Arc, OnceLock};

use fuji_core::scan;
use tauri::Emitter;

use crate::AppState;

static QUEUE: OnceLock<mpsc::Sender<Vec<i64>>> = OnceLock::new();

pub fn start(app: tauri::AppHandle, state: Arc<AppState>) {
    let (tx, rx) = mpsc::channel::<Vec<i64>>();
    if QUEUE.set(tx).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("fuji-sidecars".into())
        .spawn(move || {
            while let Ok(mut ids) = rx.recv() {
                // Coalesce everything queued meanwhile into one pass.
                while let Ok(more) = rx.try_recv() {
                    ids.extend(more);
                }
                ids.sort_unstable();
                ids.dedup();
                if !state.settings.read().write_xmp {
                    continue;
                }
                // A share that's switched off would be waited on file by
                // file: those stay owed, and are saved when it's back.
                let (ids, away): (Vec<i64>, Vec<i64>) = ids.into_iter().partition(|id| {
                    state
                        .lib
                        .file_info(*id)
                        .is_ok_and(|f| !state.reach.is_offline(f.source_id))
                });
                if !away.is_empty()
                    && let Err(e) = state.lib.mark_unsaved(&away, false)
                {
                    log::warn!("sidecars: {e}");
                }
                if ids.is_empty() {
                    continue;
                }
                match fuji_core::guard(|| scan::save_sidecars(&state.lib, &ids)) {
                    Ok(saved) => {
                        if saved.failed > 0 {
                            log::info!(
                                "{} sidecars will be saved when the library is reachable",
                                saved.failed
                            );
                        }
                        // Another PC's changes, come across while saving.
                        if saved.synced + saved.held > 0 {
                            log::info!(
                                "while saving sidecars: {} photos took another PC's marks, {} held for review",
                                saved.synced,
                                saved.held
                            );
                            let _ = app.emit("library-changed", ());
                        }
                        if saved.held > 0
                            && let Ok(n) = state.lib.held_count()
                        {
                            let _ = app.emit("sync-review", n);
                        }
                    }
                    Err(e) => log::warn!("sidecars: {e}"),
                }
            }
        })
        .expect("spawn sidecar writer");
}

/// Saves these photos' sidecars soon.
pub fn save(ids: &[i64]) {
    if let Some(tx) = QUEUE.get()
        && !ids.is_empty()
    {
        let _ = tx.send(ids.to_vec());
    }
}

/// Retries sidecars that couldn't be saved earlier.
pub fn flush(state: &AppState) {
    if let Ok(ids) = state.lib.unsaved_sidecars() {
        save(&ids);
    }
}
