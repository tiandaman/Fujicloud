//! Copies new shots from the camera inbox into the library, organised as
//! `<library>/<YYYY>/<YYYY-MM-DD>/<file>`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Kind, meta};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Not in the library yet; copy it here.
    New(PathBuf),
    /// An identical file (same name and size) is already in the library.
    Exists(PathBuf),
}

/// Decides where `src` belongs in the library without copying anything.
pub fn target(src: &Path, library: &Path) -> Result<Target, Error> {
    let kind = Kind::from_path(src)
        .ok_or_else(|| Error::Decode(format!("not a photo: {}", src.display())))?;
    let md = fs::metadata(src)?;
    let taken = meta::read(src, kind)
        .ok()
        .and_then(|m| m.taken_at)
        .unwrap_or_else(|| {
            md.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    let date = chrono::DateTime::from_timestamp(taken, 0)
        .unwrap_or_default()
        .naive_utc()
        .date();
    let dir = library
        .join(date.format("%Y").to_string())
        .join(date.format("%Y-%m-%d").to_string());

    let name = src.file_name().expect("imported paths are files");
    let stem = Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = Path::new(name)
        .extension()
        .unwrap_or_default()
        .to_string_lossy();
    let mut dest = dir.join(name);
    let mut n = 1;
    while let Ok(existing) = fs::metadata(&dest) {
        if existing.len() == md.len() {
            return Ok(Target::Exists(dest));
        }
        // Same name, different file (the camera's counter wrapped around).
        dest = dir.join(format!("{stem}_{n}.{ext}"));
        n += 1;
    }
    Ok(Target::New(dest))
}

/// Copies via a temporary name so the library never sees half a file.
/// On Windows `fs::copy` is `CopyFileExW`, which keeps the modified time and
/// uses large pipelined I/O on SMB shares.
pub fn copy(src: &Path, dest: &Path) -> Result<(), Error> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = dest.with_extension("fujicloud-part");
    fs::copy(src, &part)?;
    fs::rename(&part, dest)?;
    Ok(())
}

pub fn import(src: &Path, library: &Path) -> Result<Target, Error> {
    let target = target(src, library)?;
    if let Target::New(dest) = &target {
        copy(src, dest)?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_by_date_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let inbox = dir.path().join("inbox");
        let lib = dir.path().join("lib");
        fs::create_dir_all(&inbox).unwrap();
        let src = inbox.join("DSCF0042.JPG");
        fs::write(&src, crate::test_jpeg(32, 32, Some("2026:10:21 09:15:00"))).unwrap();

        let dest = lib.join("2026/2026-10-21/DSCF0042.JPG");
        assert_eq!(import(&src, &lib).unwrap(), Target::New(dest.clone()));
        assert!(dest.is_file());
        assert_eq!(import(&src, &lib).unwrap(), Target::Exists(dest));

        fs::write(&src, crate::test_jpeg(48, 32, Some("2026:10:21 09:16:00"))).unwrap();
        let Target::New(dest) = import(&src, &lib).unwrap() else {
            panic!("expected a copy")
        };
        assert_eq!(dest.file_name().unwrap(), "DSCF0042_1.JPG");
        assert!(!dest.with_extension("fujicloud-part").exists());
    }
}
