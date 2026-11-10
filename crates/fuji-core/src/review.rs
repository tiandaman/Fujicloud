//! Changes from other PCs, held for a look before they're applied.
//!
//! When a scan finds that another PC (or Lightroom) changed a photo's
//! rating, pick, colour label, favourite or albums in its sidecar, or that
//! a photo's file has gone (deleted elsewhere), and reviewing is on, the
//! change is held here instead of applied. Each can then be applied, or
//! rejected: a rejected change keeps this PC's marks, and a rejected
//! deletion brings the file back from the library folder's trash.

use std::path::{Path, PathBuf};

use rusqlite::{OptionalExtension, params};
use serde::Serialize;

use crate::db::Library;
use crate::xmp::Tags;
use crate::{Error, Kind};

pub(crate) const TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS sync_review (
    photo_id  INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    removed   INTEGER NOT NULL,
    rating    INTEGER NOT NULL DEFAULT 0,
    label     TEXT,
    flag      INTEGER NOT NULL DEFAULT 0,
    favorite  INTEGER NOT NULL DEFAULT 0,
    albums    TEXT NOT NULL DEFAULT '[]',
    xmp_mtime INTEGER NOT NULL DEFAULT 0,
    found_at  INTEGER NOT NULL DEFAULT (unixepoch())
);
"#;

/// What someone sets on a photo themselves.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Marks {
    pub rating: i32,
    /// `1` pick, `-1` reject.
    pub flag: i32,
    pub label: Option<String>,
    pub favorite: bool,
    pub albums: Vec<String>,
}

impl Marks {
    pub fn of(tags: &Tags) -> Marks {
        let mut albums: Vec<String> = tags.albums.iter().map(|a| a.trim().to_owned()).collect();
        albums.sort_by_key(|a| a.to_lowercase());
        albums.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        Marks {
            rating: tags.rating.clamp(0, 5),
            flag: tags.flag.clamp(-1, 1),
            label: tags.label.clone().filter(|l| !l.trim().is_empty()),
            favorite: tags.favorite,
            albums,
        }
    }
}

/// One held change, for the overview.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    pub id: i64,
    pub name: String,
    pub folder: String,
    pub kind: Kind,
    pub taken_at: Option<i64>,
    /// The file is gone: deleted, or moved away, on another PC.
    pub removed: bool,
    /// For a deletion: the file is in the library folder's trash, so
    /// rejecting it can put it back.
    pub restorable: bool,
    /// The marks here now.
    pub before: Marks,
    /// What the other PC saved (none for a deletion).
    pub after: Option<Marks>,
    /// The other halves of a RAW+JPEG pair, which share its sidecar and so
    /// this change: decided along with it.
    pub twins: Vec<i64>,
}

/// What resolving a set of changes did.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub applied: usize,
    pub kept: usize,
    pub restored: usize,
    /// Names of deleted photos that couldn't be brought back (not in the
    /// library folder's trash); they stay to be decided.
    pub failed: Vec<String>,
}

