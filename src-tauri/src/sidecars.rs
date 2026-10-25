//! Saves XMP sidecars in the background, one batch at a time, so rating
//! quickly through a shoot never waits on the network. Sidecars that can't
//! be written (the Shield is off) stay marked and are retried by [`flush`].

use std::sync::mpsc;
use std::sync::{Arc, OnceLock};

use fuji_core::scan;

use crate::AppState;

static QUEUE: OnceLock<mpsc::Sender<Vec<i64>>> = OnceLock::new();

pub fn start(state: Arc<AppState>) {
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
                match fuji_core::guard(|| scan::save_sidecars(&state.lib, &ids)) {
                    Ok(0) => {}
                    Ok(n) => log::info!("{n} sidecars will be saved when the library is reachable"),
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
