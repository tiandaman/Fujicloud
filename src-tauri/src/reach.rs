//! Which sources can be reached right now.
//!
//! The library usually lives on the Shield's SSD, and the Shield sleeps,
//! reboots for updates, or is simply switched off. While a source is offline
//! its photos stay in the library and browse from the thumbnail cache;
//! anything that needs the files themselves waits until it is back, and
//! nothing is ever removed because a share went quiet.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use parking_lot::RwLock;
use tauri::{AppHandle, Emitter};

use crate::AppState;

/// How often sources are checked.
const EVERY: Duration = Duration::from_secs(20);
/// An unreachable SMB share can take half a minute to fail; we don't wait.
const PATIENCE: Duration = Duration::from_secs(6);

#[derive(Default)]
pub struct Reach {
    /// Offline sources: id and folder.
    offline: RwLock<Vec<(i64, PathBuf)>>,
}

impl Reach {
    pub fn is_offline(&self, source_id: i64) -> bool {
        self.offline.read().iter().any(|(id, _)| *id == source_id)
    }

    /// True when `path` lives in a source that is offline.
    pub fn covers(&self, path: &Path) -> bool {
        self.offline
            .read()
            .iter()
            .any(|(_, root)| path.starts_with(root))
    }

    /// Records whether a source answered; true if that changed anything.
    fn set(&self, id: i64, root: &Path, reachable: bool) -> bool {
        let mut offline = self.offline.write();
        let known = offline.iter().position(|(i, _)| *i == id);
        match (known, reachable) {
            (Some(i), true) => {
                offline.remove(i);
                true
            }
            (None, false) => {
                offline.push((id, root.to_path_buf()));
                true
            }
            _ => false,
        }
    }

    fn forget_removed(&self, ids: &[i64]) {
        self.offline.write().retain(|(id, _)| ids.contains(id));
    }
}

/// True if `dir` answers within [`PATIENCE`].
pub fn reachable(dir: &Path) -> bool {
    let (tx, rx) = mpsc::channel();
    let dir = dir.to_path_buf();
    // A hung check is left to finish on its own.
    let spawned = std::thread::Builder::new()
        .name("fuji-reach-probe".into())
        .spawn(move || {
            let _ = tx.send(dir.is_dir());
        });
    spawned.is_ok() && rx.recv_timeout(PATIENCE).unwrap_or(false)
}

/// Checks one source now, telling the window if it went off- or online.
/// Returns whether it can be reached.
pub fn check(app: &AppHandle, state: &AppState, id: i64, root: &Path) -> bool {
    let ok = reachable(root);
    if state.reach.set(id, root, ok) {
        if ok {
            log::info!("{} is back", root.display());
        } else {
            log::warn!("{} can't be reached", root.display());
        }
        let _ = app.emit("sources-changed", ());
    }
    ok
}

/// Watches every source; when one comes back, rescans it and saves the
/// sidecars that had to wait.
pub fn spawn(app: AppHandle, state: Arc<AppState>) {
    std::thread::Builder::new()
        .name("fuji-reach".into())
        .spawn(move || {
            loop {
                std::thread::sleep(EVERY);
                let sources = state.lib.sources().unwrap_or_default();
                let ids: Vec<i64> = sources.iter().map(|s| s.id).collect();
                state.reach.forget_removed(&ids);
                let mut back = false;
                for source in &sources {
                    let was_offline = state.reach.is_offline(source.id);
                    let ok = check(&app, &state, source.id, Path::new(&source.path));
                    back |= was_offline && ok;
                }
                if back {
                    crate::sidecars::flush(&state);
                    crate::spawn_scan(app.clone(), state.clone());
                }
            }
        })
        .expect("spawn reachability monitor");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offline_sources_cover_their_photos() {
        let reach = Reach::default();
        let root = Path::new("/shield/photos");
        assert!(reach.set(1, root, false));
        assert!(!reach.set(1, root, false), "no change the second time");
        assert!(reach.is_offline(1));
        assert!(reach.covers(Path::new("/shield/photos/2026/DSCF0001.JPG")));
        assert!(!reach.covers(Path::new("/shield/photos-old/DSCF0001.JPG")));
        assert!(reach.set(1, root, true));
        assert!(!reach.is_offline(1));
        assert!(!reach.covers(Path::new("/shield/photos/2026/DSCF0001.JPG")));
    }

    #[test]
    fn folders_that_exist_are_reachable() {
        let dir = std::env::temp_dir();
        assert!(reachable(&dir));
        assert!(!reachable(&dir.join("fujicloud-no-such-folder-7f3a")));
    }
}
