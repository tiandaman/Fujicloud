mod backup;
mod logfile;
mod protocol;
mod reach;
mod settings;
mod sidecars;
mod smart;
mod sync;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fuji_core::db::{AlbumRow, PhotoDetail, PhotoRow, RecipeRow, SourceRow, Touched};
use fuji_core::thumbs::{self, Size};
use fuji_core::{Kind, Library, discard, dupes, export, ingest, meta, scan, xmp};
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use settings::Settings;
use tauri::{AppHandle, Emitter, Manager, State};

pub struct AppState {
    pub lib: Library,
    pub cache: thumbs::Cache,
    pub queue: Arc<protocol::Queue>,
    pub settings: RwLock<Settings>,
    settings_path: PathBuf,
    effects: AtomicBool,
    scanning: AtomicBool,
    warming: AtomicBool,
    sync: Mutex<Option<sync::InboxSync>>,
    pub models_dir: PathBuf,
    pub smart: smart::Smart,
    pub reach: reach::Reach,
    pub backup: backup::Backup,
}

type Shared<'a> = State<'a, Arc<AppState>>;
type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Refuses to touch a file whose source is offline (it would hang for half
/// a minute and then fail anyway).
fn reachable(state: &AppState, path: &Path) -> CmdResult<()> {
    if !state.reach.covers(path) {
        return Ok(());
    }
    let label = state
        .lib
        .source_for(path)
        .ok()
        .flatten()
        .map_or_else(|| "its folder".to_owned(), |s| s.label);
    Err(format!(
        "{label} can't be reached right now. Is the Shield switched on?"
    ))
}

// ---- library ---------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    sources: Vec<SourceInfo>,
    settings: Settings,
    effects: bool,
    editor: Option<String>,
    smart: smart::Status,
    backup: backup::Status,
}

#[tauri::command]
fn bootstrap(state: Shared) -> CmdResult<Bootstrap> {
    let settings = state.settings.read().clone();
    let editor = settings
        .editor_path
        .clone()
        .or_else(settings::find_photoshop);
    Ok(Bootstrap {
        sources: sources(&state)?,
        settings,
        effects: state.effects.load(Ordering::Relaxed),
        editor: editor.map(|p| p.to_string_lossy().into_owned()),
        smart: state.smart.status(&state),
        backup: state.backup.status(&state),
    })
}

