//! Photos that are probably useless: black frames, blank ones, blown-out
//! ones and badly blurred ones, told from each photo's thumbnail. Nothing
//! is deleted on this say-so: they're shown to be looked through.
//!
//! The limits come from measuring 1,000 ordinary photos (Flickr30k's test
//! set) against the same photos ruined: none of the ordinary ones is
//! flagged, and black, blank, blown-out and heavily blurred copies all are.
//! Mild blur and camera shake don't show in a thumbnail's edges; the smart
//! search model, asked in words, catches many of those (see `BLURRY`).

use rusqlite::params;

use crate::Error;
use crate::db::Library;
use crate::decode::Rgb;

/// Changes when the measurements or their limits do, so photos are looked
/// at again.
pub const VERSION: &str = "junk-1";

pub(crate) const TABLES: &str = "
CREATE TABLE IF NOT EXISTS junk (
    photo_id INTEGER PRIMARY KEY REFERENCES photos(id) ON DELETE CASCADE,
    version  TEXT NOT NULL,
    -- Why it looks useless; NULL for a photo that doesn't.
    reason   TEXT,
    -- Looked at and kept: not offered again.
    kept     INTEGER NOT NULL DEFAULT 0
);
";

/// Why a photo looks useless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reason {
    /// Nothing but darkness: the lens cap, a pocket.
    Black,
    /// One flat tone with nothing in it.
    Blank,
    /// Mostly burnt-out white.
    Blown,
    /// No sharp edge anywhere.
    Blurry,
}

impl Reason {
    pub fn name(self) -> &'static str {
        match self {
            Reason::Black => "black",
            Reason::Blank => "blank",
            Reason::Blown => "blown",
            Reason::Blurry => "blurry",
        }
    }

    pub fn from_name(name: &str) -> Option<Reason> {
        Some(match name {
            "black" => Reason::Black,
            "blank" => Reason::Blank,
            "blown" => Reason::Blown,
            "blurry" => Reason::Blurry,
            _ => return None,
        })
    }
}

/// What's measured in a thumbnail's brightness (0–255).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measures {
    /// How much the brightness varies (standard deviation).
    pub spread: f32,
    /// The brightness all but the brightest hundredth is below.
    pub top: f32,
    /// The share of the picture that's burnt out (above 250).
    pub burnt: f32,
    /// The strength of the sharpest edges: the step in brightness between
    /// neighbours that only a thousandth of the picture exceeds, at half
    /// size (so JPEG noise doesn't count as detail).
    pub edge: f32,
}

/// The value `share` of `values` (0–255) lie at or below.
fn percentile(histogram: &[u32; 256], total: u32, share: f32) -> f32 {
    let want = (total as f32 * share).ceil().max(1.0) as u32;
    let mut seen = 0;
    for (value, &n) in histogram.iter().enumerate() {
        seen += n;
        if seen >= want {
            return value as f32;
        }
    }
    255.0
}

pub fn measure(image: &Rgb) -> Option<Measures> {
    let (w, h) = (image.width as usize, image.height as usize);
    if w < 16 || h < 16 || image.data.len() < w * h * 3 {
        return None;
    }
    let luma: Vec<f32> = image
        .data
        .chunks_exact(3)
        .take(w * h)
        .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
        .collect();
    let n = luma.len() as f32;
    let mean = luma.iter().sum::<f32>() / n;
    let spread = (luma.iter().map(|y| (y - mean).powi(2)).sum::<f32>() / n).sqrt();
    let mut tones = [0u32; 256];
    for &y in &luma {
        tones[(y as usize).min(255)] += 1;
    }
    let burnt = luma.iter().filter(|&&y| y > 250.0).count() as f32 / n;

    // Half size, each pixel the mean of four.
    let (hw, hh) = (w / 2, h / 2);
    let mut half = vec![0f32; hw * hh];
    for y in 0..hh {
        for x in 0..hw {
            let at = 2 * y * w + 2 * x;
            half[y * hw + x] = (luma[at] + luma[at + 1] + luma[at + w] + luma[at + w + 1]) / 4.0;
        }
    }
    let mut steps = [0u32; 256];
    for y in 0..hh - 1 {
        for x in 0..hw - 1 {
            let here = half[y * hw + x];
            let across = (half[y * hw + x + 1] - here).abs();
            let down = (half[(y + 1) * hw + x] - here).abs();
            steps[(across.max(down) as usize).min(255)] += 1;
        }
    }
    let edges = ((hh - 1) * (hw - 1)) as u32;
    Some(Measures {
        spread,
        top: percentile(&tones, luma.len() as u32, 0.99),
        burnt,
        edge: percentile(&steps, edges, 0.999),
    })
}

