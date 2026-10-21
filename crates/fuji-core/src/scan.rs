//! Incremental, parallel library scanning.
//!
//! Over SMB every directory listing and file open is a network round trip,
//! so both the walk and the metadata reads fan out across a wide thread pool
//! that is sized for latency rather than CPU.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use serde::Serialize;

use crate::db::{Library, NewPhoto, SourceRow};
use crate::{Error, Kind, meta};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub source_id: i64,
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Listing,
    Reading,
    Done,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone)]
pub struct FoundFile {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
}

fn io_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(24)
            .thread_name(|i| format!("fuji-io-{i}"))
            .build()
            .expect("failed to build IO pool")
    })
}

/// Rescans one source, only reading metadata for new or changed files.
pub fn scan_source(
    lib: &Library,
    source: &SourceRow,
    progress: &(dyn Fn(Progress) + Sync),
) -> Result<Summary, Error> {
    let root = PathBuf::from(&source.path);
    if !root.is_dir() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} is not reachable", source.path),
        )));
    }
    let report = |phase, done, total| {
        progress(Progress {
            source_id: source.id,
            phase,
            done,
            total,
        })
    };
    report(Phase::Listing, 0, 0);

    let found = io_pool().install(|| walk(&root));
    let mut known = lib.index_of(source.id)?;
    let mut summary = Summary::default();
    let mut todo = Vec::new();
    for file in found {
        let key = file.path.to_string_lossy().into_owned();
        match known.remove(&key) {
            Some((_, size, mtime)) if size == file.size && mtime == file.mtime => {}
            Some(_) => {
                summary.updated += 1;
                todo.push(file);
            }
            None => {
                summary.added += 1;
                todo.push(file);
            }
        }
    }

    let gone: Vec<i64> = known.into_values().map(|(id, _, _)| id).collect();
    summary.removed = gone.len();
    lib.remove_photos(&gone)?;

    // Newest first, so a fresh import shows up in the grid right away.
    todo.sort_by_key(|f| std::cmp::Reverse(f.mtime));
    let total = todo.len();
    let done = AtomicUsize::new(0);
    report(Phase::Reading, 0, total);
    for chunk in todo.chunks(512) {
        let photos: Vec<NewPhoto> =
            io_pool().install(|| chunk.par_iter().filter_map(read_one).collect());
        summary.failed += chunk.len() - photos.len();
        lib.upsert(source.id, &photos)?;
        let n = done.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
        report(Phase::Reading, n, total);
    }
    report(Phase::Done, total, total);
    Ok(summary)
}

/// Indexes specific files (e.g. fresh imports) without walking the source.
pub fn index_files(lib: &Library, source_id: i64, paths: &[PathBuf]) -> Result<usize, Error> {
    let files: Vec<FoundFile> = paths.iter().filter_map(|p| found_file(p)).collect();
    let photos: Vec<NewPhoto> =
        io_pool().install(|| files.par_iter().filter_map(read_one).collect());
    lib.upsert(source_id, &photos)?;
    Ok(photos.len())
}

fn read_one(file: &FoundFile) -> Option<NewPhoto> {
    let meta = match meta::read(&file.path, file.kind) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("metadata for {}: {e}", file.path.display());
            // Keep the file browsable even if its metadata is unreadable.
            meta::PhotoMeta {
                orientation: 1,
                ..Default::default()
            }
        }
    };
    let mut meta = meta;
    if meta.taken_at.is_none() {
        meta.taken_at = Some(file.mtime);
    }
    Some(NewPhoto {
        path: file.path.clone(),
        kind: file.kind,
        size: file.size,
        mtime: file.mtime,
        meta,
    })
}

pub fn found_file(path: &Path) -> Option<FoundFile> {
    let kind = Kind::from_path(path)?;
    let md = std::fs::metadata(path).ok()?;
    md.is_file().then(|| FoundFile {
        path: path.to_path_buf(),
        kind,
        size: md.len(),
        mtime: mtime(&md),
    })
}

fn mtime(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Recursively lists supported files, one rayon task per directory.
pub fn walk(dir: &Path) -> Vec<FoundFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::warn!("cannot list {}", dir.display());
        return Vec::new();
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !skip_dir(&entry.file_name().to_string_lossy()) {
                dirs.push(path);
            }
        } else if let Some(k) = Kind::from_path(&path) {
            // DirEntry::metadata is free on Windows (it comes with the listing).
            if let Ok(md) = entry.metadata() {
                files.push(FoundFile {
                    path,
                    kind: k,
                    size: md.len(),
                    mtime: mtime(&md),
                });
            }
        }
    }
    let nested: Vec<Vec<FoundFile>> = dirs.par_iter().map(|d| walk(d)).collect();
    files.extend(nested.into_iter().flatten());
    files
}

fn skip_dir(name: &str) -> bool {
    name.starts_with('.')
        || name.starts_with('$')
        || matches!(
            name.to_ascii_lowercase().as_str(),
            "system volume information"
                | "lost.dir"
                | "android"
                | "@eadir"
                | "#recycle"
                | "node_modules"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incremental_scan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("2026/10-21")).unwrap();
        std::fs::create_dir_all(root.join(".thumbnails")).unwrap();
        let jpeg = crate::test_jpeg(64, 48, None);
        std::fs::write(root.join("2026/10-21/DSCF0001.JPG"), &jpeg).unwrap();
        std::fs::write(
            root.join("2026/10-21/DSCF0001.RAF"),
            crate::raf::wrap_jpeg_for_tests(&jpeg),
        )
        .unwrap();
        std::fs::write(root.join(".thumbnails/x.jpg"), &jpeg).unwrap();
        std::fs::write(root.join("notes.txt"), b"hi").unwrap();

        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(root).unwrap();
        let s = scan_source(&lib, &src, &|_| {}).unwrap();
        assert_eq!((s.added, s.updated, s.removed), (2, 0, 0));
        let rows = lib.photos().unwrap();
        assert!(rows.iter().all(|r| r.width == 64 && r.height == 48));

        let s = scan_source(&lib, &src, &|_| {}).unwrap();
        assert_eq!((s.added, s.updated, s.removed), (0, 0, 0));

        std::fs::remove_file(root.join("2026/10-21/DSCF0001.RAF")).unwrap();
        let s = scan_source(&lib, &src, &|_| {}).unwrap();
        assert_eq!(s.removed, 1);
        assert_eq!(lib.photos().unwrap().len(), 1);
    }
}