#[tauri::command]
async fn list_photos(state: Shared<'_>) -> CmdResult<Vec<PhotoRow>> {
    state.lib.photos().map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceInfo {
    #[serde(flatten)]
    row: SourceRow,
    /// Can't be reached right now; its photos browse from the cache.
    offline: bool,
}

fn sources(state: &AppState) -> CmdResult<Vec<SourceInfo>> {
    Ok(state
        .lib
        .sources()
        .map_err(err)?
        .into_iter()
        .map(|row| SourceInfo {
            offline: state.reach.is_offline(row.id),
            row,
        })
        .collect())
}

#[tauri::command]
fn list_sources(state: Shared) -> CmdResult<Vec<SourceInfo>> {
    sources(&state)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DetailWithExif {
    #[serde(flatten)]
    detail: PhotoDetail,
    exif: Vec<(String, String)>,
}

#[tauri::command]
async fn photo_detail(state: Shared<'_>, id: i64) -> CmdResult<DetailWithExif> {
    let detail = state.lib.photo(id).map_err(err)?;
    let path = PathBuf::from(&detail.path);
    let kind = detail.row.kind;
    if state.reach.covers(&path) {
        return Ok(DetailWithExif {
            detail,
            exif: Vec::new(),
        });
    }
    // Reading EXIF touches the network share, so keep it off the IPC thread.
    let exif = tauri::async_runtime::spawn_blocking(move || {
        meta::all_fields(&path, kind).unwrap_or_default()
    })
    .await
    .map_err(err)?;
    Ok(DetailWithExif { detail, exif })
}

#[tauri::command]
fn add_source(app: AppHandle, state: Shared, path: String) -> CmdResult<SourceInfo> {
    let path = PathBuf::from(path.trim());
    if !path.is_dir() {
        return Err(format!(
            "Can't reach {}. Is the drive shared and online?",
            path.display()
        ));
    }
    let row = state.lib.add_source(&path).map_err(err)?;
    spawn_scan(app, state.inner().clone());
    Ok(SourceInfo {
        row,
        offline: false,
    })
}

#[tauri::command]
fn remove_source(app: AppHandle, state: Shared, id: i64) -> CmdResult<()> {
    state.lib.remove_source(id).map_err(err)?;
    let _ = app.emit("library-changed", ());
    Ok(())
}

#[tauri::command]
fn rename_source(state: Shared, id: i64, label: String) -> CmdResult<()> {
    state.lib.rename_source(id, label.trim()).map_err(err)
}

#[tauri::command]
fn rescan(app: AppHandle, state: Shared) {
    spawn_scan(app, state.inner().clone());
}

/// Rescans every source in the background, then warms the thumbnail cache.
pub(crate) fn spawn_scan(app: AppHandle, state: Arc<AppState>) {
    if state.scanning.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let sources = state.lib.sources().unwrap_or_default();
        let mut last_refresh = Instant::now();
        for source in &sources {
            // An offline source keeps its photos; it's rescanned when it's back.
            if !reach::check(&app, &state, source.id, Path::new(&source.path)) {
                continue;
            }
            let progress = |p: scan::Progress| {
                let _ = app.emit("scan", (&source.label, &p));
            };
            let refresh = Mutex::new(&mut last_refresh);
            let sidecars = state.settings.read().write_xmp;
            let result = fuji_core::guard(|| {
                scan::scan_source(&state.lib, source, sidecars, &|p| {
                    progress(p);
                    // Let the grid fill in progressively on big first scans.
                    let mut last = refresh.lock();
                    if last.elapsed() > Duration::from_millis(1500) {
                        **last = Instant::now();
                        let _ = app.emit("library-changed", ());
                    }
                })
            });
            match result {
                Ok(summary) => log::info!("scanned {}: {summary:?}", source.label),
                Err(e) => {
                    let _ = app.emit("scan-error", format!("{}: {e}", source.label));
                }
            }
        }
        state.scanning.store(false, Ordering::Release);
        let _ = app.emit("scan-done", ());
        let _ = app.emit("library-changed", ());
        smart::spawn(app.clone(), state.clone());
        spawn_warm(app, state);
    });
}

#[derive(Clone, Serialize)]
struct Warm {
    done: usize,
    total: usize,
}

/// Pre-builds grid thumbnails on the low-priority lane so scrolling is
/// instant even through parts of the library you haven't visited yet.
fn spawn_warm(app: AppHandle, state: Arc<AppState>) {
    if state.warming.swap(true, Ordering::AcqRel) {
        return;
    }
    let photos = state.lib.photos().unwrap_or_default();
    let missing: Vec<i64> = photos
        .iter()
        // A RAW or Live Photo movie stacked under its picture isn't shown in
        // the grid; build its thumbnail on demand rather than pulling it
        // over the network now.
        .filter(|p| !((p.kind.is_raw() || p.kind == Kind::Video) && p.pair_id.is_some()))
        .filter(|p| {
            let path = PathBuf::from(&p.folder).join(&p.name);
            if state.reach.covers(&path) {
                return false;
            }
            let src = thumbs::Source {
                path: &path,
                kind: p.kind,
                size: p.size as u64,
                mtime: p.mtime,
            };
            state.cache.get(&src, Size::Thumb).is_none()
        })
        .map(|p| p.id)
        .collect();
    if missing.is_empty() {
        state.warming.store(false, Ordering::Release);
        return;
    }
    let total = missing.len();
    let done = Arc::new(AtomicUsize::new(0));
    let _ = app.emit("warm", Warm { done: 0, total });
    for id in missing {
        let (app, state, done) = (app.clone(), state.clone(), done.clone());
        state.queue.clone().push(protocol::Lane::Low, move || {
            if let Ok((path, kind, size, mtime)) = state.lib.file_of(id) {
                let src = thumbs::Source {
                    path: &path,
                    kind,
                    size,
                    mtime,
                };
                let _ = state.cache.ensure(&src, Size::Thumb);
            }
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 || n == total {
                let _ = app.emit("warm", Warm { done: n, total });
            }
            if n == total {
                state.warming.store(false, Ordering::Release);
            }
        });
    }
}

