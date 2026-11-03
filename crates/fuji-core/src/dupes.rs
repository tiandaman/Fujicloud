//! Finding duplicates: byte-identical copies (the same shot imported twice,
//! or copied into two folders), and copies of the same picture saved again
//! (resized for the web, re-saved, converted). Those are found by comparing
//! smart search vectors of photos taken within a short time of each other,
//! then checking the pictures themselves are at least 99.5% the same, so a
//! burst or the same scene taken again a moment later doesn't count.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::Kind;
use crate::db::PhotoRow;
use crate::decode::Rgb;
use crate::search::Index;

/// Smart search similarity at which two photos are alike enough to compare
/// their pictures, for SigLIP (each model has its own: [`crate::embed::ModelSpec::near_duplicate`]).
/// Checked against the real model in `embed`'s tests.
pub const NEAR_DUPLICATE: f32 = 0.92;
/// Copies are looked for among photos taken this close together.
pub const WINDOW_SECS: i64 = 120;

/// Cells across and down a [`Look`].
const CELLS: usize = 32;
/// Measured on 87 photos 1,500 to 6,000 px across, through the app's own
/// 512 px thumbnails: every copy re-saved at quality 70 or 60, or as a 2048
/// or 1080 px copy, counts (they differ by at most 0.49% on average, 1.09%
/// in their worst sixteenth and 0.13% in brightness), and 93% of tiny 600
/// px ones. The same shot with the camera moved 0.5% across counts 8 times
/// in 100, moved 1% once in 100, and 3% brighter never.
const MOST_DIFFERENT: f32 = 0.005;
const MOST_DIFFERENT_PART: f32 = 0.0125;
const MOST_BRIGHTER: f32 = 0.003;

/// A photo's picture boiled down for telling a copy of it from another
/// shot: its shape and its average colour in a 32 × 32 grid.
#[derive(Debug, Clone)]
pub struct Look {
    aspect: f32,
    cells: Vec<[f32; 3]>,
}