/// Whether a photo with these measurements looks useless, and why. Of the
/// 1,000 ordinary photos the lowest `top` was 80, the lowest `spread` 17,
/// the highest `burnt` 0.40 and the lowest `edge` 48; of heavily blurred
/// copies, 19 in 20 had an `edge` under 31.
pub fn verdict(m: &Measures) -> Option<Reason> {
    if m.top < 40.0 {
        Some(Reason::Black)
    } else if m.burnt > 0.6 {
        Some(Reason::Blown)
    } else if m.spread < 8.0 {
        Some(Reason::Blank)
    } else if m.edge < 40.0 {
        Some(Reason::Blurry)
    } else {
        None
    }
}

/// What the smart search model is asked, to find blur a thumbnail's edges
/// don't show: a photo is blurry when it's this much nearer the first
/// phrase than the second.
pub const BLURRY: (&str, &str) = ("a photo that is blurry", "a photo that is sharp");

/// How much nearer (see [`BLURRY`]) counts, for a smart search model.
/// Measured on 400 ordinary photos and blurred copies of them, each limit
/// flagging under 1 in 100 of the ordinary ones. SigLIP 2 base at 0.032:
/// 9 in 10 heavily blurred photos, half the shaken ones, 4 in 10 mildly
/// blurred. SigLIP 2 large at 0.036: about half of each. Models not
/// measured aren't asked.
pub fn blurry_margin(model: &str) -> Option<f32> {
    match model {
        "siglip2-base-patch16-224" => Some(0.032),
        "siglip2-large-patch16-256" => Some(0.036),
        _ => None,
    }
}