// ---- culling ---------------------------------------------------------------

fn mirror_to_xmp(touched: &[Touched]) -> Vec<i64> {
    let ids: Vec<i64> = touched.iter().map(|t| t.id).collect();
    sidecars::save(&ids);
    ids
}

#[tauri::command]
fn set_rating(state: Shared, ids: Vec<i64>, rating: i32) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_rating(&ids, rating).map_err(err)?;
    Ok(mirror_to_xmp(&touched))
}

#[tauri::command]
fn set_flag(state: Shared, ids: Vec<i64>, flag: i32) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_flag(&ids, flag).map_err(err)?;
    Ok(mirror_to_xmp(&touched))
}

#[tauri::command]
fn set_label(state: Shared, ids: Vec<i64>, label: Option<String>) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_label(&ids, label.as_deref()).map_err(err)?;
    Ok(mirror_to_xmp(&touched))
}

#[tauri::command]
fn set_favorite(state: Shared, ids: Vec<i64>, favorite: bool) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_favorite(&ids, favorite).map_err(err)?;
    Ok(mirror_to_xmp(&touched))
}

// ---- albums ----------------------------------------------------------------

#[tauri::command]
fn list_albums(state: Shared) -> CmdResult<Vec<AlbumRow>> {
    state.lib.albums().map_err(err)
}

/// Creates an album (or finds the one with that name) and adds `ids` to it.
#[tauri::command]
fn create_album(state: Shared, name: String, ids: Vec<i64>) -> CmdResult<i64> {
    let name = name.trim();
    if name.is_empty() {
        return Err("An album needs a name".into());
    }
    let id = state.lib.create_album(name).map_err(err)?;
    sidecars::save(&state.lib.add_to_album(id, &ids).map_err(err)?);
    Ok(id)
}

#[tauri::command]
fn rename_album(state: Shared, id: i64, name: String) -> CmdResult<()> {
    if name.trim().is_empty() {
        return Err("An album needs a name".into());
    }
    sidecars::save(&state.lib.rename_album(id, &name).map_err(err)?);
    Ok(())
}

#[tauri::command]
fn delete_album(state: Shared, id: i64) -> CmdResult<()> {
    sidecars::save(&state.lib.delete_album(id).map_err(err)?);
    Ok(())
}

#[tauri::command]
fn add_to_album(state: Shared, id: i64, ids: Vec<i64>) -> CmdResult<()> {
    sidecars::save(&state.lib.add_to_album(id, &ids).map_err(err)?);
    Ok(())
}

#[tauri::command]
fn remove_from_album(state: Shared, id: i64, ids: Vec<i64>) -> CmdResult<()> {
    sidecars::save(&state.lib.remove_from_album(id, &ids).map_err(err)?);
    Ok(())
}