impl Look {
    /// From a picture (its cached thumbnail is plenty).
    pub fn of(image: &Rgb) -> Option<Look> {
        let (w, h) = (image.width as usize, image.height as usize);
        if w == 0 || h == 0 || image.data.len() < w * h * 3 {
            return None;
        }
        let span = |i: usize, n: usize| {
            let start = i * n / CELLS;
            (start, ((i + 1) * n / CELLS).max(start + 1).min(n))
        };
        let mut cells = Vec::with_capacity(CELLS * CELLS);
        for cy in 0..CELLS {
            let (y0, y1) = span(cy, h);
            for cx in 0..CELLS {
                let (x0, x1) = span(cx, w);
                let mut sum = [0f32; 3];
                for y in y0..y1 {
                    for px in image.data[(y * w + x0) * 3..(y * w + x1) * 3]
                        .as_chunks::<3>()
                        .0
                    {
                        for c in 0..3 {
                            sum[c] += px[c] as f32;
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as f32 * 255.0;
                cells.push(sum.map(|v| v / n));
            }
        }
        Some(Look {
            aspect: w as f32 / h as f32,
            cells,
        })
    }

    /// Whether `other` is the same picture: the same shape, at least 99.5%
    /// the same, no part of it (a sixteenth) more than 1.25% different, and
    /// no brighter or darker overall. Saving again, resizing and converting
    /// stay well inside that; anything that moved, the camera moving, or a
    /// change of exposure doesn't.
    pub fn same_picture(&self, other: &Look) -> bool {
        self.difference(other)
            .is_some_and(|(mean, worst_part, brighter)| {
                mean <= MOST_DIFFERENT
                    && worst_part <= MOST_DIFFERENT_PART
                    && brighter.abs() <= MOST_BRIGHTER
            })
    }

    /// How different `other` is on average, in its most different
    /// sixteenth, and in brightness, as fractions; none if another shape.
    fn difference(&self, other: &Look) -> Option<(f32, f32, f32)> {
        if (self.aspect / other.aspect - 1.0).abs() > 0.01 {
            return None;
        }
        let diff = |i: usize| -> f32 {
            let (a, b) = (self.cells[i], other.cells[i]);
            (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>() / 3.0
        };
        let n = (CELLS * CELLS) as f32;
        let mean = (0..CELLS * CELLS).map(diff).sum::<f32>() / n;
        let brightness = |l: &Look| l.cells.iter().flatten().sum::<f32>() / (n * 3.0);
        const PART: usize = CELLS / 4;
        let worst_part = (0..16)
            .map(|b| {
                let (bx, by) = (b % 4 * PART, b / 4 * PART);
                (0..PART * PART)
                    .map(|i| diff((by + i / PART) * CELLS + bx + i % PART))
                    .sum::<f32>()
                    / (PART * PART) as f32
            })
            .fold(0f32, f32::max);
        Some((mean, worst_part, brightness(other) - brightness(self)))
    }
}

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

/// Pairs of photos taken within [`WINDOW_SECS`] that smart search says
/// look alike: those that might be copies.
fn similar_pairs(rows: &[&PhotoRow], index: &Index, near: f32) -> Vec<(i64, i64)> {
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
                .filter(move |(_, b)| index.similarity(a, *b).is_some_and(|s| s >= near))
                .map(move |(_, b)| (a, *b))
        })
        .collect()
}

/// Groups of duplicates, biggest first, each with the photo to keep first:
/// the best rated, then picked, then the most pixels, then the biggest file.
/// `index` is smart search's, with the similarity at which its model counts
/// two photos as alike enough to compare their pictures, which `look` gives
/// (none, for one that can't be had, and it isn't counted a copy).
pub fn find(
    rows: &[PhotoRow],
    index: Option<(&Index, f32)>,
    look: &(dyn Fn(i64) -> Option<Look> + Sync),
    checksum: &(dyn Fn(&Path) -> Option<u128> + Sync),
) -> Vec<Group> {
    let shown = candidates(rows);
    let (exact, sums) = exact_pairs(&shown, checksum);
    let alike = index.map_or_else(Vec::new, |(i, near)| similar_pairs(&shown, i, near));
    let mut ids: Vec<i64> = alike.iter().flat_map(|&(a, b)| [a, b]).collect();
    ids.sort_unstable();
    ids.dedup();
    let looks: HashMap<i64, Look> = ids
        .into_par_iter()
        .filter_map(|id| Some((id, look(id)?)))
        .collect();
    let similar: Vec<(i64, i64)> = alike
        .into_iter()
        .filter(|(a, b)| match (looks.get(a), looks.get(b)) {
            (Some(a), Some(b)) => a.same_picture(b),
            _ => false,
        })
        .collect();

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
    fn finds_copies_and_keeps_the_best() {
        let mut rows = vec![
            // The same file imported twice.
            row(1, "DSCF0001.JPG", Kind::Jpeg, 9_000_000, 1000),
            row(2, "DSCF0001_1.JPG", Kind::Jpeg, 9_000_000, 1000),
            // Same size by chance, different bytes.
            row(3, "DSCF0002.JPG", Kind::Jpeg, 9_000_000, 50_000),
            // 5 is a copy of 4 saved again; 6 looks nothing like them; 7
            // is a copy but an hour later; 9 is the next shot of a burst,
            // alike to smart search but not the same picture.
            row(4, "DSCF0010.JPG", Kind::Jpeg, 8_000_001, 2000),
            row(5, "DSCF0011.JPG", Kind::Jpeg, 8_000_002, 2001),
            row(6, "DSCF0012.JPG", Kind::Jpeg, 8_000_003, 2002),
            row(7, "DSCF0013.JPG", Kind::Jpeg, 8_000_004, 5600),
            // A RAW stacked under 4: never its own duplicate.
            row(8, "DSCF0010.RAF", Kind::Raf, 30_000_000, 2000),
            row(9, "DSCF0014.JPG", Kind::Jpeg, 8_000_005, 2003),
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
            (9, look(&[0.97, 0.1, 0.0])),
        ]);
        let picture = |shift: usize| {
            let (w, h) = (96, 64);
            let data = (0..w * h)
                .flat_map(|i| {
                    let (x, y) = ((i % w + shift) as u8, (i / w) as u8);
                    [x.wrapping_mul(5), y.wrapping_mul(7), x ^ y]
                })
                .collect();
            Look::of(&Rgb {
                data,
                width: w as u32,
                height: h as u32,
            })
        };
        let pictures = |id: i64| match id {
            9 => picture(9),
            6 => None,
            _ => picture(0),
        };
        let sums = |p: &Path| -> Option<u128> {
            let name = p.file_name()?.to_string_lossy().into_owned();
            Some(if name.starts_with("DSCF0001") {
                1
            } else {
                name.len() as u128 * 7 + 2
            })
        };
        let groups = find(&rows, Some((&index, NEAR_DUPLICATE)), &pictures, &sums);
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

    /// Through the app's own thumbnails: a copy saved again smaller and at
    /// a lower quality is the same picture; the camera moved 1%, a change
    /// of exposure, or something moving in part of it isn't.
    #[test]
    fn only_copies_are_the_same_picture() {
        use crate::thumbs;
        let (w, h) = (1800usize, 1200usize);
        let scene = |dx: f32, gain: f32, moved: bool| {
            let data: Vec<u8> = (0..w * h)
                .flat_map(|i| {
                    let (x, y) = ((i % w) as f32 + dx, (i / w) as f32);
                    // Broad shapes, as in a photo: a sky, hills, a sun.
                    let (sx, sy) = (x - 1300.0, y - 250.0);
                    let mut px = if sx * sx + sy * sy < 120.0 * 120.0 {
                        [250.0, 230.0, 160.0]
                    } else if y > 650.0 + 120.0 * (x / 210.0).sin() {
                        [60.0 + 30.0 * (x / 97.0).sin(), 110.0, 50.0]
                    } else {
                        [100.0 + y / 8.0, 150.0 + y / 10.0, 230.0]
                    };
                    // Someone walking through the lower left corner.
                    if moved && (300.0..450.0).contains(&x) && y > 800.0 {
                        px = [40.0, 40.0, 60.0];
                    }
                    px.map(|c| (c * gain).clamp(0.0, 255.0) as u8)
                })
                .collect();
            Rgb {
                data,
                width: w as u32,
                height: h as u32,
            }
        };
        let look = |image: Rgb, quality: u8, edge: u32| {
            let small =
                thumbs::render_rgb(image.data, image.width, image.height, edge, quality).unwrap();
            let r = thumbs::render_all(&small.jpeg).unwrap();
            Look::of(&thumbs::decode_rendition(&r.thumb.jpeg, 512).unwrap()).unwrap()
        };
        let original = look(scene(0.0, 1.0, false), 95, 1800);
        let check = |copy: &Look, same: bool| {
            println!("{:?}", original.difference(copy));
            assert_eq!(original.same_picture(copy), same);
        };
        check(&look(scene(0.0, 1.0, false), 60, 1080), true);
        check(&look(scene(0.0, 1.0, false), 80, 600), true);
        check(&look(scene(18.0, 1.0, false), 95, 1800), false);
        check(&look(scene(0.0, 1.03, false), 95, 1800), false);
        check(&look(scene(0.0, 1.0, true), 95, 1800), false);
        // Cropped to another shape.
        let mut square = scene(0.0, 1.0, false);
        square.data = (0..h)
            .flat_map(|y| square.data[y * w * 3..(y * w + h) * 3].to_vec())
            .collect();
        square.width = h as u32;
        check(&look(square, 95, 1200), false);
    }
}
