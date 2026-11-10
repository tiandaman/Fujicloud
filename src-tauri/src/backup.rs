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
/// The backup is read back and compared with the library this often.
const CHECK_EVERY: Duration = Duration::from_secs(30 * 24 * 3600);
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
    /// Checking the backup against the library (reading both copies of
    /// every photo): under way, and how far along.
    pub checking: bool,
    pub check_done: usize,
    pub check_total: usize,
    /// Unix seconds when the last complete check finished.
    pub last_checked: Option<i64>,
    /// What the check (the one under way, else the last) found: photos
    /// whose copies differ, ones that couldn't be read, how many were the
    /// same and how many aren't backed up yet.
    pub damaged: Vec<String>,
    pub unreadable: Vec<String>,
    pub check_same: usize,
    pub check_waiting: usize,
    /// How far a check that was interrupted had got (the last photo read),
    /// so the next one carries on from there.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check_cursor: Option<String>,
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
                last_checked: saved.last_checked,
                damaged: saved.damaged,
                unreadable: saved.unreadable,
                check_same: saved.check_same,
                check_waiting: saved.check_waiting,
                check_cursor: saved.check_cursor,
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
                // Once a month, read everything back: a copy can rot on a
                // failing drive without its size or date changing.
                let (checked, backed_up) = {
                    let s = state.backup.status.lock();
                    (s.last_checked, s.last_backed_up)
                };
                let month_ago = chrono::Utc::now().timestamp() - CHECK_EVERY.as_secs() as i64;
                if backed_up.is_some() && checked.is_none_or(|t| t < month_ago) {
                    check(app.clone(), state.clone());
                }
            }
        })
        .expect("spawn backup thread");
}

/// Starts checking the backup against the library, unless one is running.
/// It carries on from where an interrupted check stopped.
pub fn check(app: AppHandle, state: Arc<AppState>) {
    static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    use std::sync::atomic::Ordering;
    if RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("fuji-backup-check".into())
        .spawn(move || {
            check_pass(&app, &state);
            RUNNING.store(false, Ordering::Release);
        });
    if spawned.is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

fn check_pass(app: &AppHandle, state: &AppState) {
    let Some(dest) = target(state) else {
        return;
    };
    let backup = &state.backup;
    if !crate::reach::reachable(&dest) {
        backup.update(app, state, |s| {
            s.error = Some(format!(
                "The backup folder {} can't be reached, so it couldn't be checked. Is the drive plugged in?",
                dest.display()
            ));
        });
        return;
    }
    let mut jobs = Vec::new();
    let mut skipped = 0;
    for source in state.lib.sources().unwrap_or_default() {
        let root = Path::new(&source.path);
        if root.starts_with(&dest) || dest.starts_with(root) {
            continue;
        }
        if state.reach.is_offline(source.id) {
            skipped += 1;
            continue;
        }
        jobs.extend(backup::photos(root, &dest.join(backup::folder_for(root))));
    }
    // Carry on after the last photo an interrupted check read.
    let cursor = backup.status.lock().check_cursor.clone().map(PathBuf::from);
    let from = cursor
        .and_then(|c| jobs.iter().position(|j| j.src > c))
        .unwrap_or(0);
    let mut found = backup::Check::default();
    if from > 0 {
        let s = backup.status.lock();
        found.same = s.check_same;
        found.waiting = s.check_waiting;
        found.different = s.damaged.iter().map(PathBuf::from).collect();
        found.unreadable = s
            .unreadable
            .iter()
            .map(|p| (PathBuf::from(p), String::new()))
            .collect();
    }
    let total = jobs.len();
    log::info!(
        "checking the backup in {}: {total} photos, from number {from}",
        dest.display()
    );
    backup.update(app, state, |s| {
        (s.checking, s.check_done, s.check_total) = (true, from, total);
    });
    let still_wanted = || target(state).as_deref() == Some(dest.as_path());
    let names = |paths: &mut dyn Iterator<Item = &PathBuf>| -> Vec<String> {
        paths.map(|p| p.to_string_lossy().into_owned()).collect()
    };
    let mut done = from;
    // In stretches, so progress and how far it's got are kept as it goes.
    for stretch in jobs[from..].chunks(50) {
        let got = backup::verify(stretch, &mut found, &|_| {}, &still_wanted);
        done += got;
        backup.update(app, state, |s| {
            s.check_done = done;
            s.check_same = found.same;
            s.check_waiting = found.waiting;
            s.damaged = names(&mut found.different.iter());
            s.unreadable = names(&mut found.unreadable.iter().map(|(p, _)| p));
            s.check_cursor = stretch
                .get(got.saturating_sub(1))
                .filter(|_| got > 0)
                .map(|j| j.src.to_string_lossy().into_owned())
                .or(s.check_cursor.take());
        });
        backup.save();
        if got < stretch.len() {
            break;
        }
    }
    for (file, why) in &found.unreadable {
        log::warn!("backup check: couldn't read {}: {why}", file.display());
    }
    for file in &found.different {
        log::warn!(
            "backup check: {} differs from its backup copy",
            file.display()
        );
    }
    let finished = done == total && still_wanted() && skipped == 0;
    log::info!(
        "backup check {}: {} the same, {} different, {} unreadable, {} not backed up yet",
        if finished { "finished" } else { "stopped" },
        found.same,
        found.different.len(),
        found.unreadable.len(),
        found.waiting
    );
    backup.update(app, state, |s| {
        s.checking = false;
        if done == total {
            // Done with this round (even if a folder was out of reach).
            s.check_cursor = None;
        }
        if finished {
            s.last_checked = Some(chrono::Utc::now().timestamp());
        }
    });
    backup.save();
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