/// Copies of the same file, and near-identical shots (with smart search).
#[tauri::command]
async fn find_duplicates(state: Shared<'_>) -> CmdResult<Vec<dupes::Group>> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let rows = state.lib.photos().map_err(err)?;
        let index = state.smart.vectors(&state);
        let checksum = |path: &Path| {
            // Reading a file on a share that's gone would hang.
            (!state.reach.covers(path))
                .then(|| ingest::checksum(path).ok())
                .flatten()
        };
        Ok(dupes::find(&rows, index.as_deref(), &checksum))
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Deleted {
    /// Photos taken out of the library (RAW+JPEG twins included).
    ids: Vec<i64>,
    /// Where they went: the Recycle Bin, or a library folder's trash.
    places: Vec<String>,
    /// Names of photos that couldn't be moved.
    failed: Vec<String>,
}

/// Moves photos (and their twins and sidecars) to the Recycle Bin, or for
/// network drives to `.fujicloud-trash` in their library folder.
#[tauri::command]
async fn delete_photos(app: AppHandle, state: Shared<'_>, ids: Vec<i64>) -> CmdResult<Deleted> {
    let state = state.inner().clone();
    let deleted = tauri::async_runtime::spawn_blocking(move || discard_photos(&state, &ids))
        .await
        .map_err(err)??;
    if !deleted.ids.is_empty() {
        let _ = app.emit("library-changed", ());
    }
    Ok(deleted)
}

fn discard_photos(state: &AppState, ids: &[i64]) -> CmdResult<Deleted> {
    let ids = state.lib.with_pairs(ids).map_err(err)?;
    let sources = state.lib.sources().map_err(err)?;
    let mut deleted = Deleted {
        ids: Vec::new(),
        places: Vec::new(),
        failed: Vec::new(),
    };
    for id in ids {
        let Ok(info) = state.lib.file_info(id) else {
            continue;
        };
        let name = info
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let source = sources.iter().find(|s| s.id == info.source_id);
        let (Some(source), false) = (source, state.reach.is_offline(info.source_id)) else {
            deleted.failed.push(name);
            continue;
        };
        let root = Path::new(&source.path);
        // A RAW+JPEG pair shares one sidecar; whichever goes first takes it.
        let sidecar = xmp::sidecar_path(&info.path, info.kind);
        let files: Vec<&Path> = [info.path.as_path(), sidecar.as_path()]
            .into_iter()
            .filter(|f| f.exists())
            .collect();
        let result = if files.is_empty() {
            Ok("nowhere".to_owned()) // already gone
        } else if recycles(root) {
            trash::delete_all(&files)
                .map(|()| "the Recycle Bin".to_owned())
                .map_err(err)
        } else {
            files
                .iter()
                .try_for_each(|f| discard::to_trash_folder(root, f).map(drop))
                .map(|()| discard::trash_dir(root).display().to_string())
                .map_err(err)
        };
        match result {
            Ok(place) => {
                if !deleted.places.contains(&place) && place != "nowhere" {
                    deleted.places.push(place);
                }
                deleted.ids.push(id);
            }
            Err(e) => {
                log::warn!("couldn't delete {}: {e}", info.path.display());
                deleted.failed.push(name);
            }
        }
    }
    state.lib.remove_photos(&deleted.ids).map_err(err)?;
    log::info!(
        "deleted {} photos to {:?}; {} failed",
        deleted.ids.len(),
        deleted.places,
        deleted.failed.len()
    );
    Ok(deleted)
}

/// Whether Windows keeps files deleted from this folder in the Recycle Bin.
/// Only fixed drives do: from a network share or a USB stick they'd be gone.
#[cfg(windows)]
fn recycles(path: &Path) -> bool {
    let path = path.to_string_lossy();
    let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
    let Some(drive) = path.get(..3).filter(|d| d.ends_with(":\\")) else {
        return false; // \\server\share
    };
    let drive: Vec<u16> = drive.encode_utf16().chain([0]).collect();
    // SAFETY: a NUL-terminated root path like `C:\`.
    let kind = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(drive.as_ptr()) };
    kind == 3 // DRIVE_FIXED
}

#[cfg(not(windows))]
fn recycles(_: &Path) -> bool {
    true
}

// ---- hand-off to other apps -------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Exported {
    /// Files written.
    files: usize,
    /// Names of photos that couldn't be exported.
    failed: Vec<String>,
}

/// Exports photos into `folder` (see [`export::Preset`]), two at a time so
/// big decodes don't swamp memory. Emits `export` progress.
#[tauri::command]
async fn export_photos(
    app: AppHandle,
    state: Shared<'_>,
    ids: Vec<i64>,
    preset: export::Preset,
    location: bool,
    folder: String,
) -> CmdResult<Exported> {
    let state = state.inner().clone();
    let folder = PathBuf::from(folder);
    tauri::async_runtime::spawn_blocking(move || {
        use rayon::prelude::*;
        let rows = state.lib.photos().map_err(err)?;
        let by_id: std::collections::HashMap<i64, &PhotoRow> =
            rows.iter().map(|p| (p.id, p)).collect();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .map_err(err)?;
        let done = AtomicUsize::new(0);
        let total = ids.len();
        let results: Vec<Result<usize, String>> = pool.install(|| {
            ids.par_iter()
                .map(|id| {
                    let row = by_id.get(id).ok_or_else(|| format!("photo {id}"))?;
                    let path = PathBuf::from(&row.folder).join(&row.name);
                    let name = row.name.clone();
                    reachable(&state, &path).map_err(|_| name.clone())?;
                    let twin = row
                        .pair_id
                        .and_then(|t| by_id.get(&t))
                        .map(|t| PathBuf::from(&t.folder).join(&t.name));
                    let item = export::Item {
                        path,
                        kind: row.kind,
                        info: export::Info::from(*row),
                        twin,
                    };
                    let result = export::export(&item, preset, location, &folder)
                        .map(|files| files.len())
                        .map_err(|e| {
                            log::warn!("export {name}: {e}");
                            name
                        });
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = app.emit("export", Warm { done: n, total });
                    result
                })
                .collect()
        });
        Ok(Exported {
            files: results.iter().filter_map(|r| r.as_ref().ok()).sum(),
            failed: results.into_iter().filter_map(Result::err).collect(),
        })
    })
    .await
    .map_err(err)?
}

