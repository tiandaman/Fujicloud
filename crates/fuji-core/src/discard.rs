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
    Ok(dest)
}

/// `path`, or the first of `name (2).ext`, `name (3).ext`, … that is free.
fn free_name(path: &Path) -> PathBuf {
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
}
