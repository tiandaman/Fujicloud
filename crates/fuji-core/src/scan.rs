//! Incremental, parallel library scanning.
//!
//! Over SMB every directory listing and file open is a network round trip,
//! so both the walk and the metadata reads fan out across a wide thread pool
//! that is sized for latency rather than CPU.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::UNIX_EPOCH;

use rayon::prelude::*;
use serde::Serialize;

use crate::db::{Library, NewPhoto, SourceRow};
use crate::xmp::{self, Tags};
use crate::{Error, Kind, geo, meta};

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
    /// Photos whose ratings, picks, favourites or albums came from a sidecar.
    pub synced: usize,
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

/// Rescans one source, only reading metadata for new or changed files. With
/// `sidecars`, also picks up what other laptops saved in XMP sidecars.
pub fn scan_source(
    lib: &Library,
    source: &SourceRow,
    sidecars: bool,
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

    let Listing {
        files: found,
        sidecars: seen,
    } = io_pool().install(|| list(&root));
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

    // Only drop a photo when its file is really gone. A network share that
    // drops out mid-scan (the Shield going to sleep, a Wi-Fi blip) can't
    // list some folders, and their photos must not be forgotten along with
    // their ratings and albums.
    let gone: Vec<i64> = known
        .into_iter()
        .filter(|(path, _)| really_gone(Path::new(path)))
        .map(|(_, (id, _, _))| id)
        .collect();
    summary.removed = gone.len();
    lib.remove_photos(&gone)?;

    // Newest first, so a fresh import shows up in the grid right away.
    todo.sort_by_key(|f| std::cmp::Reverse(f.mtime));
    let total = todo.len();
    let done = AtomicUsize::new(0);
    report(Phase::Reading, 0, total);
    for chunk in todo.chunks(512) {
        let photos: Vec<NewPhoto> = io_pool().install(|| {
            chunk
                .par_iter()
                .filter_map(|f| read_one(f, Some(&root)))
                .collect()
        });
        summary.failed += chunk.len() - photos.len();
        lib.upsert(source.id, &photos)?;
        let n = done.fetch_add(chunk.len(), Ordering::Relaxed) + chunk.len();
        report(Phase::Reading, n, total);
    }
    // Photos indexed before folder dates were read.
    let redated = lib.redate_from_names(source.id, &root)?;
    if redated > 0 {
        log::info!("{redated} photos dated by their name or folder");
    }
    if total > 0 || summary.removed > 0 || redated > 0 {
        lib.link_edits()?;
        lib.infer_locations()?;
    }
    if sidecars {
        summary.synced = sync_sidecars(lib, source.id, &seen)?;
    }
    report(Phase::Done, total, total);
    Ok(summary)
}