#[derive(Serialize)]
struct DragFiles {
    paths: Vec<String>,
    /// A thumbnail shown under the pointer while dragging.
    icon: String,
}

/// The files behind photos dragged out of the window, and an image to show.
#[tauri::command]
async fn drag_files(state: Shared<'_>, ids: Vec<i64>) -> CmdResult<DragFiles> {
    let mut paths = Vec::new();
    let mut icon = None;
    for id in ids {
        let (path, kind, size, mtime) = state.lib.file_of(id).map_err(err)?;
        reachable(&state, &path)?;
        if icon.is_none() {
            let src = thumbs::Source {
                path: &path,
                kind,
                size,
                mtime,
            };
            icon = state
                .cache
                .get(&src, Size::Thumb)
                .or_else(|| state.cache.ensure(&src, Size::Thumb).ok());
        }
        paths.push(path.to_string_lossy().into_owned());
    }
    Ok(DragFiles {
        paths,
        icon: icon
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    })
}

#[tauri::command]
fn open_folder(path: String) -> CmdResult<()> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(err)
}

/// Opens photos in Photoshop (Camera Raw for RAFs), falling back to the
/// default app. For RAW+JPEG pairs the RAW is opened if `edit_raw` is set.
#[tauri::command]
fn open_in_editor(state: Shared, ids: Vec<i64>) -> CmdResult<String> {
    let settings = state.settings.read().clone();
    let mut paths = Vec::new();
    for id in ids {
        let detail = state.lib.photo(id).map_err(err)?;
        reachable(&state, Path::new(&detail.path))?;
        let mut path = detail.path.clone();
        if settings.edit_raw
            && !detail.row.kind.is_raw()
            && let Some(pair) = detail.row.pair_id
            && let Ok(other) = state.lib.photo(pair)
            && other.row.kind.is_raw()
        {
            path = other.path;
        }
        paths.push(path);
    }
    match settings.editor_path.or_else(settings::find_photoshop) {
        Some(editor) => {
            std::process::Command::new(&editor)
                .args(&paths)
                .spawn()
                .map_err(err)?;
            Ok(editor
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default())
        }
        None => {
            for p in &paths {
                tauri_plugin_opener::open_path(p, None::<&str>).map_err(err)?;
            }
            Ok("default app".into())
        }
    }
}

#[tauri::command]
fn reveal(state: Shared, id: i64) -> CmdResult<()> {
    let detail = state.lib.photo(id).map_err(err)?;
    reachable(&state, Path::new(&detail.path))?;
    tauri_plugin_opener::reveal_item_in_dir(&detail.path).map_err(err)
}

#[tauri::command]
fn open_default(state: Shared, id: i64) -> CmdResult<()> {
    let detail = state.lib.photo(id).map_err(err)?;
    reachable(&state, Path::new(&detail.path))?;
    tauri_plugin_opener::open_path(&detail.path, None::<&str>).map_err(err)
}

// ---- recipes ------------------------------------------------------------------

#[tauri::command]
async fn list_recipes(state: Shared<'_>) -> CmdResult<Vec<RecipeRow>> {
    state.lib.recipes().map_err(err)
}

#[tauri::command]
fn name_recipe(state: Shared, key: String, name: Option<String>) -> CmdResult<()> {
    state.lib.name_recipe(&key, name.as_deref()).map_err(err)
}

// ---- smart search -----------------------------------------------------------

