//! Putting deleted photos somewhere they can be fetched back from.
//!
//! The Windows Recycle Bin only covers fixed local drives: deleting from a
//! network share or a USB stick removes the file for good. Photos there go
//! into a `.fujicloud-trash` folder at the top of their library folder
//! instead (a rename on the same drive, so it's instant), keeping their
//! folder layout so they're easy to put back. Scans skip dot-folders.

use std::io;
use std::path::{Path, PathBuf};

/// Name of the folder discarded photos are kept in, under a library folder.
pub const TRASH_DIR: &str = ".fujicloud-trash";

pub fn trash_dir(root: &Path) -> PathBuf {
    root.join(TRASH_DIR)
}

/// Moves `file` (somewhere under `root`) into the library folder's trash,
/// and returns where it went. A name already taken there gets " (2)" etc.
pub fn to_trash_folder(root: &Path, file: &Path) -> io::Result<PathBuf> {
    let relative = file.strip_prefix(root).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{} isn't in {}", file.display(), root.display()),
        )
    })?;
    let dest = free_name(&trash_dir(root).join(relative));
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(file, &dest)?;
    // Who deleted it and when, for the trash and for other PCs' reviews.
    // (Best effort: the photo is in the trash either way.)
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let line = format!(
        "{at}\t{}\t{}\t{}\n",
        crate::device_name().replace(['\t', '\n'], " "),
        relative.to_string_lossy(),
        dest.strip_prefix(trash_dir(root))
            .unwrap_or(&dest)
            .to_string_lossy()
    );
    use std::io::Write;
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(trash_dir(root).join(LOG))
        .and_then(|mut f| f.write_all(line.as_bytes()));
    Ok(dest)
}

/// The trash's record of what was deleted: a line each of time, PC, where
/// the file was, and where it is in the trash.
const LOG: &str = "deleted.tsv";

/// One line of the trash's record.
#[derive(Debug, Clone)]
pub struct Deletion {
    pub at: i64,
    pub by: String,
    /// Where the file was, relative to the library folder.
    pub from: PathBuf,
    /// Where it is, relative to the trash.
    pub to: PathBuf,
}

/// The trash's record, oldest first.
pub fn deletion_log(root: &Path) -> Vec<Deletion> {
    std::fs::read_to_string(trash_dir(root).join(LOG))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            Some(Deletion {
                at: f.next()?.parse().ok()?,
                by: f.next()?.to_owned(),
                from: PathBuf::from(f.next()?),
                to: PathBuf::from(f.next()?),
            })
        })
        .collect()
}

/// A photo in a library folder's trash.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Trashed {
    /// Where it is now.
    pub path: PathBuf,
    /// Where it was (and goes back to).
    pub from: PathBuf,
    pub name: String,
    pub kind: crate::Kind,
    pub size: u64,
    /// When it was deleted (Unix seconds) and by which PC, if this app did.
    pub at: Option<i64>,
    pub by: Option<String>,
}

/// Everything in the library folder's trash, latest deleted first. (A
/// photo's sidecar travels with it and isn't listed.)
pub fn list_trash(root: &Path) -> Vec<Trashed> {
    let trash = trash_dir(root);
    let log: std::collections::HashMap<PathBuf, Deletion> = deletion_log(root)
        .into_iter()
        .map(|d| (d.to.clone(), d))
        .collect();
    let mut out: Vec<Trashed> = crate::scan::walk(&trash)
        .into_iter()
        .filter_map(|f| {
            let rel = f.path.strip_prefix(&trash).ok()?.to_path_buf();
            let entry = log.get(&rel);
            Some(Trashed {
                from: root.join(entry.map_or(&rel, |d| &d.from)),
                name: f.path.file_name()?.to_string_lossy().into_owned(),
                kind: f.kind,
                size: f.size,
                at: entry.map(|d| d.at),
                by: entry.map(|d| d.by.clone()),
                path: f.path,
            })
        })
        .collect();
    out.sort_by(|a, b| b.at.cmp(&a.at).then_with(|| a.path.cmp(&b.path)));
    out
}

/// Puts a trashed photo (somewhere in `root`'s trash) back where it was,
/// with its sidecar, and returns where it now is. A name taken meanwhile
/// gets " (2)" etc.
pub fn restore(root: &Path, trashed: &Path) -> io::Result<PathBuf> {
    let trash = trash_dir(root);
    let rel = trashed.strip_prefix(&trash).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} isn't in the trash of {}",
                trashed.display(),
                root.display()
            ),
        )
    })?;
    let from = deletion_log(root)
        .into_iter()
        .rev()
        .find(|d| d.to == rel)
        .map_or_else(|| rel.to_path_buf(), |d| d.from);
    let dest = free_name(&root.join(from));
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(trashed, &dest)?;
    if let Some(kind) = crate::Kind::from_path(trashed) {
        let sidecar = crate::xmp::sidecar_path(trashed, kind);
        let back = crate::xmp::sidecar_path(&dest, kind);
        if sidecar.is_file() && !back.exists() {
            let _ = std::fs::rename(&sidecar, &back);
        }
    }
    Ok(dest)
}