/// Brings in ratings, picks, favourites and albums that another laptop (or
/// Lightroom) saved in sidecars since we last looked. `seen` maps each
/// sidecar on disk (lowercased path) to its modified time.
///
/// The first time a photo is synced, nothing is lost on either side: the
/// database and the sidecar are merged and the result written back. After
/// that, whichever side changed last wins.
pub fn sync_sidecars(
    lib: &Library,
    source_id: i64,
    seen: &HashMap<String, i64>,
) -> Result<usize, Error> {
    let rows = lib.sidecars(source_id)?;
    let on_disk = |r: &crate::db::SidecarRow| seen.get(&sidecar_key(&r.sidecar)).copied();
    let due: Vec<_> = rows
        .iter()
        .filter(|r| !r.dirty)
        .filter(|r| match (r.xmp_mtime, on_disk(r)) {
            (None, _) => true,
            (Some(known), Some(now)) => known != now,
            (Some(_), None) => false,
        })
        .collect();

    // Read each sidecar once; RAW+JPEG pairs share one.
    let mut paths: Vec<&Path> = due
        .iter()
        .filter(|r| on_disk(r).is_some())
        .map(|r| r.sidecar.as_path())
        .collect();
    paths.sort();
    paths.dedup();
    let read: HashMap<&Path, Tags> = io_pool().install(|| {
        paths
            .par_iter()
            .filter_map(|p| match xmp::read(p) {
                Ok(tags) => Some((*p, tags)),
                Err(e) => {
                    log::warn!("sidecar {}: {e}", p.display());
                    None
                }
            })
            .collect()
    });

    let mut apply = Vec::new();
    let mut write_back = Vec::new();
    for r in due {
        let mtime = on_disk(r);
        let theirs = mtime.and_then(|_| read.get(r.sidecar.as_path()));
        if mtime.is_some() && theirs.is_none() {
            continue; // unreadable: try again next scan
        }
        if r.xmp_mtime.is_some() {
            apply.push((
                r.id,
                theirs.cloned().unwrap_or_default(),
                mtime.unwrap_or(0),
            ));
            continue;
        }
        let mine = lib.tags(r.id)?;
        let merged = merge(&mine, theirs);
        if theirs != Some(&merged) && !merged.is_empty() {
            write_back.push(r.id);
        }
        apply.push((r.id, merged, mtime.unwrap_or(0)));
    }
    let applied = lib.apply_sidecars(&apply)?;
    // What another PC's models found, and the names given there.
    for (id, tags, _) in &apply {
        let imported = (|| -> Result<(), Error> {
            for (subject, found) in [
                (crate::people::Subject::Person, &tags.faces),
                (crate::people::Subject::Pet, &tags.pets),
            ] {
                if let Some(found) = found {
                    lib.import_found(*id, subject, found)?;
                }
            }
            if let Some((model, vector)) = &tags.smart {
                lib.import_embedding(*id, model, vector)?;
            }
            Ok(())
        })();
        if let Err(e) = imported {
            log::warn!("analysis from the sidecar of photo {id}: {e}");
        }
    }
    lib.sidecar_saved(&write_back, None)?;
    Ok(applied)
}

/// Keeps everything either side knows.
fn merge(mine: &Tags, theirs: Option<&Tags>) -> Tags {
    let Some(theirs) = theirs else {
        return mine.clone();
    };
    let mut albums = mine.albums.clone();
    for a in &theirs.albums {
        if !albums.iter().any(|b| b.eq_ignore_ascii_case(a)) {
            albums.push(a.clone());
        }
    }
    Tags {
        rating: if mine.rating != 0 {
            mine.rating
        } else {
            theirs.rating
        },
        label: mine.label.clone().or_else(|| theirs.label.clone()),
        flag: if mine.flag != 0 {
            mine.flag
        } else {
            theirs.flag
        },
        favorite: mine.favorite || theirs.favorite,
        albums,
        faces: mine.faces.clone().or_else(|| theirs.faces.clone()),
        pets: mine.pets.clone().or_else(|| theirs.pets.clone()),
        smart: mine.smart.clone().or_else(|| theirs.smart.clone()),
    }
}

fn sidecar_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

/// Writes these photos' sidecars (with their pair halves) from the database.
/// A sidecar that can't be written (the share is off, say) stays marked and
/// is retried later. Returns how many sidecars couldn't be saved.
pub fn save_sidecars(lib: &Library, ids: &[i64]) -> Result<usize, Error> {
    let mut groups: HashMap<PathBuf, (PathBuf, Kind, Vec<i64>)> = HashMap::new();
    for id in lib.with_pairs(ids)? {
        let Ok(file) = lib.file_info(id) else {
            continue;
        };
        let sidecar = xmp::sidecar_path(&file.path, file.kind);
        groups
            .entry(sidecar)
            .or_insert_with(|| (file.path, file.kind, Vec::new()))
            .2
            .push(id);
    }
    let mut failed = 0;
    for (sidecar, (file, kind, ids)) in groups {
        let saved = pair_tags(lib, &ids)
            .map_err(std::io::Error::other)
            .and_then(|tags| {
                xmp::write(&file, kind, &tags)?;
                match std::fs::metadata(&sidecar) {
                    Ok(md) => Ok(mtime_ms(&md)),
                    // Nothing to record and no sidecar to keep in step.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0),
                    Err(e) => Err(e),
                }
            });
        match saved {
            Ok(mtime) => lib.sidecar_saved(&ids, Some(mtime))?,
            Err(e) => {
                log::warn!("sidecar {}: {e}", sidecar.display());
                lib.sidecar_saved(&ids, None)?;
                failed += 1;
            }
        }
    }
    Ok(failed)
}