#[tauri::command]
fn smart_status(state: Shared) -> smart::Status {
    state.smart.status(&state)
}

#[tauri::command]
fn set_smart_search(app: AppHandle, state: Shared, enabled: bool) -> CmdResult<smart::Status> {
    {
        let mut settings = state.settings.write();
        settings.smart_search = enabled;
        settings.save(&state.settings_path).map_err(err)?;
    }
    if enabled {
        smart::spawn(app, state.inner().clone());
    } else {
        state.smart.disable(&app, &state);
    }
    Ok(state.smart.status(&state))
}

/// Switches photo analysis between the graphics card and the CPU.
#[tauri::command]
fn set_smart_gpu(app: AppHandle, state: Shared, enabled: bool) -> CmdResult<smart::Status> {
    {
        let mut settings = state.settings.write();
        settings.smart_gpu = enabled;
        settings.save(&state.settings_path).map_err(err)?;
    }
    state.smart.reset_device();
    smart::spawn(app, state.inner().clone());
    Ok(state.smart.status(&state))
}

#[derive(Serialize)]
struct Hit {
    id: i64,
    score: f32,
}

fn hits(found: Vec<(i64, f32)>) -> Vec<Hit> {
    found
        .into_iter()
        .map(|(id, score)| Hit { id, score })
        .collect()
}

/// Photos matching a description, best first. Embedding the query (and, the
/// first time, loading the model) takes a moment, so it runs off the IPC thread.
#[tauri::command]
async fn search_smart(state: Shared<'_>, query: String) -> CmdResult<Vec<Hit>> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.smart.search(&state, &query))
        .await
        .map_err(err)?
        .map(hits)
}

#[tauri::command]
async fn similar_photos(state: Shared<'_>, id: i64) -> CmdResult<Vec<Hit>> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || state.smart.similar(&state, id))
        .await
        .map_err(err)?
        .map(hits)
}

// ---- settings & sync ---------------------------------------------------------

#[tauri::command]
fn save_settings(app: AppHandle, state: Shared, mut settings: Settings) -> CmdResult<()> {
    // Smart search is switched by its own commands (they download and index).
    settings.smart_search = state.settings.read().smart_search;
    settings.smart_gpu = state.settings.read().smart_gpu;
    settings.cache_version = state.settings.read().cache_version;
    settings.migrate();
    settings.save(&state.settings_path).map_err(err)?;
    let new_backup = settings.backup_to != state.settings.read().backup_to;
    *state.settings.write() = settings;
    if new_backup {
        backup::now();
        let _ = app.emit("backup", state.backup.status(&state));
    }
    restart_sync(&app, state.inner())
}

#[tauri::command]
fn backup_status(state: Shared) -> backup::Status {
    state.backup.status(&state)
}

#[tauri::command]
fn backup_now() {
    backup::now();
}