impl Library {
    /// Whether scans hold changes from other PCs for review rather than
    /// applying them.
    pub fn set_review(&self, on: bool) {
        self.review.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn reviewing(&self) -> bool {
        self.review.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Photos whose files are gone, held rather than forgotten. Photos held
    /// earlier in `source_id` whose files are back are let go.
    pub fn hold_removed(&self, source_id: i64, gone: &[i64]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO sync_review (photo_id, removed) VALUES (?1, 1)
                 ON CONFLICT(photo_id) DO UPDATE SET removed = 1",
            )?;
            for id in gone {
                stmt.execute([id])?;
            }
            let held: Vec<i64> = tx
                .prepare_cached(
                    "SELECT r.photo_id FROM sync_review r JOIN photos p ON p.id = r.photo_id
                     WHERE r.removed = 1 AND p.source_id = ?1",
                )?
                .query_map([source_id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            let mut back = tx.prepare_cached("DELETE FROM sync_review WHERE photo_id = ?1")?;
            for id in held.iter().filter(|id| !gone.contains(id)) {
                back.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Changes another PC saved in sidecars, held: `(photo, their tags,
    /// the sidecar's modified time)`.
    pub fn hold_changes(&self, items: &[(i64, Tags, i64)]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO sync_review (photo_id, removed, rating, label, flag, favorite, albums, xmp_mtime)
                 VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(photo_id) DO UPDATE SET removed = 0, rating = ?2, label = ?3,
                   flag = ?4, favorite = ?5, albums = ?6, xmp_mtime = ?7
                 WHERE removed = 0",
            )?;
            for (id, tags, mtime) in items {
                let m = Marks::of(tags);
                stmt.execute(params![
                    id,
                    m.rating,
                    m.label,
                    m.flag,
                    m.favorite as i64,
                    serde_json::to_string(&m.albums).unwrap_or_else(|_| "[]".into()),
                    mtime
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets held sidecar changes to these photos: a change made here
    /// since has been saved over them.
    pub(crate) fn drop_held_changes(&self, ids: &[i64]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt =
                tx.prepare_cached("DELETE FROM sync_review WHERE photo_id = ?1 AND removed = 0")?;
            for id in ids {
                stmt.execute([id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// A sidecar holding a held change was saved again with the same marks
    /// (only what the models found was added): the change is as of the new
    /// file, so deciding it doesn't make the next scan ask again.
    pub(crate) fn move_held(&self, ids: &[i64], mtime: i64) -> Result<(), Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "UPDATE sync_review SET xmp_mtime = ?2 WHERE photo_id = ?1 AND removed = 0",
        )?;
        for id in ids {
            stmt.execute(params![id, mtime])?;
        }
        Ok(())
    }

    /// How many changes are waiting.
    pub fn held_count(&self) -> Result<usize, Error> {
        let conn = self.conn.lock();
        Ok(conn.query_row("SELECT count(*) FROM sync_review", [], |r| {
            r.get::<_, i64>(0)
        })? as usize)
    }

    /// The marks set on a photo here.
    pub fn marks(&self, id: i64) -> Result<Marks, Error> {
        let conn = self.conn.lock();
        let mut marks = conn
            .query_row(
                "SELECT rating, flag, label, favorite FROM photos WHERE id = ?1",
                [id],
                |r| {
                    Ok(Marks {
                        rating: r.get(0)?,
                        flag: r.get(1)?,
                        label: r.get(2)?,
                        favorite: r.get::<_, i64>(3)? != 0,
                        albums: Vec::new(),
                    })
                },
            )
            .optional()?
            .ok_or(Error::NotFound(id))?;
        marks.albums = conn
            .prepare_cached(
                "SELECT a.name FROM albums a JOIN album_photos ap ON ap.album_id = a.id
                 WHERE ap.photo_id = ?1 ORDER BY lower(a.name)",
            )?
            .query_map([id], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(marks)
    }

    /// Every held change, newest photo first.
    pub fn held_changes(&self) -> Result<Vec<Change>, Error> {
        Ok(self.held_page(usize::MAX)?.0)
    }

    /// The photos with a held change (both halves of a pair).
    pub fn held_ids(&self) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT photo_id FROM sync_review")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The first `limit` held changes, newest photo first, and how many
    /// there are in all. (Each one shown is looked up in the library
    /// folder's trash, which on a network share takes a moment.)
    pub fn held_page(&self, limit: usize) -> Result<(Vec<Change>, usize), Error> {
        let rows: Vec<(Change, Marks)> = {
            let conn = self.conn.lock();
            let mut stmt = conn.prepare_cached(
                "SELECT p.id, p.name, p.folder, p.kind, p.taken_at, r.removed,
                        r.rating, r.flag, r.label, r.favorite, r.albums
                 FROM sync_review r JOIN photos p ON p.id = r.photo_id
                 ORDER BY p.taken_at DESC, p.name",
            )?;
            stmt.query_map([], |r| {
                let change = Change {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    folder: r.get(2)?,
                    kind: Kind::parse(&r.get::<_, String>(3)?).unwrap_or(Kind::Jpeg),
                    taken_at: r.get(4)?,
                    removed: r.get::<_, i64>(5)? != 0,
                    restorable: false,
                    before: Marks::default(),
                    after: None,
                    twins: Vec::new(),
                };
                let after = Marks {
                    rating: r.get(6)?,
                    flag: r.get(7)?,
                    label: r.get(8)?,
                    favorite: r.get::<_, i64>(9)? != 0,
                    albums: serde_json::from_str(&r.get::<_, String>(10)?).unwrap_or_default(),
                };
                Ok((change, after))
            })?
            .collect::<Result<_, _>>()?
        };
        // A pair's halves share a sidecar, so one change: shown once, on
        // the picture rather than the RAW.
        let changed: Vec<i64> = rows
            .iter()
            .filter(|(c, _)| !c.removed)
            .map(|(c, _)| c.id)
            .collect();
        let mut merged = std::collections::HashSet::new();
        let mut rows: Vec<(Change, Marks)> = rows;
        rows.sort_by_key(|(c, _)| (c.removed, is_raw(c.kind), c.id));
        let mut kept = Vec::with_capacity(rows.len());
        for (mut change, after) in rows {
            if merged.contains(&change.id) {
                continue;
            }
            if !change.removed {
                change.twins = self
                    .with_pairs(&[change.id])?
                    .into_iter()
                    .filter(|t| *t != change.id && changed.contains(t))
                    .collect();
                merged.extend(change.twins.iter().copied());
            }
            kept.push((change, after));
        }
        kept.sort_by(|(a, _), (b, _)| {
            b.taken_at
                .cmp(&a.taken_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        let total = kept.len();
        kept.truncate(limit);
        let roots = self.roots()?;
        let mut out = Vec::with_capacity(kept.len());
        for (mut change, after) in kept {
            change.before = self.marks(change.id)?;
            if change.removed {
                change.restorable = self.trashed_copy(change.id, &roots)?.is_some();
            } else {
                change.after = Some(after);
            }
            out.push(change);
        }
        Ok((out, total))
    }

    /// Applies these held changes (`apply`) and rejects those (`reject`).
    pub fn resolve_held(&self, apply: &[i64], reject: &[i64]) -> Result<Resolved, Error> {
        let mut done = Resolved::default();
        let roots = self.roots()?;
        // Deletions turned down first: a RAW put back then shares in a
        // change taken for its JPEG.
        for &id in reject.iter().chain(apply) {
            let accepted = apply.contains(&id);
            let held = {
                let conn = self.conn.lock();
                conn.query_row(
                    "SELECT removed, rating, flag, label, favorite, albums, xmp_mtime
                     FROM sync_review WHERE photo_id = ?1",
                    [id],
                    |r| {
                        Ok((
                            r.get::<_, i64>(0)? != 0,
                            Tags {
                                rating: r.get(1)?,
                                flag: r.get(2)?,
                                label: r.get(3)?,
                                favorite: r.get::<_, i64>(4)? != 0,
                                albums: serde_json::from_str(&r.get::<_, String>(5)?)
                                    .unwrap_or_default(),
                                ..Tags::default()
                            },
                            r.get::<_, i64>(6)?,
                        ))
                    },
                )
                .optional()?
            };
            let Some((removed, tags, mtime)) = held else {
                continue;
            };
            match (removed, accepted) {
                (true, true) => {
                    self.remove_photos(&[id])?;
                    done.applied += 1;
                    continue;
                }
                (true, false) => match self.restore(id, &roots) {
                    Ok(true) => done.restored += 1,
                    Ok(false) | Err(_) => {
                        let name = self.file_info(id).map(|f| {
                            f.path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        });
                        done.failed.push(name.unwrap_or_default());
                        continue;
                    }
                },
                (false, _) => {
                    // The whole pair shares the sidecar, so the decision.
                    let pair: Vec<i64> = self
                        .with_pairs(&[id])?
                        .into_iter()
                        .filter(|t| *t == id || !self.held_removed(*t).unwrap_or(true))
                        .collect();
                    if accepted {
                        let items: Vec<_> =
                            pair.iter().map(|&t| (t, tags.clone(), mtime)).collect();
                        self.apply_sidecars(&items)?;
                        done.applied += 1;
                    } else {
                        // Keep the marks here; don't ask about this version again.
                        let conn = self.conn.lock();
                        for t in &pair {
                            conn.execute(
                                "UPDATE photos SET xmp_mtime = ?2 WHERE id = ?1",
                                params![t, mtime],
                            )?;
                        }
                        done.kept += 1;
                    }
                    let conn = self.conn.lock();
                    for t in &pair {
                        conn.execute(
                            "DELETE FROM sync_review WHERE photo_id = ?1 AND removed = 0",
                            [t],
                        )?;
                    }
                    continue;
                }
            }
            let conn = self.conn.lock();
            conn.execute("DELETE FROM sync_review WHERE photo_id = ?1", [id])?;
        }
        Ok(done)
    }

    fn held_removed(&self, id: i64) -> Result<bool, Error> {
        let conn = self.conn.lock();
        Ok(conn
            .query_row(
                "SELECT removed FROM sync_review WHERE photo_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            == Some(1))
    }

    fn roots(&self) -> Result<Vec<(i64, PathBuf)>, Error> {
        Ok(self
            .sources()?
            .into_iter()
            .map(|s| (s.id, PathBuf::from(s.path)))
            .collect())
    }

    /// Where a deleted photo's file is in its library folder's trash.
    fn trashed_copy(&self, id: i64, roots: &[(i64, PathBuf)]) -> Result<Option<PathBuf>, Error> {
        let info = self.file_info(id)?;
        let Some((_, root)) = roots.iter().find(|(s, _)| *s == info.source_id) else {
            return Ok(None);
        };
        Ok(crate::discard::find_in_trash(
            root,
            &info.path,
            Some(info.size),
        ))
    }

    /// Puts a deleted photo's file (and its sidecar) back from the trash.
    fn restore(&self, id: i64, roots: &[(i64, PathBuf)]) -> Result<bool, Error> {
        let info = self.file_info(id)?;
        let Some((_, root)) = roots.iter().find(|(s, _)| *s == info.source_id) else {
            return Ok(false);
        };
        let Some(copy) = crate::discard::find_in_trash(root, &info.path, Some(info.size)) else {
            return Ok(false);
        };
        if info.path.exists() {
            return Ok(true);
        }
        move_back(&copy, &info.path)?;
        let sidecar = crate::xmp::sidecar_path(&info.path, info.kind);
        if !sidecar.exists()
            && let Some(copy) = crate::discard::find_in_trash(root, &sidecar, None)
        {
            move_back(&copy, &sidecar)?;
        }
        Ok(true)
    }
}

fn is_raw(kind: Kind) -> bool {
    matches!(kind, Kind::Raf | Kind::Dng)
}

fn move_back(from: &Path, to: &Path) -> Result<(), Error> {
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(from, to)?;
    Ok(())
}
