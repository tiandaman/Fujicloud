//! The text read in photos (see `text`), kept in the library and searched.

use rusqlite::{OptionalExtension, params};

use crate::Error;
use crate::db::Library;

pub(crate) const TABLES: &str = "
CREATE TABLE IF NOT EXISTS texts (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    model    TEXT NOT NULL,
    -- As read, a line each; empty for a photo with nothing written in it.
    text     TEXT NOT NULL,
    -- As matched (see `searchable`).
    norm     TEXT NOT NULL,
    -- How many pixels across the picture it was read from was (0 if
    -- another PC read it): small writing needs more than a thumbnail has.
    edge     INTEGER NOT NULL DEFAULT 0
);
";

/// A picture this many pixels across or more shows small writing: one read
/// from less is read again when a bigger copy of it is to hand.
pub const SHARP: u32 = 1000;

/// Columns added since the first version.
pub(crate) fn migrate(conn: &rusqlite::Connection) -> Result<(), Error> {
    let have: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('texts')")?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    if !have.iter().any(|c| c == "edge") {
        conn.execute_batch("ALTER TABLE texts ADD COLUMN edge INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

/// Text as it's matched: lower case, punctuation as single spaces. (So
/// \"Café—OPEN!\" is found by \"café open\".)
pub fn searchable(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_owned()
}

impl Library {
    /// Photos not read by `model` yet, newest first: not movies, and not a
    /// RAW behind its JPEG.
    pub fn text_todo(&self, model: &str) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT p.id FROM photos p
             WHERE p.kind <> 'video'
               AND NOT EXISTS (SELECT 1 FROM texts t WHERE t.photo_id = p.id AND t.model = ?1)
               AND NOT (p.kind IN ('raf', 'dng') AND EXISTS
                    (SELECT 1 FROM photos j WHERE j.pair_key = p.pair_key
                       AND j.kind IN ('jpeg', 'heif', 'png')))
             ORDER BY p.taken_at DESC",
        )?;
        let rows = stmt.query_map([model], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records what `model` read in a photo (its lines; none for nothing),
    /// from a picture `edge` pixels across.
    pub fn store_text(
        &self,
        id: i64,
        model: &str,
        lines: &[String],
        edge: u32,
    ) -> Result<(), Error> {
        let text = lines.join("\n");
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO texts (photo_id, model, text, norm, edge)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, model, text, searchable(&text), edge],
        )?;
        Ok(())
    }

    /// Photos `model` read from a small picture only (or that another PC
    /// read): worth reading again where a bigger copy is to hand.
    pub fn text_read_small(&self, model: &str) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt =
            conn.prepare_cached("SELECT photo_id FROM texts WHERE model = ?1 AND edge < ?2")?;
        let rows = stmt.query_map(params![model, SHARP], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Takes what another PC read, unless this one has read the photo.
    pub fn import_text(&self, id: i64, model: &str, text: &str) -> Result<bool, Error> {
        let n = self.conn.lock().execute(
            "INSERT OR IGNORE INTO texts (photo_id, model, text, norm) VALUES (?1, ?2, ?3, ?4)",
            params![id, model, text, searchable(text)],
        )?;
        Ok(n > 0)
    }

    /// What was read in a photo, and by which model.
    pub fn text_of(&self, id: i64) -> Result<Option<(String, String)>, Error> {
        Ok(self
            .conn
            .lock()
            .query_row(
                "SELECT model, text FROM texts WHERE photo_id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    /// How many photos `model` has read, and how many had something written.
    pub fn texts_read(&self, model: &str) -> Result<(usize, usize), Error> {
        Ok(self.conn.lock().query_row(
            "SELECT count(*), coalesce(sum(norm <> ''), 0) FROM texts WHERE model = ?1",
            [model],
            |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)? as usize)),
        )?)
    }

    /// Photos with every word of `query` written in them (as part of a
    /// word or more: \"harb\" finds \"harbour\").
    pub fn search_text(&self, query: &str) -> Result<Vec<i64>, Error> {
        let words: Vec<String> = searchable(query)
            .split(' ')
            .filter(|w| !w.is_empty())
            .map(|w| {
                format!(
                    "%{}%",
                    w.replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_")
                )
            })
            .collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT photo_id FROM texts WHERE {}",
            vec!["norm LIKE ? ESCAPE '\\'"; words.len()].join(" AND ")
        );
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(words.iter()), |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use crate::db::NewPhoto;
    use std::path::{Path, PathBuf};

    #[test]
    fn text_is_stored_and_found() {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let photo = |name: &str, kind| NewPhoto {
            path: PathBuf::from(format!("/lib/{name}")),
            kind,
            size: 1,
            mtime: 1,
            meta: Default::default(),
            place: None,
        };
        lib.upsert(
            src.id,
            &[
                photo("DSCF0001.JPG", Kind::Jpeg),
                photo("DSCF0001.RAF", Kind::Raf),
                photo("IMG_0002.HEIC", Kind::Heif),
                photo("IMG_0003.MOV", Kind::Video),
            ],
        )
        .unwrap();
        let id = |name: &str| {
            lib.photos()
                .unwrap()
                .iter()
                .find(|p| p.name == name)
                .unwrap()
                .id
        };
        let (jpg, heic) = (id("DSCF0001.JPG"), id("IMG_0002.HEIC"));
        let mut todo = lib.text_todo("m").unwrap();
        todo.sort();
        let mut both = vec![jpg, heic];
        both.sort();
        assert_eq!(todo, both, "not the movie, nor the RAW behind its JPEG");

        lib.store_text(
            jpg,
            "m",
            &["HALIFAX HARBOUR".into(), "Open 9am – 5pm".into()],
            512,
        )
        .unwrap();
        lib.store_text(heic, "m", &[], 1280).unwrap();
        assert_eq!(lib.text_read_small("m").unwrap(), vec![jpg]);
        assert!(lib.text_todo("m").unwrap().is_empty());
        assert_eq!(lib.texts_read("m").unwrap(), (2, 1));
        assert_eq!(lib.search_text("harbour").unwrap(), vec![jpg]);
        assert_eq!(lib.search_text("Harb, OPEN!").unwrap(), vec![jpg]);
        assert_eq!(lib.search_text("9am 5pm").unwrap(), vec![jpg]);
        assert!(lib.search_text("harbour closed").unwrap().is_empty());
        assert!(lib.search_text("100%").unwrap().is_empty());
        assert!(lib.search_text(" ").unwrap().is_empty());
        // Another PC's reading doesn't replace this one's.
        assert!(!lib.import_text(jpg, "m", "something else").unwrap());
        assert_eq!(
            lib.text_of(jpg).unwrap().unwrap().1,
            "HALIFAX HARBOUR\nOpen 9am – 5pm"
        );
        assert_eq!(searchable("  Café—OPEN!!  9am – 5pm "), "café open 9am 5pm");
    }
}
