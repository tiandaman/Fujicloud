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
    let part = part_path(dest);
    fs::copy(src, &part)?;
    fs::rename(&part, dest)?;
    Ok(())
}

/// Where a copy is written before it's complete. Keeps the whole name, so a
/// RAW and a JPEG of the same shot copied at once don't share one.
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".fujicloud-part");
    dest.with_file_name(name)
}

/// A checksum of a file's contents (xxh3, read in 1 MB chunks).
pub fn checksum(path: &Path) -> std::io::Result<u128> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = xxhash_rust::xxh3::Xxh3::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.digest128());
        }
        hasher.update(&buf[..n]);
    }
}

/// Whether two files hold exactly the same bytes.
pub fn same_content(a: &Path, b: &Path) -> std::io::Result<bool> {
    if fs::metadata(a)?.len() != fs::metadata(b)?.len() {
        return Ok(false);
    }
    Ok(checksum(a)? == checksum(b)?)
}

/// Copies like [`copy`], then reads the copy back and compares it with the
/// original before giving it its real name. A bad copy (a flaky network
/// write) is thrown away and tried once more.
pub fn copy_verified(src: &Path, dest: &Path) -> Result<(), Error> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let want = checksum(src)?;
    for attempt in 1..=2 {
        fs::copy(src, &part)?;
        if checksum(&part)? == want {
            fs::rename(&part, dest)?;
            return Ok(());
        }
        let _ = fs::remove_file(&part);
        log::warn!("copy of {} didn't match (attempt {attempt})", src.display());
    }
    Err(Error::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("the copy of {} didn't match the original", src.display()),
    )))
}

pub fn import(src: &Path, library: &Path) -> Result<Target, Error> {
    let target = target(src, library)?;
    if let Target::New(dest) = &target {
        copy_verified(src, dest)?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verified_copies_match_and_differences_are_caught() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.JPG");
        fs::write(&src, b"some photo bytes").unwrap();
        let dest = dir.path().join("lib/2026/a.JPG");
        copy_verified(&src, &dest).unwrap();
        assert!(same_content(&src, &dest).unwrap());
        assert!(!part_path(&dest).exists());
        assert_ne!(
            part_path(&dest.with_file_name("DSCF0001.JPG")),
            part_path(&dest.with_file_name("DSCF0001.RAF")),
            "a pair copied at once"
        );

        let other = dir.path().join("b.JPG");
        fs::write(&other, b"some photo bytez").unwrap();
        assert!(
            !same_content(&src, &other).unwrap(),
            "same size, different bytes"
        );
    }

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
        assert!(!part_path(&dest).exists());
    }
}