/// What a shared sidecar records: the pair's ratings and albums (the same
/// on both halves), and what the models found in whichever half they looked
/// at (the JPEG, not the RAW behind it).
fn pair_tags(lib: &Library, ids: &[i64]) -> Result<Tags, Error> {
    let mut tags = lib.tags(ids[0])?;
    for &id in &ids[1..] {
        if tags.faces.is_some() && tags.pets.is_some() && tags.smart.is_some() {
            break;
        }
        let other = lib.tags(id)?;
        tags.faces = tags.faces.or(other.faces);
        tags.pets = tags.pets.or(other.pets);
        tags.smart = tags.smart.or(other.smart);
    }
    Ok(tags)
}

/// Indexes specific files (e.g. fresh imports) without walking the source.
pub fn index_files(lib: &Library, source_id: i64, paths: &[PathBuf]) -> Result<usize, Error> {
    let files: Vec<FoundFile> = paths.iter().filter_map(|p| found_file(p)).collect();
    let root = lib
        .sources()?
        .into_iter()
        .find(|s| s.id == source_id)
        .map(|s| PathBuf::from(s.path));
    let photos: Vec<NewPhoto> = io_pool().install(|| {
        files
            .par_iter()
            .filter_map(|f| read_one(f, root.as_deref()))
            .collect()
    });
    lib.upsert(source_id, &photos)?;
    lib.link_edits()?;
    lib.infer_locations()?;
    Ok(photos.len())
}

