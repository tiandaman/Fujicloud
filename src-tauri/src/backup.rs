//! Keeps the backup drive up to date in the background: soon after new
//! photos are imported, every few hours, and when asked. Remembers when it
//! last finished, for "Backed up 2 hours ago".

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use fuji_core::backup;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::AppState;

/// A pass runs at least this often while the app is open.
const EVERY: Duration = Duration::from_secs(4 * 3600);
/// Imports arrive in bursts; wait for the burst to end.
const SETTLE: Duration = Duration::from_secs(60);
/// The first pass after the app starts, once the scan has had a go.
const FIRST: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Status {
    /// The backup folder, or None when backup is off.
    pub target: Option<String>,
    /// Unix seconds when the last complete pass finished.
    pub last_backed_up: Option<i64>,
    pub running: bool,
    pub done: usize,
    pub total: usize,
    /// Why the last pass didn't finish cleanly.
    pub error: Option<String>,
}

pub struct Backup {
    status: Mutex<Status>,
    file: PathBuf,
}

impl Backup {
    pub fn new(file: PathBuf) -> Self {
        let saved: Status = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Backup {
            status: Mutex::new(Status {
                last_backed_up: saved.last_backed_up,
                error: saved.error,
                ..Status::default()
            }),
            file,
        }
    }

    pub fn status(&self, state: &AppState) -> Status {
        let mut status = self.status.lock().clone();
        status.target = target(state).map(|t| t.to_string_lossy().into_owned());
        status
    }

    fn update(&self, app: &AppHandle, state: &AppState, change: impl FnOnce(&mut Status)) {
        change(&mut self.status.lock());
        let status = self.status(state);
        let _ = app.emit("backup", &status);
    }

    fn save(&self) {
        let status = self.status.lock().clone();
        if let Ok(json) = serde_json::to_vec(&status) {
            let _ = std::fs::write(&self.file, json);
        }
    }
}

fn target(state: &AppState) -> Option<PathBuf> {
    state.settings.read().backup_to.clone()
}

static QUEUE: OnceLock<mpsc::Sender<bool>> = OnceLock::new();

/// Asks for a pass once things settle (after an import).
pub fn soon() {
    if let Some(tx) = QUEUE.get() {
        let _ = tx.send(false);
    }
}

/// Asks for a pass right away ("Back up now", or a new backup folder).
pub fn now() {
    if let Some(tx) = QUEUE.get() {
        let _ = tx.send(true);
    }
}

pub fn start(app: AppHandle, state: Arc<AppState>) {
    let (tx, rx) = mpsc::channel::<bool>();
    if QUEUE.set(tx).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("fuji-backup".into())
        .spawn(move || {
            let mut due = Instant::now() + FIRST;
            loop {
                let wait = due.saturating_duration_since(Instant::now());
                match rx.recv_timeout(wait) {
                    Ok(true) => due = Instant::now(),
                    Ok(false) => due = due.min(Instant::now() + SETTLE),
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
                if Instant::now() < due {
                    continue;
                }
                pass(&app, &state);
                due = Instant::now() + EVERY;
            }
        })
        .expect("spawn backup thread");
}

fn pass(app: &AppHandle, state: &AppState) {
    let Some(dest) = target(state) else {
        return;
    };
    let backup = &state.backup;
    if !crate::reach::reachable(&dest) {
        backup.update(app, state, |s| {
            s.error = Some(format!(
                "The backup folder {} can't be reached. Is the drive plugged in?",
                dest.display()
            ));
        });
        backup.save();
        return;
    }

    backup.update(app, state, |s| {
        (s.running, s.done, s.total, s.error) = (true, 0, 0, None);
    });
    let mut jobs = Vec::new();
    let mut problems = Vec::new();
    for source in state.lib.sources().unwrap_or_default() {
        let root = Path::new(&source.path);
        // The backup folder may itself be indexed; don't back it up into itself.
        if root.starts_with(&dest) || dest.starts_with(root) {
            problems.push(format!(
                "{} is inside the backup folder (or holds it), so it was skipped",
                source.label
            ));
            continue;
        }
        if state.reach.is_offline(source.id) {
            problems.push(format!(
                "{} can't be reached, so it wasn't backed up",
                source.label
            ));
            continue;
        }
        let into = dest.join(backup::folder_for(root));
        match backup::plan(root, &into) {
            Ok(found) => jobs.extend(found),
            Err(e) => problems.push(format!("{}: {e}", source.label)),
        }
    }

    let still_wanted = || target(state).as_deref() == Some(dest.as_path());
    let last_emit = Mutex::new(Instant::now());
    let report = backup::run(
        &jobs,
        &|done, total| {
            let mut last = last_emit.lock();
            if done == total || last.elapsed() > Duration::from_millis(500) {
                *last = Instant::now();
                backup.update(app, state, |s| (s.done, s.total) = (done, total));
            }
        },
        &still_wanted,
    );
    for (file, why) in &report.failed {
        log::warn!("backup of {}: {why}", file.display());
    }
    if !report.failed.is_empty() {
        problems.push(format!(
            "{} couldn't be copied (see the log)",
            if report.failed.len() == 1 {
                "1 file".to_owned()
            } else {
                format!("{} files", report.failed.len())
            }
        ));
    }
    log::info!(
        "backup to {}: {} files ({} MB) copied, {} problems",
        dest.display(),
        report.copied,
        report.bytes >> 20,
        problems.len()
    );
    let finished = still_wanted();
    backup.update(app, state, |s| {
        s.running = false;
        if finished {
            s.last_backed_up = Some(chrono::Utc::now().timestamp());
        }
        s.error = (!problems.is_empty()).then(|| problems.join(". "));
    });
    backup.save();
}