#[tauri::command]
fn suggest_inboxes() -> Vec<String> {
    settings::suggested_inboxes()
        .into_iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

fn restart_sync(app: &AppHandle, state: &Arc<AppState>) -> CmdResult<()> {
    let mut slot = state.sync.lock();
    *slot = None;
    let settings = state.settings.read().clone();
    let (present, missing): (Vec<_>, Vec<_>) =
        settings.inboxes.into_iter().partition(|i| i.path.is_dir());
    let Some(library) = settings.import_to.filter(|_| !present.is_empty()) else {
        let _ = app.emit("sync", serde_json::json!({ "active": false }));
        return match missing.first() {
            Some(i) => Err(format!("Inbox folder {} doesn't exist", i.path.display())),
            None => Ok(()),
        };
    };
    // New shots should show up in the library view immediately. The folder
    // itself is made by the sync thread: the Shield may be asleep, and
    // waiting on it here would freeze the window.
    if state.lib.source_for(&library).map_err(err)?.is_none() {
        state.lib.add_source(&library).map_err(err)?;
    }
    *slot = Some(sync::start(app.clone(), state.clone(), present, library).map_err(err)?);
    match missing.first() {
        Some(i) => Err(format!("Inbox folder {} doesn't exist", i.path.display())),
        None => Ok(()),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheInfo {
    bytes: u64,
    path: String,
    backlog: usize,
}

#[tauri::command]
async fn cache_info(state: Shared<'_>) -> CmdResult<CacheInfo> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || CacheInfo {
        bytes: state.cache.disk_usage(),
        path: state.cache.root().to_string_lossy().into_owned(),
        backlog: state.queue.backlog(),
    })
    .await
    .map_err(err)
}

/// Throws away every thumbnail and preview and builds them again, e.g. after
/// an update that shows some photos better. Returns the new cache version.
#[tauri::command]
fn rebuild_thumbnails(app: AppHandle, state: Shared) -> CmdResult<u32> {
    state.cache.clear().map_err(err)?;
    let version = {
        let mut settings = state.settings.write();
        settings.cache_version += 1;
        settings.save(&state.settings_path).map_err(err)?;
        settings.cache_version
    };
    log::info!("rebuilding thumbnails");
    spawn_warm(app, state.inner().clone());
    Ok(version)
}

#[tauri::command]
fn open_log_folder(app: AppHandle) -> CmdResult<()> {
    let dir = app.path().app_local_data_dir().map_err(err)?.join("logs");
    std::fs::create_dir_all(&dir).map_err(err)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(err)
}

// ---- app ---------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_drag::init())
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol("fuji", |ctx, request, responder| {
            let Some(state) = ctx.app_handle().try_state::<Arc<AppState>>() else {
                responder.respond(
                    tauri::http::Response::builder()
                        .status(503)
                        .body(Vec::new())
                        .unwrap(),
                );
                return;
            };
            let state = state.inner().clone();
            let lane = protocol::lane_for(&request);
            state.queue.clone().push(lane, move || {
                responder.respond(protocol::handle(&state, &request))
            });
        })
        .setup(|app| {
            let data = app.path().app_local_data_dir()?;
            logfile::init(&data.join("logs"));
            let config = app.path().app_config_dir()?;
            let settings_path = config.join("settings.json");
            let workers = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4);
            let state = Arc::new(AppState {
                lib: Library::open(&data.join("library.db"))?,
                cache: thumbs::Cache::new(data.join("cache"))?,
                // Leave a core for the UI; decoding is CPU-bound.
                queue: protocol::Queue::start(workers.saturating_sub(1)),
                settings: RwLock::new(Settings::load(&settings_path)),
                settings_path,
                effects: AtomicBool::new(false),
                scanning: AtomicBool::new(false),
                warming: AtomicBool::new(false),
                sync: Mutex::new(None),
                models_dir: data.join("models"),
                smart: smart::Smart::default(),
                reach: reach::Reach::default(),
                backup: backup::Backup::new(data.join("backup.json")),
            });

            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                // Windows 11 Mica: the desktop tint shows through the sidebar.
                let mica = window_vibrancy::apply_mica(&window, Some(true)).is_ok();
                state.effects.store(mica, Ordering::Relaxed);
            }

            app.manage(state.clone());
            sidecars::start(state.clone());
            sidecars::flush(&state);
            let handle = app.handle().clone();
            if let Err(e) = restart_sync(&handle, &state) {
                log::warn!("sync: {e}");
            }
            // Right away, not after the scan: the UI already says the model
            // is downloading, and on a big network library the scan is slow.
            smart::spawn(handle.clone(), state.clone());
            reach::spawn(handle.clone(), state.clone());
            backup::start(handle.clone(), state.clone());
            spawn_scan(handle, state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap,
            list_photos,
            list_sources,
            photo_detail,
            add_source,
            remove_source,
            rename_source,
            rescan,
            set_rating,
            set_flag,
            set_label,
            set_favorite,
            list_albums,
            create_album,
            rename_album,
            delete_album,
            add_to_album,
            remove_from_album,
            open_in_editor,
            reveal,
            open_default,
            save_settings,
            suggest_inboxes,
            backup_status,
            backup_now,
            cache_info,
            rebuild_thumbnails,
            delete_photos,
            find_duplicates,
            export_photos,
            drag_files,
            open_folder,
            open_log_folder,
            list_recipes,
            name_recipe,
            smart_status,
            set_smart_search,
            set_smart_gpu,
            search_smart,
            similar_photos,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Fujicloud");
}
