//! Finding duplicates: byte-identical copies (the same shot imported twice,
//! or copied into two folders), and near-identical shots (a burst, the same
//! picture taken again a moment later, a resized copy) found by comparing
//! smart search vectors of photos taken within a short time of each other.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::Kind;
use crate::db::PhotoRow;
use crate::search::Index;

/// Smart search similarity at which two photos count as near-identical.
/// Checked against the real model in `embed`'s tests.
pub const NEAR_DUPLICATE: f32 = 0.92;
/// Near-identical shots are looked for among photos taken this close together.
pub const WINDOW_SECS: i64 = 120;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    /// The photos, the one to keep first.
    pub ids: Vec<i64>,
    /// Every photo in the group is byte-for-byte the same file.
    pub exact: bool,
}

/// Photos that stand for themselves in the grid: not a RAW or Live Photo
/// movie stacked under its picture, and not an edit (which is meant to look
/// like its original).
fn candidates(rows: &[PhotoRow]) -> Vec<&PhotoRow> {
    let kind: HashMap<i64, Kind> = rows.iter().map(|p| (p.id, p.kind)).collect();
    rows.iter()
        .filter(|p| p.edit_of.is_none())
        .filter(|p| {
            let secondary = matches!(p.kind, Kind::Raf | Kind::Dng | Kind::Video);
            let twin_is_picture = p
                .pair_id
                .and_then(|t| kind.get(&t))
                .is_some_and(|k| matches!(k, Kind::Jpeg | Kind::Heif | Kind::Png));
            !(secondary && twin_is_picture)
        })
        .collect()
}

fn path(p: &PhotoRow) -> PathBuf {
    Path::new(&p.folder).join(&p.name)
}

/// Pairs of byte-identical files. Only files of the same kind and size are
/// read, so this is quick unless there really are many copies.
fn exact_pairs(
    rows: &[&PhotoRow],
    checksum: &(dyn Fn(&Path) -> Option<u128> + Sync),
) -> (Vec<(i64, i64)>, HashMap<i64, u128>) {
    let mut by_size: HashMap<(i64, Kind), Vec<&PhotoRow>> = HashMap::new();
    for p in rows {
        by_size.entry((p.size, p.kind)).or_default().push(p);
    }
    let suspects: Vec<&PhotoRow> = by_size
        .into_values()
        .filter(|g| g.len() > 1)
        .flatten()
        .collect();
    let sums: HashMap<i64, u128> = suspects
        .par_iter()
        .filter_map(|p| Some((p.id, checksum(&path(p))?)))
        .collect();
    let mut by_sum: HashMap<u128, Vec<i64>> = HashMap::new();
    for (&id, &sum) in &sums {
        by_sum.entry(sum).or_default().push(id);
    }
    let pairs = by_sum
        .into_values()
        .filter(|ids| ids.len() > 1)
        .flat_map(|mut ids| {
            ids.sort_unstable();
            let first = ids[0];
            ids.into_iter().skip(1).map(move |id| (first, id))
        })
        .collect();
    (pairs, sums)
}

/// Pairs of photos taken within [`WINDOW_SECS`] that look the same.
fn similar_pairs(rows: &[&PhotoRow], index: &Index) -> Vec<(i64, i64)> {
    let mut timed: Vec<(i64, i64)> = rows
        .iter()
        .filter_map(|p| Some((p.taken_at?, p.id)))
        .filter(|(_, id)| index.contains(*id))
        .collect();
    timed.sort_unstable();
    (0..timed.len())
        .into_par_iter()
        .flat_map_iter(|i| {
            let (t, a) = timed[i];
            timed[i + 1..]
                .iter()
                .take_while(move |(u, _)| u - t <= WINDOW_SECS)
                .filter(move |(_, b)| index.similarity(a, *b).is_some_and(|s| s >= NEAR_DUPLICATE))
                .map(move |(_, b)| (a, *b))
        })
        .collect()
}

