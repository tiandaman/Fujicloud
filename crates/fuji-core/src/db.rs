//! The library index: a single SQLite file in WAL mode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::fuji::Recipe;
use crate::geo::Place;
use crate::xmp::Tags;
use crate::{Error, Kind, PhotoMeta};

const TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS sources (
    id    INTEGER PRIMARY KEY,
    path  TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS photos (
    id            INTEGER PRIMARY KEY,
    source_id     INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    path          TEXT NOT NULL UNIQUE,
    folder        TEXT NOT NULL,
    name          TEXT NOT NULL,
    kind          TEXT NOT NULL,
    pair_key      TEXT NOT NULL,
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    taken_at      INTEGER,
    width         INTEGER NOT NULL DEFAULT 0,
    height        INTEGER NOT NULL DEFAULT 0,
    orientation   INTEGER NOT NULL DEFAULT 1,
    camera        TEXT,
    lens          TEXT,
    focal_length  REAL,
    aperture      REAL,
    shutter       REAL,
    iso           INTEGER,
    exposure_comp REAL,
    film_sim      TEXT,
    grain         TEXT,
    dynamic_range TEXT,
    rating        INTEGER NOT NULL DEFAULT 0,
    flag          INTEGER NOT NULL DEFAULT 0,
    label         TEXT,
    added_at      INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE IF NOT EXISTS recipes (
    key  TEXT PRIMARY KEY,
    name TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS embeddings (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    model    TEXT NOT NULL,
    vec      BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS albums (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at INTEGER NOT NULL DEFAULT (unixepoch())
);
CREATE TABLE IF NOT EXISTS imported (
    path  TEXT PRIMARY KEY,
    size  INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    dest  TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS album_photos (
    album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    photo_id INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    added_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (album_id, photo_id)
);
"#;

/// Columns added after the first release. Libraries created before them get
/// the columns and a metadata re-read on the next scan.
const ADDED_COLUMNS: &[(&str, &str)] = &[
    ("recipe", "TEXT"),
    ("recipe_key", "TEXT"),
    ("lat", "REAL"),
    ("lon", "REAL"),
    ("altitude", "REAL"),
    ("gps_estimated", "INTEGER NOT NULL DEFAULT 0"),
    ("town", "TEXT"),
    ("region", "TEXT"),
    ("country", "TEXT"),
    ("edit_of", "INTEGER"),
];

/// Columns added later that hold user data or only matter for new kinds of
/// file, so existing rows don't need their metadata read again.
const ADDED_USER_COLUMNS: &[(&str, &str)] = &[
    ("favorite", "INTEGER NOT NULL DEFAULT 0"),
    // The sidecar's modified time when we last read or wrote it.
    ("xmp_mtime", "INTEGER"),
    // Changed here but not yet saved to the sidecar (e.g. the share was off).
    ("xmp_dirty", "INTEGER NOT NULL DEFAULT 0"),
    // Videos: length in seconds.
    ("duration", "REAL"),
];

const INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS photos_source ON photos(source_id);
CREATE INDEX IF NOT EXISTS photos_pair ON photos(pair_key);
CREATE INDEX IF NOT EXISTS photos_recipe ON photos(recipe_key);
"#;

/// Shots this close in time to a geotagged one inherit its location.
const LOCATION_WINDOW_SECS: i64 = 30 * 60;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRow {
    pub id: i64,
    pub path: String,
    pub label: String,
    pub count: i64,
}

/// The compact row the grid needs for every photo. Sorting, filtering and
/// grouping all happen client-side on this list, so it stays lean.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRow {
    pub id: i64,
    pub source_id: i64,
    pub name: String,
    pub folder: String,
    pub kind: Kind,
    pub size: i64,
    pub mtime: i64,
    pub taken_at: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub focal_length: Option<f64>,
    pub aperture: Option<f64>,
    pub shutter: Option<f64>,
    pub iso: Option<u32>,
    pub film_sim: Option<String>,
    pub rating: i32,
    pub flag: i32,
    pub label: Option<String>,
    pub added_at: i64,
    /// The other half of a RAW+JPEG pair shot together, if present.
    pub pair_id: Option<i64>,
    pub recipe_key: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    /// The position was borrowed from a geotagged shot taken around the same time.
    pub gps_estimated: bool,
    pub town: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
    /// For an edit (a Photoshop save or export), the camera original.
    pub edit_of: Option<i64>,
    pub favorite: bool,
    /// Videos: length in seconds.
    pub duration: Option<f64>,
}

/// Everything about one photo, for the info panel.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoDetail {
    #[serde(flatten)]
    pub row: PhotoRow,
    pub path: String,
    pub orientation: u16,
    pub exposure_comp: Option<f64>,
    pub altitude: Option<f64>,
    pub recipe: Option<Recipe>,
}

/// A recipe seen in the library, with how many photos used it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeRow {
    pub key: String,
    pub name: Option<String>,
    pub recipe: Recipe,
    pub count: i64,
}

/// An album and the photos in it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRow {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    pub ids: Vec<i64>,
}

/// A photo's sidecar bookkeeping, for syncing between laptops.
#[derive(Debug, Clone)]
pub struct SidecarRow {
    pub id: i64,
    pub path: PathBuf,
    pub kind: Kind,
    pub sidecar: PathBuf,
    pub xmp_mtime: Option<i64>,
    pub dirty: bool,
}

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
    pub source_id: i64,
}

/// A file found on disk, ready to be indexed.
pub struct NewPhoto {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
    pub meta: PhotoMeta,
    pub place: Option<Place>,
}

/// What a write touched, so the caller can mirror it into XMP sidecars.
#[derive(Debug, Clone)]
pub struct Touched {
    pub id: i64,
    pub path: PathBuf,
    pub kind: Kind,
    pub rating: i32,
    pub label: Option<String>,
}

pub struct Library {
    conn: Mutex<Connection>,
}

