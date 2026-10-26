//! People and pets: faces grouped by who they are, and named.
//!
//! Each face found (see `faces`) is stored with its 128-number description;
//! each pet found (see `animals`) with what the smart search model makes of
//! it. People and pets are kept apart but handled the same way.
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

/// Similarity (of smart search vectors of pets' pictures, with what all pets
/// share taken away: see `animals::distinctive`) at which two pictures are
/// taken to be the same pet. Checked against the real models in `animals`'
/// tests: cats of one breed scored about 0.29 there, cats of different breeds
/// about 0, and one pet in two photos is closer still than two of a breed.
/// Leans towards keeping pets apart: a pet split in two is merged in a
/// click, two pets as one take sorting out photo by photo.
pub const SAME_PET: f32 = 0.25;

/// Who a group of faces belongs to: a person, or a pet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Subject {
    Person,
    Pet,
}

impl Subject {
    pub fn as_str(self) -> &'static str {
        match self {
            Subject::Person => "person",
            Subject::Pet => "pet",
        }
    }

    fn parse(s: &str) -> Self {
        if s == "pet" {
            Subject::Pet
        } else {
            Subject::Person
        }
    }

    fn same(self) -> f32 {
        match self {
            Subject::Person => SAME_PERSON,
            Subject::Pet => SAME_PET,
        }
    }

    /// A stored description in the form faces (or pets) are compared in.
    fn comparable(self, v: Vec<f32>) -> Vec<f32> {
        match self {
            #[cfg(feature = "smart-search")]
            Subject::Pet => crate::animals::distinctive(&v),
            _ => v,
        }
    }

    /// Which table records the photos searched.
    fn scans(self) -> &'static str {
        match self {
            Subject::Person => "face_scans",
            Subject::Pet => "pet_scans",
        }
    }
}

pub(crate) const TABLES: &str = "
CREATE TABLE IF NOT EXISTS people (
    id         INTEGER PRIMARY KEY,
    -- NULL until someone names this group of faces.
    name       TEXT COLLATE NOCASE,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Someone said they don't know this person.
    hidden     INTEGER NOT NULL DEFAULT 0,
    kind       TEXT NOT NULL DEFAULT 'person'
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
    not_person INTEGER,
    kind       TEXT NOT NULL DEFAULT 'person'
);
CREATE INDEX IF NOT EXISTS faces_photo ON faces(photo_id);
CREATE INDEX IF NOT EXISTS faces_person ON faces(person_id);
-- Photos already searched for faces (with or without finding any).
CREATE TABLE IF NOT EXISTS face_scans (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    model    TEXT NOT NULL
);
-- And for pets.
CREATE TABLE IF NOT EXISTS pet_scans (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    model    TEXT NOT NULL
);
";