impl Library {
    /// Photos not looked at yet (by this version): not movies.
    pub fn junk_todo(&self) -> Result<Vec<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT p.id FROM photos p
             WHERE p.kind <> 'video'
               AND NOT EXISTS (SELECT 1 FROM junk j WHERE j.photo_id = p.id AND j.version = ?1)",
        )?;
        let rows = stmt.query_map([VERSION], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Records what was made of these photos. One that was kept stays kept.
    pub fn store_junk(&self, found: &[(i64, Option<Reason>)]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO junk (photo_id, version, reason) VALUES (?1, ?2, ?3)
                 ON CONFLICT(photo_id) DO UPDATE SET version = ?2, reason = ?3",
            )?;
            for (id, reason) in found {
                stmt.execute(params![id, VERSION, reason.map(Reason::name)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The photos that look useless and haven't been kept, with why.
    pub fn junk(&self) -> Result<Vec<(i64, Reason)>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached(
            "SELECT photo_id, reason FROM junk
             WHERE reason IS NOT NULL AND kept = 0 AND version = ?1",
        )?;
        let rows = stmt.query_map([VERSION], |r| Ok((r.get(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, reason) = row?;
            if let Some(reason) = Reason::from_name(&reason) {
                out.push((id, reason));
            }
        }
        Ok(out)
    }

    /// The photos looked at and kept: not to be offered again.
    pub fn junk_kept(&self) -> Result<std::collections::HashSet<i64>, Error> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare_cached("SELECT photo_id FROM junk WHERE kept = 1")?;
        let rows = stmt.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// These photos are worth keeping after all.
    pub fn keep_junk(&self, ids: &[i64]) -> Result<(), Error> {
        let mut conn = self.conn.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO junk (photo_id, version, kept) VALUES (?1, ?2, 1)
                 ON CONFLICT(photo_id) DO UPDATE SET kept = 1",
            )?;
            for id in ids {
                stmt.execute(params![id, VERSION])?;
            }
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture with detail in it: stripes and a gradient.
    fn picture(scale: f32, add: f32) -> Rgb {
        let (w, h) = (256u32, 192u32);
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let v = if (x / 16 + y / 16) % 2 == 0 {
                    60.0
                } else {
                    190.0
                } + (x as f32) / 8.0;
                let v = (v * scale + add).clamp(0.0, 255.0) as u8;
                data.extend([v, v, v]);
            }
        }
        Rgb {
            data,
            width: w,
            height: h,
        }
    }

    /// The picture, smeared over `radius` pixels each way.
    fn blurred(image: &Rgb, radius: i32) -> Rgb {
        let (w, h) = (image.width as i32, image.height as i32);
        let at = |x: i32, y: i32| {
            image.data[((y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) * 3) as usize] as f32
        };
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0.0;
                for dy in -radius..=radius {
                    for dx in -radius..=radius {
                        sum += at(x + dx, y + dy);
                    }
                }
                let v = (sum / ((2 * radius + 1).pow(2)) as f32) as u8;
                data.extend([v, v, v]);
            }
        }
        Rgb {
            data,
            width: image.width,
            height: image.height,
        }
    }

    #[test]
    fn useless_photos_are_told_from_good_ones() {
        let of = |image: &Rgb| verdict(&measure(image).unwrap());
        let good = picture(1.0, 0.0);
        assert_eq!(of(&good), None);
        assert_eq!(of(&picture(0.05, 2.0)), Some(Reason::Black));
        assert_eq!(of(&picture(0.02, 120.0)), Some(Reason::Blank));
        assert_eq!(of(&picture(0.02, 200.0)), Some(Reason::Blank));
        assert_eq!(of(&picture(6.0, 0.0)), Some(Reason::Blown));
        assert_eq!(of(&blurred(&good, 12)), Some(Reason::Blurry));
        // Too small to say.
        assert!(
            measure(&Rgb {
                data: vec![0; 48],
                width: 4,
                height: 4
            })
            .is_none()
        );
    }

    #[test]
    fn kept_photos_arent_offered_again() {
        use crate::Kind;
        use crate::db::NewPhoto;
        use std::path::{Path, PathBuf};
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
                photo("A.JPG", Kind::Jpeg),
                photo("B.JPG", Kind::Jpeg),
                photo("C.JPG", Kind::Jpeg),
                photo("D.MOV", Kind::Video),
            ],
        )
        .unwrap();
        let mut ids: Vec<i64> = lib
            .photos()
            .unwrap()
            .iter()
            .filter(|p| p.kind != Kind::Video)
            .map(|p| p.id)
            .collect();
        ids.sort();
        assert_eq!(lib.junk_todo().unwrap().len(), 3);
        lib.store_junk(&[
            (ids[0], Some(Reason::Black)),
            (ids[1], Some(Reason::Blurry)),
            (ids[2], None),
        ])
        .unwrap();
        assert!(lib.junk_todo().unwrap().is_empty());
        let mut found = lib.junk().unwrap();
        found.sort_by_key(|f| f.0);
        assert_eq!(
            found,
            vec![(ids[0], Reason::Black), (ids[1], Reason::Blurry)]
        );
        lib.keep_junk(&[ids[1], ids[2]]).unwrap();
        assert_eq!(lib.junk().unwrap(), vec![(ids[0], Reason::Black)]);
        // Looked at again, a kept photo stays kept.
        lib.store_junk(&[(ids[1], Some(Reason::Blurry))]).unwrap();
        assert_eq!(lib.junk().unwrap(), vec![(ids[0], Reason::Black)]);
        assert!(lib.junk_kept().unwrap().contains(&ids[2]));
    }

    /// Over a folder of real photos (`FUJICLOUD_JUNK_PHOTOS`): how many
    /// are flagged, and why.
    #[test]
    #[ignore]
    fn real_photos() {
        let dir = std::env::var("FUJICLOUD_JUNK_PHOTOS").unwrap();
        let (mut seen, mut flagged) = (0, Vec::new());
        let mut least = (255f32, 255f32, 0f32, 255f32);
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jpg") {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let Ok(image) = crate::thumbs::decode_rendition(&bytes, 512) else {
                continue;
            };
            let m = measure(&image).unwrap();
            seen += 1;
            least = (
                least.0.min(m.top),
                least.1.min(m.spread),
                least.2.max(m.burnt),
                least.3.min(m.edge),
            );
            if let Some(why) = verdict(&m) {
                flagged.push((path, why, m));
            }
        }
        eprintln!(
            "{seen} photos, {} flagged; extremes {least:?}",
            flagged.len()
        );
        for f in &flagged {
            eprintln!("  {f:?}");
        }
    }
}
