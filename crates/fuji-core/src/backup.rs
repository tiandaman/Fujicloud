//! A second copy of the library on another drive (a USB disk, or a OneDrive
//! folder that uploads it). Only new and changed files are copied, each one
//! checked against the original, and nothing is ever deleted from the
//! backup: if a photo disappears from the Shield, the backup still has it.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Kind, ingest, scan};

/// One file to bring up to date.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub src: PathBuf,
    pub dest: PathBuf,
    pub size: u64,
}

/// What a backup pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Report {
    pub copied: usize,
    pub bytes: u64,
    /// Files that couldn't be copied, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

/// The folder inside the backup that mirrors `source`: its name, or for a
/// share's root (`\\SHIELD\Samsung_T7`) the whole path made into a name.
pub fn folder_for(source: &Path) -> String {
    match source.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => source
            .to_string_lossy()
            .chars()
            .map(|c| {
                if matches!(c, '\\' | '/' | ':') {
                    '_'
                } else {
                    c
                }
            })
            .collect::<String>()
            .trim_matches('_')
            .to_owned(),
    }
}

fn wanted(path: &Path) -> bool {
    Kind::from_path(path).is_some()
        || path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
}

fn modified(md: &fs::Metadata) -> Option<std::time::SystemTime> {
    md.modified().ok()
}

/// Files under `source` that are missing from, or newer than, their copy
/// under `dest`. Photos and their sidecars; hidden folders are skipped.
pub fn plan(source: &Path, dest: &Path) -> std::io::Result<Vec<Job>> {
    let mut jobs = Vec::new();
    let mut dirs = vec![PathBuf::new()];
    while let Some(rel) = dirs.pop() {
        for entry in fs::read_dir(source.join(&rel))? {
            let entry = entry?;
            let name = entry.file_name();
            let rel = rel.join(&name);
            let kind = entry.file_type()?;
            if kind.is_dir() {
                if !scan::skip_dir(&name.to_string_lossy()) {
                    dirs.push(rel);
                }
                continue;
            }
            if !kind.is_file() || !wanted(&rel) {
                continue;
            }
            let md = entry.metadata()?;
            let target = dest.join(&rel);
            let current = match fs::metadata(&target) {
                Ok(have) => {
                    have.len() == md.len()
                        && match (modified(&have), modified(&md)) {
                            (Some(have), Some(src)) => have >= src,
                            _ => true,
                        }
                }
                Err(_) => false,
            };
            if !current {
                jobs.push(Job {
                    src: source.join(&rel),
                    dest: target,
                    size: md.len(),
                });
            }
        }
    }
    jobs.sort_by(|a, b| a.src.cmp(&b.src));
    Ok(jobs)
}

/// Copies every job, checking each copy. `progress` gets (done, total).
/// `keep_going` is asked between files so a pass can be stopped.
pub fn run(jobs: &[Job], progress: &dyn Fn(usize, usize), keep_going: &dyn Fn() -> bool) -> Report {
    let mut report = Report::default();
    for (i, job) in jobs.iter().enumerate() {
        if !keep_going() {
            break;
        }
        match crate::guard(|| ingest::copy_verified(&job.src, &job.dest)) {
            Ok(()) => {
                report.copied += 1;
                report.bytes += job.size;
            }
            Err(e) => report.failed.push((job.src.clone(), e.to_string())),
        }
        progress(i + 1, jobs.len());
    }
    report
}

/// Brings `dest/<folder_for(source)>` up to date with `source`.
pub fn mirror(
    source: &Path,
    dest: &Path,
    progress: &dyn Fn(usize, usize),
    keep_going: &dyn Fn() -> bool,
) -> Result<Report, Error> {
    let target = dest.join(folder_for(source));
    let jobs = plan(source, &target)?;
    Ok(run(&jobs, progress, keep_going))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn copies_new_and_changed_photos_and_never_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("Photos");
        let drive = dir.path().join("Backup");
        write(&lib.join("2026/2026-10-21/DSCF0001.JPG"), b"jpeg");
        write(&lib.join("2026/2026-10-21/DSCF0001.RAF"), b"raw!");
        write(&lib.join("2026/2026-10-21/DSCF0001.xmp"), b"<x/>");
        write(&lib.join("2026/2026-10-21/notes.txt"), b"not a photo");
        write(&lib.join(".fujicloud-trash/DSCF0000.JPG"), b"deleted");
        let none = |_: usize, _: usize| {};
        let go = || true;

        let first = mirror(&lib, &drive, &none, &go).unwrap();
        assert_eq!((first.copied, first.bytes), (3, 12));
        let day = drive.join("Photos/2026/2026-10-21");
        assert_eq!(fs::read(day.join("DSCF0001.RAF")).unwrap(), b"raw!");
        assert!(!day.join("notes.txt").exists());
        assert!(!drive.join("Photos/.fujicloud-trash").exists());

        // Nothing changed: nothing to do.
        assert_eq!(mirror(&lib, &drive, &none, &go).unwrap().copied, 0);

        // A rating changed the sidecar; a photo was deleted from the library.
        write(
            &lib.join("2026/2026-10-21/DSCF0001.xmp"),
            b"<x rating='5'/>",
        );
        fs::remove_file(lib.join("2026/2026-10-21/DSCF0001.RAF")).unwrap();
        let again = mirror(&lib, &drive, &none, &go).unwrap();
        assert_eq!(again.copied, 1);
        assert_eq!(
            fs::read(day.join("DSCF0001.xmp")).unwrap(),
            b"<x rating='5'/>"
        );
        assert!(day.join("DSCF0001.RAF").exists(), "the backup keeps it");
    }

    #[test]
    fn a_pass_can_be_stopped_between_files() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("Photos");
        for i in 0..5 {
            write(&lib.join(format!("IMG_000{i}.HEIC")), b"heic");
        }
        let jobs = plan(&lib, &dir.path().join("Backup/Photos")).unwrap();
        let done = std::cell::Cell::new(0);
        let report = run(&jobs, &|n, _| done.set(n), &|| done.get() < 2);
        assert_eq!(report.copied, 2);
    }

    #[test]
    fn share_roots_get_a_readable_folder_name() {
        assert_eq!(folder_for(Path::new("/mnt/shield/Photos")), "Photos");
        assert_eq!(folder_for(Path::new("/")), "");
        #[cfg(windows)]
        assert_eq!(
            folder_for(Path::new(r"\\SHIELD\Samsung_T7")),
            "SHIELD_Samsung_T7"
        );
    }
}
