//! The library index: a single SQLite file in WAL mode.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use parking_lot::Mutex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::{Error, Kind, PhotoMeta};

const SCHEMA: &str = r#"
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
CREATE INDEX IF NOT EXISTS photos_source ON photos(source_id);
CREATE INDEX IF NOT EXISTS photos_pair ON photos(pair_key);
"#;

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
    pub grain: Option<String>,
    pub dynamic_range: Option<String>,
}

/// A file found on disk, ready to be indexed.
pub struct NewPhoto {
    pub path: PathBuf,
    pub kind: Kind,
    pub size: u64,
    pub mtime: i64,
    pub meta: PhotoMeta,
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
        conn.execute_batch(SCHEMA)?;
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
                    exposure_comp, film_sim, grain, dynamic_range)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
                 ON CONFLICT(path) DO UPDATE SET
                    source_id = excluded.source_id, size = excluded.size, mtime = excluded.mtime,
                    taken_at = excluded.taken_at, width = excluded.width, height = excluded.height,
                    orientation = excluded.orientation, camera = excluded.camera, lens = excluded.lens,
                    focal_length = excluded.focal_length, aperture = excluded.aperture,
                    shutter = excluded.shutter, iso = excluded.iso, exposure_comp = excluded.exposure_comp,
                    film_sim = excluded.film_sim, grain = excluded.grain, dynamic_range = excluded.dynamic_range",
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
                stmt.execute(params![
                    source_id,
                    p.path.to_string_lossy(),
                    folder,
                    name,
                    p.kind.as_str(),
                    pair_key(&p.path),
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
            "{ROW_SELECT}, p.path, p.orientation, p.exposure_comp, p.grain, p.dynamic_range FROM photos p WHERE p.id = ?1"
        ))?;
        stmt.query_row([id], |r| {
            Ok(PhotoDetail {
                row: row_from(r)?,
                path: r.get(ROW_COLUMNS)?,
                orientation: r.get(ROW_COLUMNS + 1)?,
                exposure_comp: r.get(ROW_COLUMNS + 2)?,
                grain: r.get(ROW_COLUMNS + 3)?,
                dynamic_range: r.get(ROW_COLUMNS + 4)?,
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
}

const ROW_COLUMNS: usize = 22;
const ROW_SELECT: &str =
    "SELECT p.id, p.source_id, p.name, p.folder, p.kind, p.size, p.mtime, p.taken_at,
    p.width, p.height, p.camera, p.lens, p.focal_length, p.aperture, p.shutter, p.iso, p.film_sim,
    p.rating, p.flag, p.label, p.added_at,
    (SELECT q.id FROM photos q WHERE q.pair_key = p.pair_key AND q.id <> p.id LIMIT 1)";

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
    })
}

/// Files shot as RAW+JPEG share a folder and stem (`DSCF1234.RAF`/`.JPG`).
fn pair_key(path: &Path) -> String {
    path.with_extension("").to_string_lossy().to_lowercase()
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
}