/// `root` is the source the file is in: dates in folder names count up to
/// it (see [`crate::pathdate`]).
fn read_one(file: &FoundFile, root: Option<&Path>) -> Option<NewPhoto> {
    let meta = match crate::guard(|| meta::read(&file.path, file.kind)) {
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
        // No date of its own: its name's or its folders', else the file's.
        meta.taken_at =
            Some(crate::pathdate::taken_at(&file.path, root, file.mtime).unwrap_or(file.mtime));
    }
    let place = meta
        .lat
        .zip(meta.lon)
        .and_then(|(lat, lon)| geo::reverse(lat, lon));
    Some(NewPhoto {
        path: file.path.clone(),
        kind: file.kind,
        size: file.size,
        mtime: file.mtime,
        meta,
        place,
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

/// True only when the system says the file doesn't exist: not for a share
/// that can't be reached or a folder that can't be read.
fn really_gone(path: &Path) -> bool {
    match std::fs::symlink_metadata(path) {
        Ok(_) => false,
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => false,
        // The file is missing. Make sure its folder is, or that the folder
        // can be read and simply doesn't hold the file any more.
        Err(_) => path
            .parent()
            .is_none_or(|dir| match std::fs::read_dir(dir) {
                Ok(_) => true,
                Err(e) => e.kind() == std::io::ErrorKind::NotFound && dir_gone(dir),
            }),
    }
}

/// A folder is gone when it's missing and its parent can still be read.
fn dir_gone(dir: &Path) -> bool {
    dir.parent()
        .is_some_and(|parent| std::fs::read_dir(parent).is_ok())
}

fn mtime_ms(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
    list(dir).files
}

/// Supported files, and the XMP sidecars next to them.
#[derive(Default)]
pub struct Listing {
    pub files: Vec<FoundFile>,
    /// Lowercased path → modified time.
    pub sidecars: HashMap<String, i64>,
}

pub fn list(dir: &Path) -> Listing {
    let Ok(entries) = std::fs::read_dir(dir) else {
        log::warn!("cannot list {}", dir.display());
        return Listing::default();
    };
    let mut files = Vec::new();
    let mut sidecars = HashMap::new();
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
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("xmp"))
            && let Ok(md) = entry.metadata()
        {
            sidecars.insert(sidecar_key(&path), mtime_ms(&md));
        }
    }
    let nested: Vec<Listing> = dirs.par_iter().map(|d| list(d)).collect();
    for n in nested {
        files.extend(n.files);
        sidecars.extend(n.sidecars);
    }
    Listing { files, sidecars }
}

pub(crate) fn skip_dir(name: &str) -> bool {
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

    fn shared_folder() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let jpeg = crate::test_jpeg(64, 48, Some("2026:10:21 09:15:30"));
        std::fs::write(dir.path().join("DSCF0001.JPG"), &jpeg).unwrap();
        std::fs::write(
            dir.path().join("DSCF0001.RAF"),
            crate::raf::wrap_jpeg_for_tests(&jpeg),
        )
        .unwrap();
        std::fs::write(dir.path().join("IMG_0002.JPG"), &jpeg).unwrap();
        dir
    }

    /// A laptop's own index of the shared folder.
    fn laptop(root: &Path) -> Library {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(root).unwrap();
        scan_source(&lib, &src, true, &|_| {}).unwrap();
        lib
    }

    fn rescan(lib: &Library) -> Summary {
        let src = lib.sources().unwrap().remove(0);
        scan_source(lib, &src, true, &|_| {}).unwrap()
    }

    fn photo(lib: &Library, name: &str) -> crate::db::PhotoRow {
        lib.photos()
            .unwrap()
            .into_iter()
            .find(|p| p.name == name)
            .unwrap()
    }

    fn tick() {
        // Sidecar changes are told apart by modified time.
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    #[test]
    fn laptops_share_ratings_picks_favourites_and_albums() {
        let dir = shared_folder();
        let (a, b) = (laptop(dir.path()), laptop(dir.path()));

        let jpg = photo(&a, "DSCF0001.JPG").id;
        a.set_rating(&[jpg], 4).unwrap();
        a.set_flag(&[jpg], 1).unwrap();
        a.set_favorite(&[jpg], true).unwrap();
        let album = a.create_album("Halifax").unwrap();
        a.add_to_album(album, &[jpg]).unwrap();
        assert_eq!(save_sidecars(&a, &[jpg]).unwrap(), 0);
        tick();

        assert_eq!(rescan(&b).synced, 2, "both halves of the RAW+JPEG pair");
        for name in ["DSCF0001.JPG", "DSCF0001.RAF"] {
            let p = photo(&b, name);
            assert_eq!((p.rating, p.flag, p.favorite), (4, 1, true), "{name}");
        }
        let albums = b.albums().unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].name, "Halifax");
        assert_eq!(albums[0].ids.len(), 2);

        // And back the other way.
        let img = photo(&b, "IMG_0002.JPG").id;
        b.set_rating(&[img], 2).unwrap();
        save_sidecars(&b, &[img]).unwrap();
        tick();
        rescan(&a);
        assert_eq!(photo(&a, "IMG_0002.JPG").rating, 2);
        // Nothing changed since, so nothing is read again.
        assert_eq!(rescan(&a).synced, 0);
    }

    /// What one PC's models found, and the names given there, reach another
    /// through the sidecars: it needn't search those photos, and knows who
    /// is in them. The RAW behind its JPEG gets nothing (as when searched).
    #[test]
    fn laptops_share_faces_names_and_smart_search() {
        use crate::people::{NewFace, Subject};
        let dir = shared_folder();
        let (a, b) = (laptop(dir.path()), laptop(dir.path()));
        let face = |axis: usize, x: f32| NewFace {
            bbox: [x, 0.2, 0.2, 0.3],
            score: 0.9,
            embedding: (0..8).map(|i| if i == axis { 1.0 } else { 0.0 }).collect(),
        };
        let (jpg, img) = (photo(&a, "DSCF0001.JPG").id, photo(&a, "IMG_0002.JPG").id);
        a.store_faces(jpg, "faces-1", &[face(0, 0.1), face(3, 0.6)])
            .unwrap();
        a.store_faces(img, "faces-1", &[face(0, 0.4)]).unwrap();
        a.store_pets(img, "pets-1", &[]).unwrap();
        a.store_embeddings("smart-1", &[(img, vec![9, 8, 7])])
            .unwrap();
        a.group_faces().unwrap();
        let ana = a
            .people()
            .unwrap()
            .into_iter()
            .find(|p| p.photo_ids.len() == 2)
            .unwrap();
        a.name_person(ana.id, "Ana").unwrap();
        // The pair's sidecar has what was found in the JPEG, whichever half
        // the library lists first.
        let raf = photo(&a, "DSCF0001.RAF").id;
        assert!(pair_tags(&a, &[raf, jpg]).unwrap().faces.is_some());
        assert_eq!(save_sidecars(&a, &[jpg, img]).unwrap(), 0);
        tick();

        rescan(&b);
        assert!(
            b.faces_todo("faces-1").unwrap().is_empty(),
            "nothing to search again"
        );
        assert_eq!(
            b.pets_todo("pets-1").unwrap(),
            [photo(&b, "DSCF0001.JPG").id],
            "A looked for pets in the other photo only"
        );
        assert_eq!(
            b.missing_embeddings("smart-1").unwrap().len(),
            1,
            "only the other photo"
        );
        let people = b.people().unwrap();
        let ana_b = people
            .iter()
            .find(|p| p.name.as_deref() == Some("Ana"))
            .unwrap();
        let named: Vec<String> = ana_b
            .photo_ids
            .iter()
            .map(|&id| {
                b.photos()
                    .unwrap()
                    .into_iter()
                    .find(|p| p.id == id)
                    .unwrap()
                    .name
            })
            .collect();
        assert_eq!(named.len(), 2, "{named:?}");
        assert!(!named.iter().any(|n| n.ends_with(".RAF")), "{named:?}");
        let raf = photo(&b, "DSCF0001.RAF").id;
        assert_eq!(b.found(raf, Subject::Person).unwrap(), None);
    }

    /// Libraries from before sidecars carried picks: the first sync merges,
    /// so neither the sidecar's stars nor this laptop's picks are lost.
    #[test]
    fn first_sync_keeps_both_sides() {
        let dir = shared_folder();
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(dir.path()).unwrap();
        scan_source(&lib, &src, false, &|_| {}).unwrap();
        let jpg = photo(&lib, "DSCF0001.JPG").id;
        lib.set_flag(&[jpg], 1).unwrap();
        // What an older version wrote: stars only.
        let old = Tags {
            rating: 3,
            ..Tags::default()
        };
        xmp::write(&dir.path().join("DSCF0001.RAF"), Kind::Raf, &old).unwrap();

        rescan(&lib);
        let p = photo(&lib, "DSCF0001.JPG");
        assert_eq!((p.rating, p.flag), (3, 1));
        // The merged result is queued and then written back.
        assert!(!lib.unsaved_sidecars().unwrap().is_empty());
        save_sidecars(&lib, &lib.unsaved_sidecars().unwrap()).unwrap();
        assert!(lib.unsaved_sidecars().unwrap().is_empty());
        let tags = xmp::read(&dir.path().join("DSCF0001.xmp")).unwrap();
        assert_eq!((tags.rating, tags.flag), (3, 1));
    }

    /// A change made while the share was unreachable isn't overwritten by
    /// the sidecar; it's saved once the share is back.
    #[test]
    fn unsaved_changes_are_kept_and_saved_later() {
        let dir = shared_folder();
        let (a, b) = (laptop(dir.path()), laptop(dir.path()));
        let img = photo(&a, "IMG_0002.JPG").id;

        b.set_rating(&[img], 5).unwrap();
        b.sidecar_saved(&[img], None).unwrap(); // couldn't reach the share
        tick();
        a.set_rating(&[img], 1).unwrap();
        save_sidecars(&a, &[img]).unwrap();
        tick();

        rescan(&b);
        assert_eq!(photo(&b, "IMG_0002.JPG").rating, 5);
        save_sidecars(&b, &b.unsaved_sidecars().unwrap()).unwrap();
        tick();
        rescan(&a);
        assert_eq!(photo(&a, "IMG_0002.JPG").rating, 5);
    }

    /// A photo is forgotten only when its file is confirmed missing, never
    /// because its folder or the whole share couldn't be reached.
    #[test]
    fn only_confirmed_deletions_count() {
        let dir = tempfile::tempdir().unwrap();
        let day = dir.path().join("2026/10-21");
        std::fs::create_dir_all(&day).unwrap();
        let kept = day.join("DSCF0001.JPG");
        std::fs::write(&kept, b"x").unwrap();
        assert!(!really_gone(&kept));
        // Deleted file, deleted folder: gone.
        assert!(really_gone(&day.join("DSCF0002.JPG")));
        assert!(really_gone(&dir.path().join("2026/10-22/DSCF0003.JPG")));
        // A share that has vanished altogether: not gone, just unreachable.
        assert!(!really_gone(Path::new(
            "/no-such-share/Photos/2026/10-21/DSCF0004.JPG"
        )));
    }

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
        let s = scan_source(&lib, &src, true, &|_| {}).unwrap();
        assert_eq!((s.added, s.updated, s.removed), (2, 0, 0));
        let rows = lib.photos().unwrap();
        assert!(rows.iter().all(|r| r.width == 64 && r.height == 48));

        let s = scan_source(&lib, &src, true, &|_| {}).unwrap();
        assert_eq!((s.added, s.updated, s.removed), (0, 0, 0));

        std::fs::remove_file(root.join("2026/10-21/DSCF0001.RAF")).unwrap();
        let s = scan_source(&lib, &src, true, &|_| {}).unwrap();
        assert_eq!(s.removed, 1);
        assert_eq!(lib.photos().unwrap().len(), 1);
    }

    /// Scans and old photos with no date of their own are dated by their
    /// folders; photos that say when they were taken keep that.
    #[test]
    fn photos_without_a_date_take_their_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("1998/07 July")).unwrap();
        std::fs::create_dir_all(root.join("Scans")).unwrap();
        let undated = crate::test_jpeg(64, 48, None);
        std::fs::write(root.join("1998/07 July/scan01.jpg"), &undated).unwrap();
        std::fs::write(root.join("Scans/IMG_20050102_101500.jpg"), &undated).unwrap();
        std::fs::write(root.join("Scans/loose.jpg"), &undated).unwrap();

        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(root).unwrap();
        scan_source(&lib, &src, false, &|_| {}).unwrap();
        let date = |name: &str| {
            let p = lib
                .photos()
                .unwrap()
                .into_iter()
                .find(|p| p.name == name)
                .unwrap();
            let t = chrono::DateTime::from_timestamp(p.taken_at.unwrap(), 0).unwrap();
            (
                t.format("%Y-%m-%d").to_string(),
                p.mtime,
                p.taken_at.unwrap(),
            )
        };
        assert_eq!(date("scan01.jpg").0, "1998-07-01");
        assert_eq!(date("IMG_20050102_101500.jpg").0, "2005-01-02");
        let (_, mtime, taken) = date("loose.jpg");
        assert_eq!(taken, mtime, "nothing better than the file's time");

        // A library indexed before folders were read: re-dated on the next
        // scan, without reading the files again.
        let id = lib
            .photos()
            .unwrap()
            .into_iter()
            .find(|p| p.name == "scan01.jpg")
            .unwrap()
            .id;
        lib.conn
            .lock()
            .execute("UPDATE photos SET taken_at = mtime WHERE id = ?1", [id])
            .unwrap();
        let s = scan_source(&lib, &src, false, &|_| {}).unwrap();
        assert_eq!((s.added, s.updated), (0, 0));
        assert_eq!(date("scan01.jpg").0, "1998-07-01");
    }
}