/// Groups of duplicates, biggest first, each with the photo to keep first:
/// the best rated, then picked, then the most pixels, then the biggest file.
pub fn find(
    rows: &[PhotoRow],
    index: Option<&Index>,
    checksum: &(dyn Fn(&Path) -> Option<u128> + Sync),
) -> Vec<Group> {
    let shown = candidates(rows);
    let (exact, sums) = exact_pairs(&shown, checksum);
    let similar = index.map_or_else(Vec::new, |i| similar_pairs(&shown, i));

    // Union-find over both kinds of pair.
    let mut parent: HashMap<i64, i64> = HashMap::new();
    fn root(parent: &mut HashMap<i64, i64>, id: i64) -> i64 {
        let p = *parent.entry(id).or_insert(id);
        if p == id {
            return id;
        }
        let r = root(parent, p);
        parent.insert(id, r);
        r
    }
    for &(a, b) in exact.iter().chain(&similar) {
        let (ra, rb) = (root(&mut parent, a), root(&mut parent, b));
        if ra != rb {
            parent.insert(ra.max(rb), ra.min(rb));
        }
    }
    let mut groups: HashMap<i64, Vec<i64>> = HashMap::new();
    for id in parent.keys().copied().collect::<Vec<_>>() {
        let r = root(&mut parent, id);
        groups.entry(r).or_default().push(id);
    }

    let by_id: HashMap<i64, &PhotoRow> = rows.iter().map(|p| (p.id, p)).collect();
    let mut out: Vec<Group> = groups
        .into_values()
        .filter(|ids| ids.len() > 1)
        .map(|mut ids| {
            ids.sort_by_key(|id| {
                let p = by_id[id];
                (
                    std::cmp::Reverse(p.rating),
                    std::cmp::Reverse(p.flag),
                    std::cmp::Reverse(p.width as u64 * p.height as u64),
                    std::cmp::Reverse(p.size),
                    p.added_at,
                    p.id,
                )
            });
            let first = sums.get(&ids[0]);
            let exact = first.is_some() && ids.iter().all(|id| sums.get(id) == first);
            Group { ids, exact }
        })
        .collect();
    out.sort_by_key(|g| (std::cmp::Reverse(g.ids.len()), g.ids[0]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::quantize;

    fn row(id: i64, name: &str, kind: Kind, size: i64, taken: i64) -> PhotoRow {
        PhotoRow {
            id,
            source_id: 1,
            name: name.into(),
            folder: "/lib".into(),
            kind,
            size,
            mtime: 0,
            taken_at: Some(taken),
            width: 6000,
            height: 4000,
            camera: None,
            lens: None,
            focal_length: None,
            aperture: None,
            shutter: None,
            iso: None,
            film_sim: None,
            rating: 0,
            flag: 0,
            label: None,
            added_at: id,
            pair_id: None,
            recipe_key: None,
            lat: None,
            lon: None,
            gps_estimated: false,
            town: None,
            region: None,
            country: None,
            edit_of: None,
            favorite: false,
            duration: None,
        }
    }

    fn unit(v: &[f32]) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter().map(|x| x / n).collect()
    }

    #[test]
    fn finds_copies_and_near_identical_shots_and_keeps_the_best() {
        let mut rows = vec![
            // The same file imported twice.
            row(1, "DSCF0001.JPG", Kind::Jpeg, 9_000_000, 1000),
            row(2, "DSCF0001_1.JPG", Kind::Jpeg, 9_000_000, 1000),
            // Same size by chance, different bytes.
            row(3, "DSCF0002.JPG", Kind::Jpeg, 9_000_000, 50_000),
            // A burst: 4 and 5 look alike, 6 doesn't; 7 looks alike but is
            // an hour later.
            row(4, "DSCF0010.JPG", Kind::Jpeg, 8_000_001, 2000),
            row(5, "DSCF0011.JPG", Kind::Jpeg, 8_000_002, 2001),
            row(6, "DSCF0012.JPG", Kind::Jpeg, 8_000_003, 2002),
            row(7, "DSCF0013.JPG", Kind::Jpeg, 8_000_004, 5600),
            // A RAW stacked under 4: never its own duplicate.
            row(8, "DSCF0010.RAF", Kind::Raf, 30_000_000, 2000),
        ];
        rows[4].rating = 4; // 5 is the keeper of the burst
        rows[3].pair_id = Some(8);
        rows[7].pair_id = Some(4);

        let look = |v: &[f32]| quantize(&unit(v));
        let index = Index::new(vec![
            (4, look(&[1.0, 0.0, 0.0])),
            (5, look(&[0.98, 0.05, 0.0])),
            (6, look(&[0.0, 1.0, 0.0])),
            (7, look(&[1.0, 0.0, 0.0])),
            (8, look(&[1.0, 0.0, 0.0])),
        ]);
        let sums = |p: &Path| -> Option<u128> {
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some(if name.starts_with("DSCF0001") {
                1
            } else {
                name.len() as u128 * 7 + 2
            })
        };
        let groups = find(&rows, Some(&index), &sums);
        assert_eq!(
            groups,
            vec![
                Group {
                    ids: vec![1, 2],
                    exact: true
                },
                Group {
                    ids: vec![5, 4],
                    exact: false
                },
            ]
        );
    }
}