impl Library {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self, Error> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, Error> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 268435456;",
        )?;
        conn.execute_batch(TABLES)?;
        migrate(&conn)?;
        conn.execute_batch(INDEXES)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ---- sources ---------------------------------------------------------

    pub fn add_source(&self, path: &Path) -> Result<SourceRow, Error> {
        let path_str = path.to_string_lossy().into_owned();
        let label = default_label(path);
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO sources (path, label) VALUES (?1, ?2)",
            params![path_str, label],
        )?;
        let id: i64 =
            conn.query_row("SELECT id FROM sources WHERE path = ?1", [&path_str], |r| {
                r.get(0)
            })?;
        drop(conn);
        self.sources()?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or_else(|| Error::NotFound(id))
    }

    pub fn rename_source(&self, id: i64, label: &str) -> Result<(), Error> {
        self.conn.lock().execute(
            "UPDATE sources SET label = ?2 WHERE id = ?1",
            params![id, label],
        )?;
        Ok(())
    }

    pub fn remove_source(&self, id: i64) -> Result<(), Error> {
        self.conn
            .lock()
            .execute("DELETE FROM sources WHERE id = ?1", [id])?;
        Ok(())
    }

    pub fn sources(&self) -> Result<Vec<SourceRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT s.id, s.path, s.label, (SELECT count(*) FROM photos p WHERE p.source_id = s.id)
             FROM sources s ORDER BY s.label COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SourceRow {
                id: r.get(0)?,
                path: r.get(1)?,
                label: r.get(2)?,
                count: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The source whose folder contains `path`, preferring the deepest match.
    pub fn source_for(&self, path: &Path) -> Result<Option<SourceRow>, Error> {
        Ok(self
            .sources()?
            .into_iter()
            .filter(|s| path.starts_with(&s.path))
            .max_by_key(|s| s.path.len()))
    }

    // ---- indexing --------------------------------------------------------

    pub fn contains(&self, path: &Path) -> Result<bool, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT 1 FROM photos WHERE path = ?1")?;
        Ok(stmt.exists([path.to_string_lossy()])?)
    }

    /// `path → (id, size, mtime)` for everything indexed under a source.
    pub fn index_of(&self, source_id: i64) -> Result<HashMap<String, (i64, u64, i64)>, Error> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("SELECT path, id, size, mtime FROM photos WHERE source_id = ?1")?;
        let rows = stmt.query_map([source_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get(1)?, r.get::<_, i64>(2)? as u64, r.get(3)?),
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Inserts or refreshes files. User data (rating, flag, label) survives.
    pub fn upsert(&self, source_id: i64, photos: &[NewPhoto]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO photos (source_id, path, folder, name, kind, pair_key, size, mtime, taken_at,
                    width, height, orientation, camera, lens, focal_length, aperture, shutter, iso,
                    exposure_comp, film_sim, grain, dynamic_range, recipe, recipe_key,
                    lat, lon, altitude, gps_estimated, town, region, country)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                    ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, 0, ?28, ?29, ?30)
                 ON CONFLICT(path) DO UPDATE SET
                    source_id = excluded.source_id, size = excluded.size, mtime = excluded.mtime,
                    taken_at = excluded.taken_at, width = excluded.width, height = excluded.height,
                    orientation = excluded.orientation, camera = excluded.camera, lens = excluded.lens,
                    focal_length = excluded.focal_length, aperture = excluded.aperture,
                    shutter = excluded.shutter, iso = excluded.iso, exposure_comp = excluded.exposure_comp,
                    film_sim = excluded.film_sim, grain = excluded.grain, dynamic_range = excluded.dynamic_range,
                    recipe = excluded.recipe, recipe_key = excluded.recipe_key,
                    lat = excluded.lat, lon = excluded.lon, altitude = excluded.altitude,
                    gps_estimated = 0, town = excluded.town, region = excluded.region,
                    country = excluded.country",
            )?;
            for p in photos {
                let m = &p.meta;
                let folder = p
                    .path
                    .parent()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let name = p
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let recipe_json = m
                    .recipe
                    .as_ref()
                    .and_then(|r| serde_json::to_string(r).ok());
                let recipe_key = m.recipe.as_ref().and_then(Recipe::key);
                stmt.execute(params![
                    source_id,
                    p.path.to_string_lossy(),
                    folder,
                    name,
                    p.kind.as_str(),
                    pair_key(&p.path, p.kind),
                    p.size as i64,
                    p.mtime,
                    m.taken_at,
                    m.width,
                    m.height,
                    m.orientation,
                    m.camera,
                    m.lens,
                    m.focal_length,
                    m.aperture,
                    m.shutter,
                    m.iso,
                    m.exposure_comp,
                    m.film_sim,
                    m.grain,
                    m.dynamic_range,
                    recipe_json,
                    recipe_key,
                    m.lat,
                    m.lon,
                    m.altitude,
                    p.place.as_ref().and_then(|pl| pl.town.clone()),
                    p.place.as_ref().and_then(|pl| pl.region.clone()),
                    p.place.as_ref().map(|pl| pl.country.clone()),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_photos(&self, ids: &[i64]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM photos WHERE id = ?1")?;
            for id in ids {
                stmt.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    // ---- queries ---------------------------------------------------------

    pub fn photos(&self) -> Result<Vec<PhotoRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(&format!("{ROW_SELECT} FROM photos p"))?;
        let rows = stmt.query_map([], row_from)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn photo(&self, id: i64) -> Result<PhotoDetail, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "{ROW_SELECT}, p.path, p.orientation, p.exposure_comp, p.altitude, p.recipe FROM photos p WHERE p.id = ?1"
        ))?;
        stmt.query_row([id], |r| {
            Ok(PhotoDetail {
                row: row_from(r)?,
                path: r.get(ROW_COLUMNS)?,
                orientation: r.get(ROW_COLUMNS + 1)?,
                exposure_comp: r.get(ROW_COLUMNS + 2)?,
                altitude: r.get(ROW_COLUMNS + 3)?,
                recipe: r
                    .get::<_, Option<String>>(ROW_COLUMNS + 4)?
                    .and_then(|json| serde_json::from_str(&json).ok()),
            })
        })
        .optional()?
        .ok_or(Error::NotFound(id))
    }

    /// The minimum needed to serve image bytes: `(path, kind, size, mtime)`.
    pub fn file_of(&self, id: i64) -> Result<(PathBuf, Kind, u64, i64), Error> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("SELECT path, kind, size, mtime FROM photos WHERE id = ?1")?;
        stmt.query_row([id], |r| {
            Ok((
                PathBuf::from(r.get::<_, String>(0)?),
                Kind::parse(&r.get::<_, String>(1)?).unwrap_or(Kind::Jpeg),
                r.get::<_, i64>(2)? as u64,
                r.get(3)?,
            ))
        })
        .optional()?
        .ok_or(Error::NotFound(id))
    }

    // ---- culling ---------------------------------------------------------

    /// Rates photos. Both halves of a RAW+JPEG pair always move together.
    pub fn set_rating(&self, ids: &[i64], rating: i32) -> Result<Vec<Touched>, Error> {
        self.update_with_pairs(
            ids,
            "rating = ?2",
            rusqlite::types::Value::Integer(rating.clamp(0, 5) as i64),
        )
    }

    /// `1` pick, `-1` reject, `0` clear.
    pub fn set_flag(&self, ids: &[i64], flag: i32) -> Result<Vec<Touched>, Error> {
        self.update_with_pairs(
            ids,
            "flag = ?2",
            rusqlite::types::Value::Integer(flag.clamp(-1, 1) as i64),
        )
    }

    pub fn set_label(&self, ids: &[i64], label: Option<&str>) -> Result<Vec<Touched>, Error> {
        let value = match label {
            Some(l) => rusqlite::types::Value::Text(l.to_owned()),
            None => rusqlite::types::Value::Null,
        };
        self.update_with_pairs(ids, "label = ?2", value)
    }

    fn update_with_pairs(
        &self,
        ids: &[i64],
        set: &str,
        value: rusqlite::types::Value,
    ) -> Result<Vec<Touched>, Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut touched = Vec::new();
        {
            let mut update = tx.prepare(&format!(
                "UPDATE photos SET {set} WHERE pair_key = (SELECT pair_key FROM photos WHERE id = ?1)"
            ))?;
            let mut select = tx.prepare_cached(
                "SELECT id, path, kind, rating, label FROM photos
                 WHERE pair_key = (SELECT pair_key FROM photos WHERE id = ?1)",
            )?;
            for &id in ids {
                update.execute(params![id, value])?;
                let rows = select.query_map([id], |r| {
                    Ok(Touched {
                        id: r.get(0)?,
                        path: PathBuf::from(r.get::<_, String>(1)?),
                        kind: Kind::parse(&r.get::<_, String>(2)?).unwrap_or(Kind::Jpeg),
                        rating: r.get(3)?,
                        label: r.get(4)?,
                    })
                })?;
                for row in rows {
                    let row = row?;
                    if !touched.iter().any(|t: &Touched| t.id == row.id) {
                        touched.push(row);
                    }
                }
            }
        }
        tx.commit()?;
        Ok(touched)
    }

    pub fn set_favorite(&self, ids: &[i64], favorite: bool) -> Result<Vec<Touched>, Error> {
        self.update_with_pairs(
            ids,
            "favorite = ?2",
            rusqlite::types::Value::Integer(favorite as i64),
        )
    }

    /// `ids` plus the other halves of any RAW+JPEG (or Live Photo) pairs.
    pub fn with_pairs(&self, ids: &[i64]) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id FROM photos WHERE pair_key = (SELECT pair_key FROM photos WHERE id = ?1)",
        )?;
        let mut out = Vec::new();
        for &id in ids {
            for row in stmt.query_map([id], |r| r.get::<_, i64>(0))? {
                let row = row?;
                if !out.contains(&row) {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }

    // ---- albums ------------------------------------------------------------

    pub fn albums(&self) -> Result<Vec<AlbumRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT a.id, a.name, a.created_at,
                    (SELECT group_concat(ap.photo_id) FROM album_photos ap WHERE ap.album_id = a.id)
             FROM albums a ORDER BY a.name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            let ids: Option<String> = r.get(3)?;
            Ok(AlbumRow {
                id: r.get(0)?,
                name: r.get(1)?,
                created_at: r.get(2)?,
                ids: ids
                    .unwrap_or_default()
                    .split(',')
                    .filter_map(|v| v.parse().ok())
                    .collect(),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Creates an album, or returns the one that already has this name.
    pub fn create_album(&self, name: &str) -> Result<i64, Error> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR IGNORE INTO albums (name) VALUES (?1)",
            [name.trim()],
        )?;
        Ok(conn.query_row(
            "SELECT id FROM albums WHERE name = ?1",
            [name.trim()],
            |r| r.get(0),
        )?)
    }

    /// Returns the album's photos, whose sidecars need the new name.
    pub fn rename_album(&self, id: i64, name: &str) -> Result<Vec<i64>, Error> {
        self.conn.lock().execute(
            "UPDATE albums SET name = ?2 WHERE id = ?1",
            params![id, name.trim()],
        )?;
        self.album_members(id)
    }

    /// Returns the photos that were in it, whose sidecars need updating.
    pub fn delete_album(&self, id: i64) -> Result<Vec<i64>, Error> {
        let members = self.album_members(id)?;
        self.conn
            .lock()
            .execute("DELETE FROM albums WHERE id = ?1", [id])?;
        Ok(members)
    }

    /// Adds photos (with their pair halves); returns the ones affected.
    pub fn add_to_album(&self, album: i64, ids: &[i64]) -> Result<Vec<i64>, Error> {
        let ids = self.with_pairs(ids)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO album_photos (album_id, photo_id) VALUES (?1, ?2)",
            )?;
            for id in &ids {
                stmt.execute(params![album, id])?;
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    pub fn remove_from_album(&self, album: i64, ids: &[i64]) -> Result<Vec<i64>, Error> {
        let ids = self.with_pairs(ids)?;
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx
                .prepare_cached("DELETE FROM album_photos WHERE album_id = ?1 AND photo_id = ?2")?;
            for id in &ids {
                stmt.execute(params![album, id])?;
            }
        }
        tx.commit()?;
        Ok(ids)
    }

    fn album_members(&self, album: i64) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("SELECT photo_id FROM album_photos WHERE album_id = ?1")?;
        let rows = stmt.query_map([album], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // ---- imports ----------------------------------------------------------

    /// Whether this inbox file (as it is now) was already copied in.
    pub fn was_imported(&self, path: &Path, size: u64, mtime: i64) -> Result<bool, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT 1 FROM imported WHERE path = ?1 AND size = ?2 AND mtime = ?3",
        )?;
        Ok(stmt.exists(params![path.to_string_lossy(), size as i64, mtime])?)
    }

    pub fn mark_imported(
        &self,
        path: &Path,
        size: u64,
        mtime: i64,
        dest: &Path,
    ) -> Result<(), Error> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO imported (path, size, mtime, dest) VALUES (?1, ?2, ?3, ?4)",
            params![
                path.to_string_lossy(),
                size as i64,
                mtime,
                dest.to_string_lossy()
            ],
        )?;
        Ok(())
    }

    // ---- sidecar sync --------------------------------------------------------

    /// What a photo's sidecar should say.
    pub fn tags(&self, id: i64) -> Result<Tags, Error> {
        let conn = self.conn.lock();
        let mut tags = conn
            .query_row(
                "SELECT rating, label, flag, favorite FROM photos WHERE id = ?1",
                [id],
                |r| {
                    Ok(Tags {
                        rating: r.get(0)?,
                        label: r.get(1)?,
                        flag: r.get(2)?,
                        favorite: r.get::<_, i64>(3)? != 0,
                        albums: Vec::new(),
                    })
                },
            )
            .optional()?
            .ok_or(Error::NotFound(id))?;
        let mut stmt = conn.prepare_cached(
            "SELECT a.name FROM albums a JOIN album_photos ap ON ap.album_id = a.id
             WHERE ap.photo_id = ?1 ORDER BY a.name COLLATE NOCASE",
        )?;
        tags.albums = stmt
            .query_map([id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(tags)
    }

    /// Every photo in a source with where its sidecar lives, when we last
    /// saw it change, and whether a change made here is still unsaved.
    pub fn sidecars(&self, source_id: i64) -> Result<Vec<SidecarRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT id, path, kind, xmp_mtime, xmp_dirty FROM photos WHERE source_id = ?1",
        )?;
        let rows = stmt.query_map([source_id], |r| {
            let path = PathBuf::from(r.get::<_, String>(1)?);
            let kind = Kind::parse(&r.get::<_, String>(2)?).unwrap_or(Kind::Jpeg);
            Ok(SidecarRow {
                id: r.get(0)?,
                sidecar: crate::xmp::sidecar_path(&path, kind),
                path,
                kind,
                xmp_mtime: r.get(3)?,
                dirty: r.get::<_, i64>(4)? != 0,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Applies what other laptops saved in sidecars. Photos with unsaved
    /// changes made here keep them; those get written out instead.
    pub fn apply_sidecars(&self, items: &[(i64, Tags, i64)]) -> Result<usize, Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut applied = 0;
        {
            let mut update = tx.prepare_cached(
                "UPDATE photos SET rating = ?2, label = ?3, flag = ?4, favorite = ?5, xmp_mtime = ?6
                 WHERE id = ?1 AND xmp_dirty = 0",
            )?;
            let mut clear = tx.prepare_cached("DELETE FROM album_photos WHERE photo_id = ?1")?;
            let mut album = tx.prepare_cached("INSERT OR IGNORE INTO albums (name) VALUES (?1)")?;
            let mut join = tx.prepare_cached(
                "INSERT OR IGNORE INTO album_photos (album_id, photo_id)
                 SELECT id, ?2 FROM albums WHERE name = ?1",
            )?;
            for (id, tags, mtime) in items {
                let changed = update.execute(params![
                    id,
                    tags.rating.clamp(0, 5),
                    tags.label,
                    tags.flag.clamp(-1, 1),
                    tags.favorite as i64,
                    mtime
                ])?;
                if changed == 0 {
                    continue;
                }
                applied += 1;
                clear.execute([id])?;
                for name in &tags.albums {
                    album.execute([name.trim()])?;
                    join.execute(params![name.trim(), id])?;
                }
            }
        }
        tx.commit()?;
        Ok(applied)
    }

    /// Records a sidecar write: `Some(mtime)` saved, `None` still pending.
    pub fn sidecar_saved(&self, ids: &[i64], mtime: Option<i64>) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE photos SET xmp_dirty = ?2, xmp_mtime = coalesce(?3, xmp_mtime) WHERE id = ?1",
            )?;
            for id in ids {
                stmt.execute(params![id, mtime.is_none() as i64, mtime])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Photos changed here whose sidecars couldn't be saved yet.
    pub fn unsaved_sidecars(&self) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT id FROM photos WHERE xmp_dirty = 1")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Where a photo's file is and which source it belongs to.
    pub fn file_info(&self, id: i64) -> Result<FileInfo, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT path, kind, size, mtime, source_id FROM photos WHERE id = ?1",
        )?;
        stmt.query_row([id], |r| {
            Ok(FileInfo {
                path: PathBuf::from(r.get::<_, String>(0)?),
                kind: Kind::parse(&r.get::<_, String>(1)?).unwrap_or(Kind::Jpeg),
                size: r.get::<_, i64>(2)? as u64,
                mtime: r.get(3)?,
                source_id: r.get(4)?,
            })
        })
        .optional()?
        .ok_or(Error::NotFound(id))
    }

    // ---- recipes -----------------------------------------------------------

    /// Every recipe in the library, most used first. RAW+JPEG pairs count once.
    pub fn recipes(&self) -> Result<Vec<RecipeRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT p.recipe_key, count(DISTINCT p.pair_key), max(p.recipe), r.name
             FROM photos p LEFT JOIN recipes r ON r.key = p.recipe_key
             WHERE p.recipe_key IS NOT NULL AND p.flag <> -1
             GROUP BY p.recipe_key
             ORDER BY 2 DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (key, count, json, name) = row?;
            if let Ok(recipe) = serde_json::from_str(&json) {
                out.push(RecipeRow {
                    key,
                    name,
                    recipe,
                    count,
                });
            }
        }
        Ok(out)
    }

    /// Names a recipe ("My Kodachrome"); `None` goes back to the default name.
    pub fn name_recipe(&self, key: &str, name: Option<&str>) -> Result<(), Error> {
        let conn = self.conn.lock();
        match name.map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => conn.execute(
                "INSERT INTO recipes (key, name) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET name = excluded.name",
                params![key, name],
            )?,
            None => conn.execute("DELETE FROM recipes WHERE key = ?1", [key])?,
        };
        Ok(())
    }

    // ---- places ------------------------------------------------------------

    /// Gives photos without GPS the location of the nearest-in-time geotagged
    /// shot within half an hour (location sync can lag or drop out), and
    /// clears estimates that no longer have a source. Returns how many
    /// photos have an estimated location.
    pub fn infer_locations(&self) -> Result<usize, Error> {
        type Loc = (f64, f64, Option<String>, Option<String>, Option<String>);
        let mut conn = self.conn.lock();
        let rows: Vec<(i64, Option<i64>, Option<Loc>, bool)> = conn
            .prepare(
                "SELECT id, taken_at, lat, lon, town, region, country, gps_estimated FROM photos",
            )?
            .query_map([], |r| {
                let lat: Option<f64> = r.get(2)?;
                let lon: Option<f64> = r.get(3)?;
                let loc = lat.zip(lon).map(|(lat, lon)| {
                    (
                        lat,
                        lon,
                        r.get(4).ok().flatten(),
                        r.get(5).ok().flatten(),
                        r.get(6).ok().flatten(),
                    )
                });
                Ok((r.get(0)?, r.get(1)?, loc, r.get::<_, i64>(7)? != 0))
            })?
            .collect::<Result<_, _>>()?;

        let mut anchors: Vec<(i64, &Loc)> = rows
            .iter()
            .filter(|(_, _, loc, est)| !est && loc.is_some())
            .filter_map(|(_, t, loc, _)| Some((t.as_ref().copied()?, loc.as_ref()?)))
            .collect();
        anchors.sort_by_key(|(t, _)| *t);

        let nearest = |t: i64| -> Option<&Loc> {
            let i = anchors.partition_point(|(at, _)| *at < t);
            [i.checked_sub(1), Some(i)]
                .into_iter()
                .flatten()
                .filter_map(|j| anchors.get(j))
                .filter(|(at, _)| (at - t).abs() <= LOCATION_WINDOW_SECS)
                .min_by_key(|(at, _)| (at - t).abs())
                .map(|(_, loc)| *loc)
        };

        let tx = conn.transaction()?;
        let mut estimated = 0;
        {
            let mut set = tx.prepare(
                "UPDATE photos SET lat = ?2, lon = ?3, town = ?4, region = ?5, country = ?6, gps_estimated = 1
                 WHERE id = ?1",
            )?;
            let mut clear = tx.prepare(
                "UPDATE photos SET lat = NULL, lon = NULL, town = NULL, region = NULL, country = NULL,
                 gps_estimated = 0 WHERE id = ?1",
            )?;
            for (id, taken, loc, est) in &rows {
                if loc.is_some() && !est {
                    continue;
                }
                match taken.and_then(nearest) {
                    Some((lat, lon, town, region, country)) => {
                        set.execute(params![id, lat, lon, town, region, country])?;
                        estimated += 1;
                    }
                    None if *est => {
                        clear.execute([id])?;
                    }
                    None => {}
                }
            }
        }
        tx.commit()?;
        Ok(estimated)
    }

    // ---- edits ---------------------------------------------------------------

    /// Links edits to the camera file they were made from, so they can be
    /// compared before/after. An edit sits in the same folder as its original
    /// or one below it (e.g. `Edits/`), and its name starts with the
    /// original's: `DSCF1234.psd`, `DSCF1234.tif`, `DSCF1234-Edit.jpg`,
    /// `DSCF1234_v2.tif`. Edits inherit the original's capture date, camera
    /// settings and place, which Photoshop saves don't always carry. Returns
    /// how many edits are linked.
    pub fn link_edits(&self) -> Result<usize, Error> {
        let mut conn = self.conn.lock();
        let rows: Vec<(i64, String, String, String, Option<i64>)> = conn
            .prepare("SELECT id, folder, name, kind, edit_of FROM photos")?
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<Result<_, _>>()?;

        let stem = |name: &str| {
            Path::new(name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_lowercase())
                .unwrap_or_default()
        };
        let folder_key = |f: &str| f.to_lowercase();
        // Camera originals by (folder, stem); the JPEG is the better "before"
        // than the RAF because it carries the film simulation.
        let mut originals: HashMap<(String, String), (i64, bool)> = HashMap::new();
        for (id, folder, name, kind, _) in &rows {
            let is_jpeg = kind == "jpeg";
            if is_jpeg || kind == "raf" || kind == "heif" {
                originals
                    .entry((folder_key(folder), stem(name)))
                    .and_modify(|e| {
                        if is_jpeg && !e.1 {
                            *e = (*id, true);
                        }
                    })
                    .or_insert((*id, is_jpeg));
            }
        }

        let mut links = Vec::new();
        for (id, folder, name, kind, current) in &rows {
            let Some(k) = Kind::parse(kind) else { continue };
            if !(k.is_edit_format() || k == Kind::Jpeg) {
                continue;
            }
            let full = stem(name);
            // Candidate base names, longest first: "dscf1-edit-2" → "dscf1-edit" → "dscf1".
            let mut bases = Vec::new();
            if k.is_edit_format() {
                bases.push(full.clone());
            }
            let mut rest = full.as_str();
            while let Some(i) = rest.rfind(['-', '_', ' ']) {
                rest = &rest[..i];
                if !rest.is_empty() {
                    bases.push(rest.to_owned());
                }
            }
            let here = folder_key(folder);
            let above = Path::new(folder)
                .parent()
                .map(|p| folder_key(&p.to_string_lossy()));
            let original = bases.iter().find_map(|base| {
                originals
                    .get(&(here.clone(), base.clone()))
                    .or_else(|| {
                        above
                            .as_ref()
                            .and_then(|a| originals.get(&(a.clone(), base.clone())))
                    })
                    .map(|o| o.0)
                    .filter(|o| o != id)
            });
            if original != *current {
                links.push((*id, original));
            }
        }

        let tx = conn.transaction()?;
        {
            let mut set = tx.prepare("UPDATE photos SET edit_of = ?2 WHERE id = ?1")?;
            for (id, original) in &links {
                set.execute(params![id, original])?;
            }
            // Photoshop files carry no metadata we read; JPEG exports may have
            // had it stripped. Either way, borrow the original's.
            tx.execute_batch(
                "UPDATE photos AS e SET
                    taken_at = o.taken_at, camera = o.camera, lens = o.lens,
                    focal_length = o.focal_length, aperture = o.aperture, shutter = o.shutter,
                    iso = o.iso, lat = o.lat, lon = o.lon, gps_estimated = o.gps_estimated,
                    town = o.town, region = o.region, country = o.country
                 FROM photos AS o
                 WHERE e.edit_of = o.id AND (e.kind IN ('tiff', 'psd') OR e.camera IS NULL)",
            )?;
        }
        tx.commit()?;
        Ok(conn.query_row(
            "SELECT count(*) FROM photos WHERE edit_of IS NOT NULL",
            [],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    // ---- smart search embeddings --------------------------------------------

    /// Photos still needing an embedding for `model`, newest first. A RAF
    /// stacked under its JPEG twin is skipped: the JPEG stands for both.
    pub fn missing_embeddings(&self, model: &str) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT p.id FROM photos p
             WHERE p.kind IN ('jpeg', 'raf', 'heif', 'tiff', 'psd')
               AND NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.photo_id = p.id AND e.model = ?1)
               AND NOT (p.kind = 'raf' AND EXISTS
                    (SELECT 1 FROM photos j WHERE j.pair_key = p.pair_key AND j.kind = 'jpeg'))
             ORDER BY p.taken_at DESC",
        )?;
        let rows = stmt.query_map([model], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores embeddings packed by [`crate::search::quantize`].
    pub fn store_embeddings(&self, model: &str, items: &[(i64, Vec<u8>)]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO embeddings (photo_id, model, vec) VALUES (?1, ?2, ?3)",
            )?;
            for (id, blob) in items {
                stmt.execute(params![id, model, blob])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// `(photo id, packed embedding)` for every indexed photo.
    pub fn embeddings(&self, model: &str) -> Result<Vec<(i64, Vec<u8>)>, Error> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("SELECT photo_id, vec FROM embeddings WHERE model = ?1")?;
        let rows = stmt.query_map([model], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn embedding_count(&self, model: &str) -> Result<i64, Error> {
        let conn = self.conn.lock();
        Ok(conn.query_row(
            "SELECT count(*) FROM embeddings WHERE model = ?1",
            [model],
            |r| r.get(0),
        )?)
    }
}

const ROW_COLUMNS: usize = 32;
const ROW_SELECT: &str =
    "SELECT p.id, p.source_id, p.name, p.folder, p.kind, p.size, p.mtime, p.taken_at,
    p.width, p.height, p.camera, p.lens, p.focal_length, p.aperture, p.shutter, p.iso, p.film_sim,
    p.rating, p.flag, p.label, p.added_at,
    (SELECT q.id FROM photos q WHERE q.pair_key = p.pair_key AND q.id <> p.id LIMIT 1),
    p.recipe_key, p.lat, p.lon, p.gps_estimated, p.town, p.region, p.country, p.edit_of,
    p.favorite, p.duration";

fn row_from(r: &rusqlite::Row) -> rusqlite::Result<PhotoRow> {
    Ok(PhotoRow {
        id: r.get(0)?,
        source_id: r.get(1)?,
        name: r.get(2)?,
        folder: r.get(3)?,
        kind: Kind::parse(&r.get::<_, String>(4)?).unwrap_or(Kind::Jpeg),
        size: r.get(5)?,
        mtime: r.get(6)?,
        taken_at: r.get(7)?,
        width: r.get(8)?,
        height: r.get(9)?,
        camera: r.get(10)?,
        lens: r.get(11)?,
        focal_length: r.get(12)?,
        aperture: r.get(13)?,
        shutter: r.get(14)?,
        iso: r.get(15)?,
        film_sim: r.get(16)?,
        rating: r.get(17)?,
        flag: r.get(18)?,
        label: r.get(19)?,
        added_at: r.get(20)?,
        pair_id: r.get(21)?,
        recipe_key: r.get(22)?,
        lat: r.get(23)?,
        lon: r.get(24)?,
        gps_estimated: r.get::<_, i64>(25)? != 0,
        town: r.get(26)?,
        region: r.get(27)?,
        country: r.get(28)?,
        edit_of: r.get(29)?,
        favorite: r.get::<_, i64>(30)? != 0,
        duration: r.get(31)?,
    })
}

fn migrate(conn: &Connection) -> Result<(), Error> {
    let existing: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('photos')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let mut added = false;
    for (name, decl) in ADDED_COLUMNS {
        if !existing.iter().any(|c| c == name) {
            conn.execute_batch(&format!("ALTER TABLE photos ADD COLUMN {name} {decl}"))?;
            added = true;
        }
    }
    if added {
        // Make the next scan re-read every file's metadata to fill them in.
        conn.execute_batch("UPDATE photos SET mtime = -1")?;
    }
    for (name, decl) in ADDED_USER_COLUMNS {
        if !existing.iter().any(|c| c == name) {
            conn.execute_batch(&format!("ALTER TABLE photos ADD COLUMN {name} {decl}"))?;
        }
    }
    Ok(())
}

/// Files the camera shot together share a folder and stem (`DSCF1234.RAF`
/// and `.JPG` or `.HIF`). Edits such as `DSCF1234.psd` share the stem too but
/// are separate photos (linked by `edit_of`), so they get a key of their own.
fn pair_key(path: &Path, kind: Kind) -> String {
    if kind.is_edit_format() {
        path.to_string_lossy().to_lowercase()
    } else {
        path.with_extension("").to_string_lossy().to_lowercase()
    }
}

fn default_label(path: &Path) -> String {
    // `\\SHIELD\Samsung_T7` → "Samsung_T7"; `D:\` → "D:\".
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(path: &str, kind: Kind) -> NewPhoto {
        NewPhoto {
            path: PathBuf::from(path),
            kind,
            size: 10,
            mtime: 1,
            meta: PhotoMeta {
                width: 600,
                height: 400,
                film_sim: Some("Velvia".into()),
                ..Default::default()
            },
            place: None,
        }
    }

    #[test]
    fn pairs_and_ratings_move_together() {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        lib.upsert(
            src.id,
            &[
                photo("/lib/a/DSCF0001.JPG", Kind::Jpeg),
                photo("/lib/a/DSCF0001.RAF", Kind::Raf),
                photo("/lib/a/DSCF0002.JPG", Kind::Jpeg),
            ],
        )
        .unwrap();

        let rows = lib.photos().unwrap();
        assert_eq!(rows.len(), 3);
        let jpg = rows.iter().find(|r| r.name == "DSCF0001.JPG").unwrap();
        let raf = rows.iter().find(|r| r.name == "DSCF0001.RAF").unwrap();
        assert_eq!(jpg.pair_id, Some(raf.id));
        assert_eq!(raf.pair_id, Some(jpg.id));

        let touched = lib.set_rating(&[jpg.id], 4).unwrap();
        assert_eq!(touched.len(), 2);
        assert!(
            lib.photos()
                .unwrap()
                .iter()
                .filter(|r| r.name.starts_with("DSCF0001"))
                .all(|r| r.rating == 4)
        );

        // Re-indexing keeps the rating.
        lib.upsert(src.id, &[photo("/lib/a/DSCF0001.JPG", Kind::Jpeg)])
            .unwrap();
        assert_eq!(lib.photo(jpg.id).unwrap().row.rating, 4);

        assert_eq!(lib.sources().unwrap()[0].count, 3);
        lib.remove_source(src.id).unwrap();
        assert!(lib.photos().unwrap().is_empty());
    }

    fn shot(path: &str, taken: i64, gps: Option<(f64, f64)>, recipe: Option<Recipe>) -> NewPhoto {
        let mut p = photo(path, Kind::Jpeg);
        p.meta.taken_at = Some(taken);
        p.meta.lat = gps.map(|g| g.0);
        p.meta.lon = gps.map(|g| g.1);
        p.place = gps.and_then(|(lat, lon)| crate::geo::reverse(lat, lon));
        p.meta.recipe = recipe;
        p
    }

    #[test]
    fn recipes_are_grouped_and_nameable() {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let chrome = Recipe {
            film_sim: Some("Classic Chrome".into()),
            clarity: Some(2),
            ..Default::default()
        };
        let acros = Recipe {
            film_sim: Some("Acros".into()),
            ..Default::default()
        };
        lib.upsert(
            src.id,
            &[
                shot("/lib/1.JPG", 0, None, Some(chrome.clone())),
                shot("/lib/2.JPG", 0, None, Some(chrome.clone())),
                shot("/lib/3.JPG", 0, None, Some(acros)),
                shot("/lib/4.JPG", 0, None, None),
            ],
        )
        .unwrap();
        let recipes = lib.recipes().unwrap();
        assert_eq!(recipes.len(), 2);
        assert_eq!(
            (recipes[0].count, recipes[0].recipe.clone()),
            (2, chrome.clone())
        );

        let key = chrome.key().unwrap();
        lib.name_recipe(&key, Some("  Kodachrome-ish ")).unwrap();
        assert_eq!(
            lib.recipes().unwrap()[0].name.as_deref(),
            Some("Kodachrome-ish")
        );
        lib.name_recipe(&key, None).unwrap();
        assert_eq!(lib.recipes().unwrap()[0].name, None);

        let id = lib
            .photos()
            .unwrap()
            .into_iter()
            .find(|p| p.name == "1.JPG")
            .unwrap()
            .id;
        assert_eq!(lib.photo(id).unwrap().recipe, Some(chrome));
    }

    #[test]
    fn locations_are_borrowed_from_nearby_shots() {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let kyoto = Some((35.0116, 135.7681));
        lib.upsert(
            src.id,
            &[
                shot("/lib/a.JPG", 10_000, kyoto, None),
                shot("/lib/b.JPG", 10_600, None, None), // 10 min later
                shot("/lib/c.JPG", 20_000, None, None), // hours later
            ],
        )
        .unwrap();
        assert_eq!(lib.infer_locations().unwrap(), 1);
        let rows = lib.photos().unwrap();
        let get = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
        assert!(!get("a.JPG").gps_estimated);
        assert_eq!(get("a.JPG").town.as_deref(), Some("Kyoto"));
        assert!(get("b.JPG").gps_estimated);
        assert_eq!(get("b.JPG").country.as_deref(), Some("JP"));
        assert_eq!(get("c.JPG").lat, None);

        // The source photo disappears: the estimate goes with it.
        lib.remove_photos(&[get("a.JPG").id]).unwrap();
        assert_eq!(lib.infer_locations().unwrap(), 0);
        assert_eq!(
            lib.photos()
                .unwrap()
                .iter()
                .filter(|r| r.lat.is_some())
                .count(),
            0
        );
    }

    #[test]
    fn embeddings_round_trip_and_skip_stacked_raws() {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        lib.upsert(
            src.id,
            &[
                photo("/lib/DSCF1.JPG", Kind::Jpeg),
                photo("/lib/DSCF1.RAF", Kind::Raf),
                photo("/lib/DSCF2.RAF", Kind::Raf),
            ],
        )
        .unwrap();
        let missing = lib.missing_embeddings("m").unwrap();
        assert_eq!(missing.len(), 2, "the RAF twin of DSCF1.JPG is skipped");
        lib.store_embeddings("m", &[(missing[0], vec![1, 2, 3, 4, 5])])
            .unwrap();
        assert_eq!(
            lib.embeddings("m").unwrap(),
            vec![(missing[0], vec![1, 2, 3, 4, 5])]
        );
        assert_eq!(lib.embedding_count("m").unwrap(), 1);
        assert_eq!(lib.missing_embeddings("m").unwrap().len(), 1);
        assert_eq!(lib.missing_embeddings("other-model").unwrap().len(), 2);
    }

    #[test]
    fn edits_are_linked_to_their_originals() {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let mut original = photo("/lib/day/DSCF0001.JPG", Kind::Jpeg);
        original.meta.taken_at = Some(1_000);
        original.meta.camera = Some("X-T5".into());
        let files = [
            original,
            photo("/lib/day/DSCF0001.RAF", Kind::Raf),
            photo("/lib/day/DSCF0001.psd", Kind::Psd),
            photo("/lib/day/Edits/DSCF0001-Edit.jpg", Kind::Jpeg),
            photo("/lib/day/DSCF0002.RAF", Kind::Raf),
            photo("/lib/day/DSCF0002_v2-final.tif", Kind::Tiff),
            photo("/lib/day/DSCF0003.JPG", Kind::Jpeg),
            photo("/lib/day/holiday-best.jpg", Kind::Jpeg),
        ];
        lib.upsert(src.id, &files).unwrap();
        assert_eq!(lib.link_edits().unwrap(), 3);

        let rows = lib.photos().unwrap();
        let get = |n: &str| rows.iter().find(|r| r.name == n).unwrap().clone();
        let jpg = get("DSCF0001.JPG");
        // The JPEG (with its film simulation) is the "before", not the RAF.
        assert_eq!(get("DSCF0001.psd").edit_of, Some(jpg.id));
        assert_eq!(get("DSCF0001-Edit.jpg").edit_of, Some(jpg.id));
        assert_eq!(
            get("DSCF0002_v2-final.tif").edit_of,
            Some(get("DSCF0002.RAF").id)
        );
        for n in [
            "DSCF0001.JPG",
            "DSCF0001.RAF",
            "DSCF0002.RAF",
            "DSCF0003.JPG",
            "holiday-best.jpg",
        ] {
            assert_eq!(get(n).edit_of, None, "{n}");
        }
        // A Photoshop save doesn't steal the RAW+JPEG pairing.
        assert_eq!(jpg.pair_id, Some(get("DSCF0001.RAF").id));
        assert_eq!(get("DSCF0001.psd").pair_id, None);
        // Edits borrow the original's capture details.
        assert_eq!(get("DSCF0001.psd").taken_at, Some(1_000));
        assert_eq!(get("DSCF0001.psd").camera.as_deref(), Some("X-T5"));

        // The original goes away: the link goes too.
        lib.remove_photos(&[jpg.id, get("DSCF0001.RAF").id])
            .unwrap();
        lib.link_edits().unwrap();
        assert_eq!(
            lib.photos()
                .unwrap()
                .iter()
                .filter(|r| r.edit_of.is_some())
                .count(),
            1
        );
    }

    #[test]
    fn upgrades_a_first_release_library() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        {
            // The schema as shipped in the first release.
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE sources (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, label TEXT NOT NULL);
                 CREATE TABLE photos (id INTEGER PRIMARY KEY, source_id INTEGER NOT NULL, path TEXT NOT NULL UNIQUE,
                    folder TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL, pair_key TEXT NOT NULL,
                    size INTEGER NOT NULL, mtime INTEGER NOT NULL, taken_at INTEGER,
                    width INTEGER NOT NULL DEFAULT 0, height INTEGER NOT NULL DEFAULT 0,
                    orientation INTEGER NOT NULL DEFAULT 1, camera TEXT, lens TEXT, focal_length REAL,
                    aperture REAL, shutter REAL, iso INTEGER, exposure_comp REAL, film_sim TEXT, grain TEXT,
                    dynamic_range TEXT, rating INTEGER NOT NULL DEFAULT 0, flag INTEGER NOT NULL DEFAULT 0,
                    label TEXT, added_at INTEGER NOT NULL DEFAULT (unixepoch()));
                 INSERT INTO sources VALUES (1, '/lib', 'lib');
                 INSERT INTO photos (source_id, path, folder, name, kind, pair_key, size, mtime, rating)
                    VALUES (1, '/lib/a.JPG', '/lib', 'a.JPG', 'jpeg', '/lib/a', 10, 123, 5);",
            )
            .unwrap();
        }
        let lib = Library::open(&path).unwrap();
        let rows = lib.photos().unwrap();
        assert_eq!(rows[0].rating, 5, "user data survives");
        assert_eq!(rows[0].mtime, -1, "forces a metadata re-read");
        assert_eq!(lib.index_of(1).unwrap()["/lib/a.JPG"].2, -1);
        // Opening again is a no-op.
        drop(lib);
        assert_eq!(Library::open(&path).unwrap().photos().unwrap()[0].mtime, -1);
    }
}