/// Deletes these trashed files for good (with their sidecars), or with
/// `None` the whole trash. Returns how many photos went.
pub fn empty(root: &Path, only: Option<&[PathBuf]>) -> io::Result<usize> {
    let trash = trash_dir(root);
    let Some(only) = only else {
        let n = list_trash(root).len();
        return match std::fs::remove_dir_all(&trash) {
            Ok(()) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(0),
            Err(e) => Err(e),
        };
    };
    let mut n = 0;
    for file in only.iter().filter(|f| f.starts_with(&trash)) {
        if let Some(kind) = crate::Kind::from_path(file) {
            let _ = std::fs::remove_file(crate::xmp::sidecar_path(file, kind));
        }
        match std::fs::remove_file(file) {
            Ok(()) => n += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Which PC deleted each file (by where it was), and when: the latest.
pub fn deletions(root: &Path) -> std::collections::HashMap<PathBuf, (String, i64)> {
    deletion_log(root)
        .into_iter()
        .map(|d| (root.join(d.from), (d.by, d.at)))
        .collect()
}

/// Where `file` (somewhere under `root`) went in the library folder's
/// trash: the same name, or a " (2)" one, with `size` bytes if given (the
/// latest such if there are several).
pub fn find_in_trash(root: &Path, file: &Path, size: Option<u64>) -> Option<PathBuf> {
    let at = trash_dir(root).join(file.strip_prefix(root).ok()?);
    let stem = at.file_stem()?.to_string_lossy().into_owned();
    let ext = at
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    // Names are given out in order, so the first one free ends the search
    // (each look is a round trip to the share).
    std::iter::once(at.clone())
        .chain((2..100).map(|n| at.with_file_name(format!("{stem} ({n}){ext}"))))
        .map_while(|p| Some((std::fs::metadata(&p).ok()?, p)))
        .filter(|(md, _)| md.is_file() && size.is_none_or(|s| md.len() == s))
        .max_by_key(|(md, _)| md.modified().ok())
        .map(|(_, p)| p)
}

/// `path`, or the first of `name (2).ext`, `name (3).ext`, … that is free.
pub(crate) fn free_name(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| path.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("some name is free")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discarded_files_keep_their_folders_and_never_overwrite() {
        let root = std::env::temp_dir().join(format!("fuji-discard-{}", std::process::id()));
        let day = root.join("2026").join("2026-10-21");
        std::fs::create_dir_all(&day).unwrap();
        let shot = day.join("DSCF0001.JPG");

        std::fs::write(&shot, b"first").unwrap();
        let first = to_trash_folder(&root, &shot).unwrap();
        assert_eq!(
            first,
            root.join(TRASH_DIR)
                .join("2026")
                .join("2026-10-21")
                .join("DSCF0001.JPG")
        );
        assert!(!shot.exists());

        // The camera reuses file names after 9999 shots.
        std::fs::write(&shot, b"second").unwrap();
        let second = to_trash_folder(&root, &shot).unwrap();
        assert_eq!(second.file_name().unwrap(), "DSCF0001 (2).JPG");
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(std::fs::read(&second).unwrap(), b"second");

        assert!(to_trash_folder(&day, &root.join("elsewhere.jpg")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The trash lists what's in it with who deleted it, puts photos back
    /// where they were (sidecar too), and empties.
    #[test]
    fn trash_lists_restores_and_empties() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let day = root.join("2026").join("2026-10-21");
        std::fs::create_dir_all(&day).unwrap();
        for name in ["DSCF0001.JPG", "DSCF0001.xmp", "DSCF0002.JPG"] {
            std::fs::write(day.join(name), name).unwrap();
        }
        for name in ["DSCF0001.JPG", "DSCF0001.xmp", "DSCF0002.JPG"] {
            to_trash_folder(root, &day.join(name)).unwrap();
        }
        let listed = list_trash(root);
        assert_eq!(listed.len(), 2, "the sidecar isn't listed: {listed:?}");
        let one = listed.iter().find(|t| t.name == "DSCF0001.JPG").unwrap();
        assert_eq!(one.from, day.join("DSCF0001.JPG"));
        assert_eq!(one.by.as_deref(), Some(crate::device_name().as_str()));
        assert!(one.at.is_some());
        assert_eq!(
            deletions(root).get(&day.join("DSCF0002.JPG")).map(|d| &d.0),
            Some(&crate::device_name())
        );

        // A file of that name has turned up meanwhile: nothing is overwritten.
        std::fs::write(day.join("DSCF0001.JPG"), "new").unwrap();
        let back = restore(root, &one.path).unwrap();
        assert_eq!(back, day.join("DSCF0001 (2).JPG"));
        assert_eq!(std::fs::read_to_string(&back).unwrap(), "DSCF0001.JPG");
        assert!(
            day.join("DSCF0001 (2).xmp").is_file(),
            "its sidecar came too"
        );
        assert!(restore(root, &root.join("elsewhere.jpg")).is_err());

        assert_eq!(list_trash(root).len(), 1);
        assert_eq!(empty(root, None).unwrap(), 1);
        assert!(list_trash(root).is_empty());
        assert_eq!(empty(root, None).unwrap(), 0);
    }
}
