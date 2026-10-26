//! People: faces grouped by who they are, and named.
//!
//! Each face found (see `faces`) is stored with its 128-number description.
//! Faces close to a known person's are filed under them; the rest are
//! grouped with each other into unnamed people, waiting for a name. Naming a
//! group, or saying "that's not them", is remembered: a face someone has
//! confirmed is never moved, and a face rejected from a person is never
//! filed under them again. Strangers can be hidden: their new photos keep
//! being filed under them, out of sight.

use std::collections::{HashMap, HashSet};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::Error;
use crate::db::Library;

/// Face similarity at which two faces are taken to be the same person.
/// Checked against the real model in `faces`' tests: the same person scored
/// about 0.75 there, different people about 0.1.
pub const SAME_PERSON: f32 = 0.40;

pub(crate) const TABLES: &str = "
CREATE TABLE IF NOT EXISTS people (
    id         INTEGER PRIMARY KEY,
    -- NULL until someone names this group of faces.
    name       TEXT COLLATE NOCASE,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Someone said they don't know this person.
    hidden     INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS faces (
    id         INTEGER PRIMARY KEY,
    photo_id   INTEGER NOT NULL REFERENCES photos(id) ON DELETE CASCADE,
    x REAL NOT NULL, y REAL NOT NULL, w REAL NOT NULL, h REAL NOT NULL,
    score      REAL NOT NULL,
    embedding  BLOB NOT NULL,
    person_id  INTEGER REFERENCES people(id) ON DELETE SET NULL,
    -- Someone said this face is this person.
    confirmed  INTEGER NOT NULL DEFAULT 0,
    -- Someone said this face is not this person.
    not_person INTEGER
);
CREATE INDEX IF NOT EXISTS faces_photo ON faces(photo_id);
CREATE INDEX IF NOT EXISTS faces_person ON faces(person_id);
-- Photos already searched for faces (with or without finding any).
CREATE TABLE IF NOT EXISTS face_scans (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    model    TEXT NOT NULL
);
";

/// Libraries made before people could be hidden.
pub(crate) fn migrate(conn: &rusqlite::Connection) -> Result<(), Error> {
    let hidden: i64 = conn.query_row(
        "SELECT count(*) FROM pragma_table_info('people') WHERE name = 'hidden'",
        [],
        |r| r.get(0),
    )?;
    if hidden == 0 {
        conn.execute_batch("ALTER TABLE people ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0")?;
    }
    Ok(())
}

/// A face to store: box as fractions of the picture, and its description.
pub struct NewFace {
    pub bbox: [f32; 4],
    pub score: f32,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonRow {
    pub id: i64,
    pub name: Option<String>,
    /// The clearest face, for the avatar.
    pub cover: i64,
    pub faces: usize,
    pub photo_ids: Vec<i64>,
    /// A stranger, kept out of the way.
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceRow {
    pub id: i64,
    pub person_id: Option<i64>,
    pub name: Option<String>,
    /// x, y, width, height as fractions of the picture.
    pub bbox: [f32; 4],
}

fn pack(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn unpack(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A running average of faces' descriptions.
#[derive(Clone)]
struct Centroid {
    sum: Vec<f32>,
}

impl Centroid {
    fn new(v: &[f32]) -> Self {
        Self { sum: v.to_vec() }
    }

    fn add(&mut self, v: &[f32]) {
        for (s, x) in self.sum.iter_mut().zip(v) {
            *s += x;
        }
    }

    fn similarity(&self, v: &[f32]) -> f32 {
        let norm = self
            .sum
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            .sqrt()
            .max(1e-12);
        dot(&self.sum, v) / norm
    }
}

/// A face filed under someone: id, photo, confirmed, detection score.
struct Filed {
    id: i64,
    photo: i64,
    confirmed: bool,
    score: f64,
}

struct Loose {
    id: i64,
    not_person: Option<i64>,
    embedding: Vec<f32>,
}

impl Library {
    /// Photos not yet searched for faces with `model`, newest first. A RAW
    /// stacked under its JPEG and movies are skipped.
    pub fn faces_todo(&self, model: &str) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT p.id FROM photos p
             WHERE NOT EXISTS (SELECT 1 FROM face_scans s WHERE s.photo_id = p.id AND s.model = ?1)
               AND p.kind <> 'video'
               AND NOT (p.kind IN ('raf', 'dng') AND EXISTS
                    (SELECT 1 FROM photos j WHERE j.pair_key = p.pair_key
                       AND j.kind IN ('jpeg', 'heif', 'png')))
             ORDER BY p.taken_at DESC",
        )?;
        let rows = stmt.query_map([model], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records the faces found in a photo (possibly none).
    pub fn store_faces(&self, photo_id: i64, model: &str, faces: &[NewFace]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM faces WHERE photo_id = ?1 AND confirmed = 0",
            [photo_id],
        )?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO faces (photo_id, x, y, w, h, score, embedding) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )?;
            for f in faces {
                let [x, y, w, h] = f.bbox;
                insert.execute(params![photo_id, x, y, w, h, f.score, pack(&f.embedding)])?;
            }
        }
        tx.execute(
            "INSERT OR REPLACE INTO face_scans (photo_id, model) VALUES (?1, ?2)",
            params![photo_id, model],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// How many photos have been searched for faces.
    pub fn faces_scanned(&self, model: &str) -> Result<usize, Error> {
        Ok(self.conn.lock().query_row(
            "SELECT count(*) FROM face_scans WHERE model = ?1",
            [model],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Files every face nobody has filed yet: under the person it looks
    /// like, or into a new unnamed group with the faces it looks like.
    /// Returns whether anything changed.
    pub fn group_faces(&self) -> Result<bool, Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut people: HashMap<i64, Centroid> = HashMap::new();
        let mut loose = Vec::new();
        {
            let mut stmt =
                tx.prepare("SELECT id, person_id, not_person, embedding FROM faces ORDER BY id")?;
            let mut rows = stmt.query([])?;
            while let Some(r) = rows.next()? {
                let embedding = unpack(&r.get::<_, Vec<u8>>(3)?);
                match r.get::<_, Option<i64>>(1)? {
                    Some(person) => people
                        .entry(person)
                        .and_modify(|c| c.add(&embedding))
                        .or_insert_with(|| Centroid::new(&embedding)),
                    None => {
                        loose.push(Loose {
                            id: r.get(0)?,
                            not_person: r.get(2)?,
                            embedding,
                        });
                        continue;
                    }
                };
            }
        }
        let mut filed: Vec<(i64, i64)> = Vec::new();
        // Faces of someone already known.
        let mut rest = Vec::new();
        for face in loose {
            let best = people
                .iter()
                .filter(|(id, _)| Some(**id) != face.not_person)
                .map(|(id, c)| (*id, c.similarity(&face.embedding)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((person, sim)) if sim >= SAME_PERSON => {
                    people.get_mut(&person).unwrap().add(&face.embedding);
                    filed.push((face.id, person));
                }
                _ => rest.push(face),
            }
        }
        // New groups among the rest; a face on its own waits for company.
        let mut groups: Vec<(Centroid, Vec<i64>, HashSet<i64>)> = Vec::new();
        for face in rest {
            let best = groups
                .iter()
                .enumerate()
                .filter(|(_, g)| face.not_person.is_none_or(|p| !g.2.contains(&p)))
                .map(|(i, g)| (i, g.0.similarity(&face.embedding)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, sim)) if sim >= SAME_PERSON => {
                    let g = &mut groups[i];
                    g.0.add(&face.embedding);
                    g.1.push(face.id);
                    g.2.extend(face.not_person);
                }
                _ => groups.push((
                    Centroid::new(&face.embedding),
                    vec![face.id],
                    face.not_person.into_iter().collect(),
                )),
            }
        }
        for (_, ids, _) in groups.into_iter().filter(|g| g.1.len() > 1) {
            tx.execute("INSERT INTO people (name) VALUES (NULL)", [])?;
            let person = tx.last_insert_rowid();
            filed.extend(ids.into_iter().map(|id| (id, person)));
        }
        {
            let mut set = tx.prepare_cached("UPDATE faces SET person_id = ?2 WHERE id = ?1")?;
            for (face, person) in &filed {
                set.execute(params![face, person])?;
            }
        }
        // Unnamed groups that lost all their faces.
        let emptied = tx.execute(
            "DELETE FROM people WHERE name IS NULL
               AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)",
            [],
        )?;
        tx.commit()?;
        Ok(!filed.is_empty() || emptied > 0)
    }

    /// Everyone, named people first (most photos first), then unnamed groups.
    pub fn people(&self) -> Result<Vec<PersonRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT f.person_id, p.name, p.hidden, f.id, f.photo_id, f.confirmed, f.score
             FROM faces f JOIN people p ON p.id = f.person_id",
        )?;
        type Entry = (Option<String>, bool, Vec<Filed>);
        let mut by_person: HashMap<i64, Entry> = HashMap::new();
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let entry = by_person.entry(r.get(0)?).or_insert_with(|| {
                (
                    r.get(1).ok().flatten(),
                    r.get::<_, i64>(2).is_ok_and(|h| h != 0),
                    Vec::new(),
                )
            });
            entry.2.push(Filed {
                id: r.get(3)?,
                photo: r.get(4)?,
                confirmed: r.get::<_, i64>(5)? != 0,
                score: r.get(6)?,
            });
        }
        // Named people with no faces left still exist.
        let mut named =
            conn.prepare_cached("SELECT id, name, hidden FROM people WHERE name IS NOT NULL")?;
        let mut rows = named.query([])?;
        while let Some(r) = rows.next()? {
            by_person.entry(r.get(0)?).or_insert_with(|| {
                (
                    r.get(1).ok(),
                    r.get::<_, i64>(2).is_ok_and(|h| h != 0),
                    Vec::new(),
                )
            });
        }
        let mut out: Vec<PersonRow> = by_person
            .into_iter()
            .map(|(id, (name, hidden, faces))| {
                // The clearest face someone confirmed, else the clearest.
                let cover = faces
                    .iter()
                    .max_by(|a, b| {
                        (a.confirmed, a.score)
                            .partial_cmp(&(b.confirmed, b.score))
                            .unwrap()
                    })
                    .map_or(0, |f| f.id);
                let mut photo_ids: Vec<i64> = faces.iter().map(|f| f.photo).collect();
                photo_ids.sort_unstable();
                photo_ids.dedup();
                PersonRow {
                    id,
                    name,
                    cover,
                    faces: faces.len(),
                    photo_ids,
                    hidden,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            (a.name.is_none(), std::cmp::Reverse(a.photo_ids.len()), a.id).cmp(&(
                b.name.is_none(),
                std::cmp::Reverse(b.photo_ids.len()),
                b.id,
            ))
        });
        Ok(out)
    }

    /// The faces in one photo, left to right; hidden strangers left out.
    pub fn faces_in(&self, photo_id: i64) -> Result<Vec<FaceRow>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT f.id, f.person_id, p.name, f.x, f.y, f.w, f.h
             FROM faces f LEFT JOIN people p ON p.id = f.person_id
             WHERE f.photo_id = ?1 AND NOT coalesce(p.hidden, 0) ORDER BY f.x",
        )?;
        let rows = stmt.query_map([photo_id], |r| {
            Ok(FaceRow {
                id: r.get(0)?,
                person_id: r.get(1)?,
                name: r.get(2)?,
                bbox: [r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?],
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Which photo a face is in, and where.
    pub fn face(&self, face_id: i64) -> Result<(i64, [f32; 4]), Error> {
        Ok(self.conn.lock().query_row(
            "SELECT photo_id, x, y, w, h FROM faces WHERE id = ?1",
            [face_id],
            |r| Ok((r.get(0)?, [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?])),
        )?)
    }

    /// Names a person (or unnamed group). If someone already has that
    /// name, the two become one. Returns who they are now.
    pub fn name_person(&self, id: i64, name: &str) -> Result<i64, Error> {
        let name = name.trim();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM people WHERE name = ?1 AND id <> ?2",
                params![name, id],
                |r| r.get(0),
            )
            .optional()?;
        let person = match existing {
            Some(other) => {
                tx.execute(
                    "UPDATE faces SET person_id = ?2 WHERE person_id = ?1",
                    params![id, other],
                )?;
                tx.execute("DELETE FROM people WHERE id = ?1", [id])?;
                other
            }
            None => {
                tx.execute(
                    "UPDATE people SET name = ?2 WHERE id = ?1",
                    params![id, name],
                )?;
                id
            }
        };
        // Someone looked at these faces and put a name to them.
        tx.execute(
            "UPDATE faces SET confirmed = 1 WHERE person_id = ?1",
            [person],
        )?;
        tx.execute("UPDATE people SET hidden = 0 WHERE id = ?1", [person])?;
        tx.commit()?;
        Ok(person)
    }

    /// Says who one face is: an existing person by name, or someone new.
    pub fn name_face(&self, face_id: i64, name: &str) -> Result<i64, Error> {
        let name = name.trim();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let person = match tx
            .query_row("SELECT id FROM people WHERE name = ?1", [name], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        {
            Some(id) => id,
            None => {
                tx.execute("INSERT INTO people (name) VALUES (?1)", [name])?;
                tx.last_insert_rowid()
            }
        };
        tx.execute(
            "UPDATE faces SET person_id = ?2, confirmed = 1, not_person = NULL WHERE id = ?1",
            params![face_id, person],
        )?;
        tx.execute("UPDATE people SET hidden = 0 WHERE id = ?1", [person])?;
        tx.execute(
            "DELETE FROM people WHERE name IS NULL
               AND NOT EXISTS (SELECT 1 FROM faces f WHERE f.person_id = people.id)",
            [],
        )?;
        tx.commit()?;
        Ok(person)
    }

    /// "Not this person": takes their faces out of these photos, for good.
    pub fn remove_from_person(&self, person_id: i64, photo_ids: &[i64]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE faces SET person_id = NULL, confirmed = 0, not_person = ?1
                 WHERE person_id = ?1 AND photo_id = ?2",
            )?;
            for photo in photo_ids {
                stmt.execute(params![person_id, photo])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The same person found twice: moves everyone in `from` into `into`.
    /// If `into` has no name, it takes the first name among `from`.
    /// Returns `into`.
    pub fn merge_people(&self, from: &[i64], into: i64) -> Result<i64, Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let name_of = |id: i64| -> Result<Option<String>, Error> {
            Ok(tx
                .query_row("SELECT name FROM people WHERE id = ?1", [id], |r| {
                    r.get::<_, Option<String>>(0)
                })
                .optional()?
                .flatten())
        };
        // One of them still in sight keeps the merged person in sight.
        let mut all_hidden = true;
        for &id in from.iter().chain([&into]) {
            all_hidden &= tx
                .query_row("SELECT hidden FROM people WHERE id = ?1", [id], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()?
                .is_none_or(|h| h != 0);
        }
        tx.execute(
            "UPDATE people SET hidden = ?2 WHERE id = ?1",
            params![into, all_hidden],
        )?;
        if name_of(into)?.is_none() {
            let mut names = Vec::new();
            for &id in from {
                names.extend(name_of(id)?);
            }
            if let Some(name) = names.first() {
                tx.execute(
                    "UPDATE people SET name = ?2 WHERE id = ?1",
                    params![into, name],
                )?;
            }
        }
        for &id in from.iter().filter(|&&id| id != into) {
            // Someone said these are the same person.
            tx.execute(
                "UPDATE faces SET person_id = ?2, confirmed = 1 WHERE person_id = ?1",
                params![id, into],
            )?;
            // "Not them" now means not the merged person.
            tx.execute(
                "UPDATE faces SET not_person = ?2 WHERE not_person = ?1",
                params![id, into],
            )?;
            tx.execute("DELETE FROM people WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(into)
    }

    /// Hides people (strangers in the background) or brings them back.
    /// Hidden people keep their faces, so new photos of them are filed
    /// under them and stay out of sight too.
    pub fn hide_people(&self, ids: &[i64], hidden: bool) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        for &id in ids {
            tx.execute(
                "UPDATE people SET hidden = ?2 WHERE id = ?1",
                params![id, hidden],
            )?;
            if hidden {
                // Kept even if the photo is searched for faces again.
                tx.execute("UPDATE faces SET confirmed = 1 WHERE person_id = ?1", [id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// "I don't know them", said of one face: hides the unnamed group it's
    /// in, or, for a face on its own (or filed under someone it isn't),
    /// files it under a new hidden person that look-alikes join later.
    /// Returns the hidden person.
    pub fn hide_face(&self, face_id: i64) -> Result<i64, Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let (current, named): (Option<i64>, bool) = tx.query_row(
            "SELECT f.person_id, p.name IS NOT NULL
             FROM faces f LEFT JOIN people p ON p.id = f.person_id WHERE f.id = ?1",
            [face_id],
            |r| Ok((r.get(0)?, r.get::<_, Option<bool>>(1)?.unwrap_or(false))),
        )?;
        let person = match current {
            Some(group) if !named => group,
            _ => {
                tx.execute("INSERT INTO people (name, hidden) VALUES (NULL, 1)", [])?;
                let person = tx.last_insert_rowid();
                tx.execute(
                    "UPDATE faces SET person_id = ?2, not_person = coalesce(person_id, not_person)
                     WHERE id = ?1",
                    params![face_id, person],
                )?;
                person
            }
        };
        tx.execute("UPDATE people SET hidden = 1 WHERE id = ?1", [person])?;
        tx.execute(
            "UPDATE faces SET confirmed = 1 WHERE person_id = ?1",
            [person],
        )?;
        tx.commit()?;
        Ok(person)
    }

    /// Forgets a person's name; their faces go back to being grouped.
    pub fn delete_person(&self, id: i64) -> Result<(), Error> {
        let conn = self.conn.lock();
        conn.execute("UPDATE faces SET confirmed = 0 WHERE person_id = ?1", [id])?;
        conn.execute("DELETE FROM people WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Kind;
    use crate::db::NewPhoto;
    use std::path::{Path, PathBuf};

    /// A face description near direction `axis` (8 dimensions are enough).
    fn look(axis: usize, wobble: f32) -> Vec<f32> {
        let mut v = [0.0; 8];
        v[axis] = 1.0;
        v[(axis + 1) % 8] = wobble;
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    fn face(axis: usize, wobble: f32) -> NewFace {
        NewFace {
            bbox: [0.1 * axis as f32, 0.1, 0.1, 0.1],
            score: 0.9,
            embedding: look(axis, wobble),
        }
    }

    fn library(photos: usize) -> Library {
        let lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new("/lib")).unwrap();
        let rows: Vec<NewPhoto> = (0..photos)
            .map(|i| NewPhoto {
                path: PathBuf::from(format!("/lib/IMG_{i}.HEIC")),
                kind: Kind::Heif,
                size: 1,
                mtime: 1,
                meta: Default::default(),
                place: None,
            })
            .collect();
        lib.upsert(src.id, &rows).unwrap();
        lib
    }

    fn ids(lib: &Library) -> Vec<i64> {
        let mut ids: Vec<i64> = lib.photos().unwrap().iter().map(|p| p.id).collect();
        ids.sort();
        ids
    }

    #[test]
    fn faces_group_get_named_and_new_photos_find_their_person() {
        let lib = library(5);
        let p = ids(&lib);
        // Ann (axis 0) in three photos, Bo (axis 3) in two, a stranger once.
        lib.store_faces(p[0], "m", &[face(0, 0.1), face(3, 0.0)])
            .unwrap();
        lib.store_faces(p[1], "m", &[face(0, 0.2)]).unwrap();
        lib.store_faces(p[2], "m", &[face(3, 0.15), face(6, 0.0)])
            .unwrap();
        assert_eq!(lib.faces_todo("m").unwrap().len(), 2);
        assert!(lib.group_faces().unwrap());

        let people = lib.people().unwrap();
        assert_eq!(people.len(), 2, "the stranger waits for company");
        assert!(people.iter().all(|p| p.name.is_none()));
        let ann = people
            .iter()
            .find(|g| g.photo_ids == [p[0], p[1]])
            .unwrap()
            .id;
        assert_eq!(lib.name_person(ann, "Ann").unwrap(), ann);

        // A new photo of Ann is filed under her.
        lib.store_faces(p[3], "m", &[face(0, 0.05)]).unwrap();
        lib.group_faces().unwrap();
        let ann_now = lib
            .people()
            .unwrap()
            .into_iter()
            .find(|g| g.id == ann)
            .unwrap();
        assert_eq!(ann_now.name.as_deref(), Some("Ann"));
        assert_eq!(ann_now.photo_ids, [p[0], p[1], p[3]]);

        // "That's not Ann" sticks, even though the face looks like her.
        lib.remove_from_person(ann, &[p[3]]).unwrap();
        lib.group_faces().unwrap();
        let ann_now = lib
            .people()
            .unwrap()
            .into_iter()
            .find(|g| g.id == ann)
            .unwrap();
        assert_eq!(ann_now.photo_ids, [p[0], p[1]]);

        // Naming a group with a name already used merges the two.
        let bo = lib
            .people()
            .unwrap()
            .into_iter()
            .find(|g| g.name.is_none())
            .unwrap()
            .id;
        assert_eq!(lib.name_person(bo, "ann").unwrap(), ann);
        assert_eq!(
            lib.people()
                .unwrap()
                .iter()
                .filter(|g| g.name.is_some())
                .count(),
            1
        );

        // One face named by hand.
        let stranger = lib
            .faces_in(p[2])
            .unwrap()
            .into_iter()
            .find(|f| f.person_id.is_none())
            .unwrap();
        let cy = lib.name_face(stranger.id, "Cy").unwrap();
        let faces = lib.faces_in(p[2]).unwrap();
        assert!(
            faces
                .iter()
                .any(|f| f.person_id == Some(cy) && f.name.as_deref() == Some("Cy"))
        );
    }

    #[test]
    fn one_person_found_twice_is_merged() {
        let lib = library(4);
        let p = ids(&lib);
        // The same person, looking different enough to make two groups.
        lib.store_faces(p[0], "m", &[face(0, 0.0)]).unwrap();
        lib.store_faces(p[1], "m", &[face(0, 0.1)]).unwrap();
        lib.store_faces(p[2], "m", &[face(4, 0.0)]).unwrap();
        lib.store_faces(p[3], "m", &[face(4, 0.1)]).unwrap();
        lib.group_faces().unwrap();
        let groups = lib.people().unwrap();
        assert_eq!(groups.len(), 2);
        let (a, b) = (groups[0].id, groups[1].id);
        lib.name_person(b, "Eli").unwrap();

        // Merging into the unnamed group keeps the name.
        assert_eq!(lib.merge_people(&[b], a).unwrap(), a);
        let people = lib.people().unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].name.as_deref(), Some("Eli"));
        assert_eq!(people[0].photo_ids, p);

        // Regrouping leaves the merged person alone.
        assert!(!lib.group_faces().unwrap());
        assert_eq!(lib.people().unwrap().len(), 1);
    }

    #[test]
    fn strangers_can_be_hidden_and_stay_hidden() {
        let lib = library(4);
        let p = ids(&lib);
        // Ann (axis 0) and a stranger in the background (axis 2).
        lib.store_faces(p[0], "m", &[face(0, 0.0), face(2, 0.0)])
            .unwrap();
        lib.store_faces(p[1], "m", &[face(2, 0.1)]).unwrap();
        lib.store_faces(p[2], "m", &[face(0, 0.1)]).unwrap();
        lib.group_faces().unwrap();
        let stranger = lib
            .people()
            .unwrap()
            .into_iter()
            .find(|g| g.photo_ids == [p[0], p[1]])
            .unwrap()
            .id;
        lib.hide_people(&[stranger], true).unwrap();
        let hidden: Vec<i64> = lib
            .people()
            .unwrap()
            .iter()
            .filter(|g| g.hidden)
            .map(|g| g.id)
            .collect();
        assert_eq!(hidden, [stranger]);
        assert_eq!(lib.faces_in(p[0]).unwrap().len(), 1, "only Ann shows");

        // A new photo of the stranger joins them, out of sight.
        lib.store_faces(p[3], "m", &[face(2, 0.05)]).unwrap();
        lib.group_faces().unwrap();
        let people = lib.people().unwrap();
        assert_eq!(people.len(), 2, "no new group pops up");
        let s = people.iter().find(|g| g.id == stranger).unwrap();
        assert!(s.hidden);
        assert_eq!(s.photo_ids, [p[0], p[1], p[3]]);

        // Merging Ann's group with them keeps the result in sight.
        let ann = people.iter().find(|g| !g.hidden).unwrap().id;
        lib.merge_people(&[ann], stranger).unwrap();
        assert!(!lib.people().unwrap()[0].hidden);

        // Unhiding, and naming, bring people back.
        lib.hide_people(&[stranger], true).unwrap();
        lib.hide_people(&[stranger], false).unwrap();
        assert!(!lib.people().unwrap()[0].hidden);
        lib.hide_people(&[stranger], true).unwrap();
        lib.name_person(stranger, "Sam").unwrap();
        assert!(!lib.people().unwrap()[0].hidden);
    }

    #[test]
    fn one_face_can_be_hidden_from_a_photo() {
        let lib = library(3);
        let p = ids(&lib);
        // Ann in two photos, plus someone passing by once.
        lib.store_faces(p[0], "m", &[face(0, 0.0), face(5, 0.0)])
            .unwrap();
        lib.store_faces(p[1], "m", &[face(0, 0.1)]).unwrap();
        lib.group_faces().unwrap();
        let passer = lib
            .faces_in(p[0])
            .unwrap()
            .into_iter()
            .find(|f| f.person_id.is_none())
            .unwrap();
        let hidden = lib.hide_face(passer.id).unwrap();
        assert_eq!(lib.faces_in(p[0]).unwrap().len(), 1);
        // They turn up again: still hidden.
        lib.store_faces(p[2], "m", &[face(5, 0.05)]).unwrap();
        lib.group_faces().unwrap();
        assert!(lib.faces_in(p[2]).unwrap().is_empty());
        let them = lib
            .people()
            .unwrap()
            .into_iter()
            .find(|g| g.id == hidden)
            .unwrap();
        assert!(them.hidden);
        assert_eq!(them.photo_ids, [p[0], p[2]]);

        // A face of an unnamed group hides the whole group.
        let ann = lib.faces_in(p[1]).unwrap()[0].clone();
        let group = ann.person_id.unwrap();
        assert_eq!(lib.hide_face(ann.id).unwrap(), group);
        assert!(lib.faces_in(p[0]).unwrap().is_empty());
    }

    #[test]
    fn libraries_from_before_hiding_are_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT COLLATE NOCASE,
                    created_at INTEGER NOT NULL DEFAULT (unixepoch()));
                 INSERT INTO people (name) VALUES ('Ann');",
            )
            .unwrap();
        let lib = Library::open(&path).unwrap();
        let people = lib.people().unwrap();
        assert_eq!(people[0].name.as_deref(), Some("Ann"));
        assert!(!people[0].hidden);
        drop(lib);
        Library::open(&path).unwrap();
    }

    #[test]
    fn deleting_photos_and_people_cleans_up() {
        let lib = library(2);
        let p = ids(&lib);
        lib.store_faces(p[0], "m", &[face(1, 0.0)]).unwrap();
        lib.store_faces(p[1], "m", &[face(1, 0.1)]).unwrap();
        lib.group_faces().unwrap();
        let group = lib.people().unwrap()[0].id;
        lib.name_person(group, "Dee").unwrap();
        lib.delete_person(group).unwrap();
        assert!(
            lib.people().unwrap().is_empty(),
            "faces wait to be grouped again"
        );
        lib.group_faces().unwrap();
        assert_eq!(lib.people().unwrap()[0].name, None);
        lib.remove_photos(&p).unwrap();
        lib.group_faces().unwrap();
        assert!(lib.people().unwrap().is_empty());
    }
}