/// Libraries made before people could be hidden, or pets found.
pub(crate) fn migrate(conn: &rusqlite::Connection) -> Result<(), Error> {
    for (table, column, decl) in [
        ("people", "hidden", "INTEGER NOT NULL DEFAULT 0"),
        ("people", "kind", "TEXT NOT NULL DEFAULT 'person'"),
        ("faces", "kind", "TEXT NOT NULL DEFAULT 'person'"),
    ] {
        let has: i64 = conn.query_row(
            &format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name = ?1"),
            [column],
            |r| r.get(0),
        )?;
        if has == 0 {
            conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"))?;
        }
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
    pub kind: Subject,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceRow {
    pub id: i64,
    pub person_id: Option<i64>,
    pub name: Option<String>,
    /// x, y, width, height as fractions of the picture.
    pub bbox: [f32; 4],
    pub kind: Subject,
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

/// How much two boxes overlap (intersection over union).
fn overlap(a: &[f32; 4], b: &[f32; 4]) -> f32 {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    inter / (a[2] * a[3] + b[2] * b[3] - inter).max(1e-12)
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

/// A face (or pet) not filed under anyone yet.
pub(crate) struct Loose {
    pub id: i64,
    /// Someone said it isn't this person.
    pub not_person: Option<i64>,
    pub embedding: Vec<f32>,
}

/// Groups faces that look alike (similarity to the group's average at least
/// `same`), in order. Only groups of two or more are returned: a face on its
/// own waits for company.
pub(crate) fn cluster(faces: Vec<Loose>, same: f32) -> Vec<Vec<i64>> {
    let mut groups: Vec<(Centroid, Vec<i64>, HashSet<i64>)> = Vec::new();
    for face in faces {
        let best = groups
            .iter()
            .enumerate()
            .filter(|(_, g)| face.not_person.is_none_or(|p| !g.2.contains(&p)))
            .map(|(i, g)| (i, g.0.similarity(&face.embedding)))
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((i, sim)) if sim >= same => {
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
    groups
        .into_iter()
        .map(|g| g.1)
        .filter(|ids| ids.len() > 1)
        .collect()
}

impl Library {
    /// Photos not yet searched for faces with `model`, newest first. A RAW
    /// stacked under its JPEG and movies are skipped.
    pub fn faces_todo(&self, model: &str) -> Result<Vec<i64>, Error> {
        self.todo(Subject::Person, model)
    }

    /// Photos not yet searched for pets with `model`, newest first.
    pub fn pets_todo(&self, model: &str) -> Result<Vec<i64>, Error> {
        self.todo(Subject::Pet, model)
    }

    fn todo(&self, subject: Subject, model: &str) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT p.id FROM photos p
             WHERE NOT EXISTS (SELECT 1 FROM {} s WHERE s.photo_id = p.id AND s.model = ?1)
               AND p.kind <> 'video'
               AND NOT (p.kind IN ('raf', 'dng') AND EXISTS
                    (SELECT 1 FROM photos j WHERE j.pair_key = p.pair_key
                       AND j.kind IN ('jpeg', 'heif', 'png')))
             ORDER BY p.taken_at DESC",
            subject.scans()
        ))?;
        let rows = stmt.query_map([model], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records the faces found in a photo (possibly none).
    pub fn store_faces(&self, photo_id: i64, model: &str, faces: &[NewFace]) -> Result<(), Error> {
        self.store(Subject::Person, photo_id, model, faces)
    }

    /// Records the pets found in a photo (possibly none).
    pub fn store_pets(&self, photo_id: i64, model: &str, pets: &[NewFace]) -> Result<(), Error> {
        self.store(Subject::Pet, photo_id, model, pets)
    }

    fn store(
        &self,
        subject: Subject,
        photo_id: i64,
        model: &str,
        faces: &[NewFace],
    ) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM faces WHERE photo_id = ?1 AND confirmed = 0 AND kind = ?2",
            params![photo_id, subject.as_str()],
        )?;
        // Faces someone confirmed keep who they are: a face found in the same
        // place takes over its row (and person), with the new description.
        // This is how names survive a better model searching photos again.
        let mut confirmed: Vec<(i64, [f32; 4])> = {
            let mut stmt = tx.prepare_cached(
                "SELECT id, x, y, w, h FROM faces WHERE photo_id = ?1 AND kind = ?2",
            )?;
            let rows = stmt.query_map(params![photo_id, subject.as_str()], |r| {
                Ok((r.get(0)?, [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?]))
            })?;
            rows.collect::<Result<_, _>>()?
        };
        {
            let mut insert = tx.prepare_cached(
                "INSERT INTO faces (photo_id, x, y, w, h, score, embedding, kind)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            let mut update = tx.prepare_cached(
                "UPDATE faces SET x = ?2, y = ?3, w = ?4, h = ?5, score = ?6, embedding = ?7
                 WHERE id = ?1",
            )?;
            for f in faces {
                let [x, y, w, h] = f.bbox;
                let same_place = confirmed
                    .iter()
                    .enumerate()
                    .map(|(i, c)| (i, overlap(&c.1, &f.bbox)))
                    .filter(|&(_, o)| o > 0.4)
                    .max_by(|a, b| a.1.total_cmp(&b.1));
                match same_place {
                    Some((i, _)) => {
                        let (id, _) = confirmed.swap_remove(i);
                        update.execute(params![id, x, y, w, h, f.score, pack(&f.embedding)])?;
                    }
                    None => {
                        insert.execute(params![
                            photo_id,
                            x,
                            y,
                            w,
                            h,
                            f.score,
                            pack(&f.embedding),
                            subject.as_str()
                        ])?;
                    }
                }
            }
        }
        tx.execute(
            &format!(
                "INSERT OR REPLACE INTO {} (photo_id, model) VALUES (?1, ?2)",
                subject.scans()
            ),
            params![photo_id, model],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// How many photos have been searched for faces.
    pub fn faces_scanned(&self, model: &str) -> Result<usize, Error> {
        self.scanned(Subject::Person, model)
    }

    /// How many photos have been searched for pets.
    pub fn pets_scanned(&self, model: &str) -> Result<usize, Error> {
        self.scanned(Subject::Pet, model)
    }

    fn scanned(&self, subject: Subject, model: &str) -> Result<usize, Error> {
        Ok(self.conn.lock().query_row(
            &format!("SELECT count(*) FROM {} WHERE model = ?1", subject.scans()),
            [model],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }

    /// Files every face (and pet) nobody has filed yet: under the person it
    /// looks like, or into a new unnamed group with the faces it looks like.
    /// Returns whether anything changed.
    pub fn group_faces(&self) -> Result<bool, Error> {
        let people = self.group(Subject::Person)?;
        let pets = self.group(Subject::Pet)?;
        Ok(people || pets)
    }

    fn group(&self, subject: Subject) -> Result<bool, Error> {
        let same = subject.same();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        let mut people: HashMap<i64, Centroid> = HashMap::new();
        let mut loose = Vec::new();
        // Only descriptions from the current model compare: the newest face
        // says which that is. Faces from an earlier model wait to be searched
        // again (keeping their person meanwhile).
        let current: Option<i64> = tx
            .query_row(
                "SELECT length(embedding) FROM faces WHERE kind = ?1 ORDER BY id DESC LIMIT 1",
                [subject.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        {
            let mut stmt = tx.prepare(
                "SELECT id, person_id, not_person, embedding FROM faces
                 WHERE kind = ?1 AND length(embedding) = ?2 ORDER BY id",
            )?;
            let mut rows = stmt.query(params![subject.as_str(), current.unwrap_or(0)])?;
            while let Some(r) = rows.next()? {
                let embedding = subject.comparable(unpack(&r.get::<_, Vec<u8>>(3)?));
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
                Some((person, sim)) if sim >= same => {
                    people.get_mut(&person).unwrap().add(&face.embedding);
                    filed.push((face.id, person));
                }
                _ => rest.push(face),
            }
        }
        // New groups among the rest.
        for ids in cluster(rest, same) {
            tx.execute(
                "INSERT INTO people (name, kind) VALUES (NULL, ?1)",
                [subject.as_str()],
            )?;
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
            "SELECT f.person_id, p.name, p.hidden, f.id, f.photo_id, f.confirmed, f.score, p.kind
             FROM faces f JOIN people p ON p.id = f.person_id",
        )?;
        type Entry = (Option<String>, bool, Subject, Vec<Filed>);
        let mut by_person: HashMap<i64, Entry> = HashMap::new();
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            let entry = by_person.entry(r.get(0)?).or_insert_with(|| {
                (
                    r.get(1).ok().flatten(),
                    r.get::<_, i64>(2).is_ok_and(|h| h != 0),
                    Subject::parse(&r.get::<_, String>(7).unwrap_or_default()),
                    Vec::new(),
                )
            });
            entry.3.push(Filed {
                id: r.get(3)?,
                photo: r.get(4)?,
                confirmed: r.get::<_, i64>(5)? != 0,
                score: r.get(6)?,
            });
        }
        // Named people with no faces left still exist.
        let mut named = conn
            .prepare_cached("SELECT id, name, hidden, kind FROM people WHERE name IS NOT NULL")?;
        let mut rows = named.query([])?;
        while let Some(r) = rows.next()? {
            by_person.entry(r.get(0)?).or_insert_with(|| {
                (
                    r.get(1).ok(),
                    r.get::<_, i64>(2).is_ok_and(|h| h != 0),
                    Subject::parse(&r.get::<_, String>(3).unwrap_or_default()),
                    Vec::new(),
                )
            });
        }
        let mut out: Vec<PersonRow> = by_person
            .into_iter()
            .map(|(id, (name, hidden, kind, faces))| {
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
                    kind,
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
            "SELECT f.id, f.person_id, p.name, f.x, f.y, f.w, f.h, f.kind
             FROM faces f LEFT JOIN people p ON p.id = f.person_id
             WHERE f.photo_id = ?1 AND NOT coalesce(p.hidden, 0) ORDER BY f.kind, f.x",
        )?;
        let rows = stmt.query_map([photo_id], |r| {
            Ok(FaceRow {
                id: r.get(0)?,
                person_id: r.get(1)?,
                name: r.get(2)?,
                bbox: [r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?],
                kind: Subject::parse(&r.get::<_, String>(7)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Which photo a face (or pet) is in, and where.
    pub fn face(&self, face_id: i64) -> Result<(i64, [f32; 4], Subject), Error> {
        Ok(self.conn.lock().query_row(
            "SELECT photo_id, x, y, w, h, kind FROM faces WHERE id = ?1",
            [face_id],
            |r| {
                Ok((
                    r.get(0)?,
                    [r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?],
                    Subject::parse(&r.get::<_, String>(5)?),
                ))
            },
        )?)
    }

    /// Names a person (or unnamed group). If someone already has that
    /// name, the two become one. Returns who they are now.
    pub fn name_person(&self, id: i64, name: &str) -> Result<i64, Error> {
        let name = name.trim();
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        // People and pets don't share names.
        let existing: Option<i64> = tx
            .query_row(
                "SELECT id FROM people WHERE name = ?1 AND id <> ?2
                   AND kind = (SELECT kind FROM people WHERE id = ?2)",
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
        let kind: String =
            tx.query_row("SELECT kind FROM faces WHERE id = ?1", [face_id], |r| {
                r.get(0)
            })?;
        let person = match tx
            .query_row(
                "SELECT id FROM people WHERE name = ?1 AND kind = ?2",
                params![name, kind],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
        {
            Some(id) => id,
            None => {
                tx.execute(
                    "INSERT INTO people (name, kind) VALUES (?1, ?2)",
                    params![name, kind],
                )?;
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
        // A person and a pet are never one.
        let kind_of = |id: i64| -> Result<Option<String>, Error> {
            Ok(tx
                .query_row("SELECT kind FROM people WHERE id = ?1", [id], |r| r.get(0))
                .optional()?)
        };
        let kind = kind_of(into)?;
        let mut same = Vec::new();
        for &id in from {
            if kind_of(id)? == kind {
                same.push(id);
            }
        }
        let from = &same[..];
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
        let (current, named, kind): (Option<i64>, bool, String) = tx.query_row(
            "SELECT f.person_id, p.name IS NOT NULL, f.kind
             FROM faces f LEFT JOIN people p ON p.id = f.person_id WHERE f.id = ?1",
            [face_id],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get::<_, Option<bool>>(1)?.unwrap_or(false),
                    r.get(2)?,
                ))
            },
        )?;
        let person = match current {
            Some(group) if !named => group,
            _ => {
                tx.execute(
                    "INSERT INTO people (name, hidden, kind) VALUES (NULL, 1, ?1)",
                    [kind],
                )?;
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
    fn pets_are_grouped_and_named_apart_from_people() {
        let lib = library(4);
        let p = ids(&lib);
        // Max the person and Max the cat look alike to the numbers here
        // (same axis), but are different kinds of description.
        lib.store_faces(p[0], "m", &[face(0, 0.0)]).unwrap();
        lib.store_faces(p[1], "m", &[face(0, 0.1)]).unwrap();
        lib.store_pets(p[1], "s", &[face(0, 0.05)]).unwrap();
        lib.store_pets(p[2], "s", &[face(0, 0.1)]).unwrap();
        lib.store_pets(p[3], "s", &[face(4, 0.0)]).unwrap();
        assert_eq!(lib.faces_todo("m").unwrap().len(), 2);
        assert_eq!(lib.pets_todo("s").unwrap().len(), 1);
        assert_eq!(lib.pets_scanned("s").unwrap(), 3);
        lib.group_faces().unwrap();

        let groups = lib.people().unwrap();
        let person = groups.iter().find(|g| g.kind == Subject::Person).unwrap();
        let pet = groups.iter().find(|g| g.kind == Subject::Pet).unwrap();
        assert_eq!(groups.len(), 2, "the other pet waits for company");
        assert_eq!(person.photo_ids, [p[0], p[1]]);
        assert_eq!(pet.photo_ids, [p[1], p[2]]);
        let (person, pet) = (person.id, pet.id);

        // The same name doesn't make them one.
        lib.name_person(person, "Max").unwrap();
        assert_eq!(lib.name_person(pet, "max").unwrap(), pet);
        assert_eq!(lib.people().unwrap().len(), 2);
        // Nor does merging.
        lib.merge_people(&[pet], person).unwrap();
        assert_eq!(lib.people().unwrap().len(), 2);

        // A pet's face in a photo says it's a pet; naming it finds the pet.
        let faces = lib.faces_in(p[1]).unwrap();
        assert_eq!(
            faces.iter().map(|f| f.kind).collect::<Vec<_>>(),
            [Subject::Person, Subject::Pet]
        );
        let other = lib.faces_in(p[3]).unwrap()[0].id;
        assert_eq!(lib.name_face(other, "Max").unwrap(), pet);
        assert_eq!(lib.face(other).unwrap().2, Subject::Pet);
        // Searching a photo for faces again leaves its pets alone.
        lib.store_faces(p[1], "m", &[]).unwrap();
        assert_eq!(lib.faces_in(p[1]).unwrap().len(), 2, "confirmed faces stay");
        lib.store_pets(p[3], "s", &[]).unwrap();
        assert_eq!(lib.faces_in(p[3]).unwrap().len(), 1, "named pets stay");
    }

    #[test]
    fn names_survive_a_new_face_model() {
        let lib = library(3);
        let p = ids(&lib);
        // The old model: Ann in two photos, named.
        lib.store_faces(p[0], "old", &[face(0, 0.0)]).unwrap();
        lib.store_faces(p[1], "old", &[face(0, 0.1)]).unwrap();
        lib.group_faces().unwrap();
        let ann = lib.people().unwrap()[0].id;
        lib.name_person(ann, "Ann").unwrap();

        // A new model with longer descriptions finds her in nearly the same
        // place in the first photo, and in a third photo.
        let newer = |wobble: f32, nudge: f32| {
            let mut f = face(0, wobble);
            f.embedding = [f.embedding.clone(), f.embedding].concat();
            let n = f.embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
            f.embedding.iter_mut().for_each(|x| *x /= n);
            f.bbox[0] += nudge;
            f
        };
        assert_eq!(lib.faces_todo("new").unwrap().len(), 3);
        lib.store_faces(p[0], "new", &[newer(0.0, 0.01)]).unwrap();
        lib.store_faces(p[2], "new", &[newer(0.05, 0.0)]).unwrap();
        lib.group_faces().unwrap();
        let people = lib.people().unwrap();
        assert_eq!(people.len(), 1, "no stray groups: {people:?}");
        assert_eq!(people[0].name.as_deref(), Some("Ann"));
        assert_eq!(people[0].photo_ids, p, "the old photo stays hers meanwhile");

        // Once every photo is searched again, all her faces are new ones.
        lib.store_faces(p[1], "new", &[newer(0.1, 0.0)]).unwrap();
        lib.group_faces().unwrap();
        assert_eq!(lib.people().unwrap()[0].faces, 3);
        assert!(lib.faces_todo("new").unwrap().is_empty());
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
        assert_eq!(people[0].kind, Subject::Person);
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
