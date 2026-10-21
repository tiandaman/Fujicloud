mod protocol;
mod settings;
mod smart;
mod sync;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fuji_core::db::{PhotoDetail, PhotoRow, RecipeRow, SourceRow, Touched};
use fuji_core::thumbs::{self, Size};
use fuji_core::{Library, meta, scan, xmp};
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
}

type Shared<'a> = State<'a, Arc<AppState>>;
type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---- library ---------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    sources: Vec<SourceRow>,
    settings: Settings,
    effects: bool,
    editor: Option<String>,
    smart: smart::Status,
}

#[tauri::command]
fn bootstrap(state: Shared) -> CmdResult<Bootstrap> {
    let settings = state.settings.read().clone();
    let editor = settings
        .editor_path
        .clone()
        .or_else(settings::find_photoshop);
    Ok(Bootstrap {
        sources: state.lib.sources().map_err(err)?,
        settings,
        effects: state.effects.load(Ordering::Relaxed),
        editor: editor.map(|p| p.to_string_lossy().into_owned()),
        smart: state.smart.status(&state),
    })
}

#[tauri::command]
async fn list_photos(state: Shared<'_>) -> CmdResult<Vec<PhotoRow>> {
    state.lib.photos().map_err(err)
}

#[tauri::command]
fn list_sources(state: Shared) -> CmdResult<Vec<SourceRow>> {
    state.lib.sources().map_err(err)
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
    // Reading EXIF touches the network share, so keep it off the IPC thread.
    let exif = tauri::async_runtime::spawn_blocking(move || {
        meta::all_fields(&path, kind).unwrap_or_default()
    })
    .await
    .map_err(err)?;
    Ok(DetailWithExif { detail, exif })
}

#[tauri::command]
fn add_source(app: AppHandle, state: Shared, path: String) -> CmdResult<SourceRow> {
    let path = PathBuf::from(path.trim());
    if !path.is_dir() {
        return Err(format!(
            "Can't reach {}. Is the drive shared and online?",
            path.display()
        ));
    }
    let source = state.lib.add_source(&path).map_err(err)?;
    spawn_scan(app, state.inner().clone());
    Ok(source)
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
fn spawn_scan(app: AppHandle, state: Arc<AppState>) {
    if state.scanning.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let sources = state.lib.sources().unwrap_or_default();
        let mut last_refresh = Instant::now();
        for source in &sources {
            let progress = |p: scan::Progress| {
                let _ = app.emit("scan", (&source.label, &p));
            };
            let refresh = Mutex::new(&mut last_refresh);
            let result = scan::scan_source(&state.lib, source, &|p| {
                progress(p);
                // Let the grid fill in progressively on big first scans.
                let mut last = refresh.lock();
                if last.elapsed() > Duration::from_millis(1500) {
                    **last = Instant::now();
                    let _ = app.emit("library-changed", ());
                }
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
        .filter(|p| p.kind.has_preview())
        // A RAF stacked under its JPEG twin isn't shown in the grid; build
        // its thumbnail on demand rather than pulling it over the network now.
        .filter(|p| !(p.kind.is_raw() && p.pair_id.is_some()))
        .filter(|p| {
            let path = PathBuf::from(&p.folder).join(&p.name);
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

fn mirror_to_xmp(state: &AppState, touched: &[Touched]) -> Vec<i64> {
    if state.settings.read().write_xmp {
        let raws: Vec<Touched> = touched
            .iter()
            .filter(|t| t.kind.is_raw())
            .cloned()
            .collect();
        if !raws.is_empty() {
            std::thread::spawn(move || {
                for t in raws {
                    if let Err(e) = xmp::write_sidecar(&t.path, t.rating, t.label.as_deref()) {
                        log::warn!("xmp {}: {e}", t.path.display());
                    }
                }
            });
        }
    }
    touched.iter().map(|t| t.id).collect()
}

#[tauri::command]
fn set_rating(state: Shared, ids: Vec<i64>, rating: i32) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_rating(&ids, rating).map_err(err)?;
    Ok(mirror_to_xmp(&state, &touched))
}

#[tauri::command]
fn set_flag(state: Shared, ids: Vec<i64>, flag: i32) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_flag(&ids, flag).map_err(err)?;
    Ok(touched.iter().map(|t| t.id).collect())
}

#[tauri::command]
fn set_label(state: Shared, ids: Vec<i64>, label: Option<String>) -> CmdResult<Vec<i64>> {
    let touched = state.lib.set_label(&ids, label.as_deref()).map_err(err)?;
    Ok(mirror_to_xmp(&state, &touched))
}

// ---- hand-off to other apps -------------------------------------------------

/// Opens photos in Photoshop (Camera Raw for RAFs), falling back to the
/// default app. For RAW+JPEG pairs the RAW is opened if `edit_raw` is set.
#[tauri::command]
fn open_in_editor(state: Shared, ids: Vec<i64>) -> CmdResult<String> {
    let settings = state.settings.read().clone();
    let mut paths = Vec::new();
    for id in ids {
        let detail = state.lib.photo(id).map_err(err)?;
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
    tauri_plugin_opener::reveal_item_in_dir(&detail.path).map_err(err)
}

#[tauri::command]
fn open_default(state: Shared, id: i64) -> CmdResult<()> {
    let detail = state.lib.photo(id).map_err(err)?;
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
    settings.save(&state.settings_path).map_err(err)?;
    *state.settings.write() = settings;
    restart_sync(&app, state.inner())
}

fn restart_sync(app: &AppHandle, state: &Arc<AppState>) -> CmdResult<()> {
    let mut slot = state.sync.lock();
    *slot = None;
    let settings = state.settings.read().clone();
    let (Some(inbox), Some(library)) = (settings.inbox, settings.import_to) else {
        let _ = app.emit("sync", serde_json::json!({ "active": false }));
        return Ok(());
    };
    if !inbox.is_dir() {
        return Err(format!("Inbox folder {} doesn't exist", inbox.display()));
    }
    std::fs::create_dir_all(&library)
        .map_err(|e| format!("Can't write to {}: {e}", library.display()))?;
    // New shots should show up in the library view immediately.
    if state.lib.source_for(&library).map_err(err)?.is_none() {
        state.lib.add_source(&library).map_err(err)?;
    }
    *slot = Some(sync::start(app.clone(), state.clone(), inbox, library).map_err(err)?);
    Ok(())
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

#[tauri::command]
fn clear_cache(state: Shared) -> CmdResult<()> {
    state.cache.clear().map_err(err)
}

// ---- app ---------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            });

            #[cfg(windows)]
            if let Some(window) = app.get_webview_window("main") {
                // Windows 11 Mica: the desktop tint shows through the sidebar.
                let mica = window_vibrancy::apply_mica(&window, Some(true)).is_ok();
                state.effects.store(mica, Ordering::Relaxed);
            }

            app.manage(state.clone());
            let handle = app.handle().clone();
            if let Err(e) = restart_sync(&handle, &state) {
                log::warn!("sync: {e}");
            }
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
            open_in_editor,
            reveal,
            open_default,
            save_settings,
            cache_info,
            clear_cache,
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
