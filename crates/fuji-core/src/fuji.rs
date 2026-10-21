//! Fujifilm MakerNote decoding: the film simulation and the rest of the
//! in-camera "recipe" (white balance shift, tone curve, colour, grain, …).
//!
//! The MakerNote is a little-endian IFD prefixed with `FUJIFILM` and a 4-byte
//! offset; all value offsets are relative to the start of the MakerNote.
//! Tag numbers and encodings follow ExifTool's FujiFilm tables.

use serde::{Deserialize, Serialize};

const SHARPNESS: u16 = 0x1001;
const WHITE_BALANCE: u16 = 0x1002;
const SATURATION: u16 = 0x1003;
const COLOR_TEMPERATURE: u16 = 0x1005;
const WB_FINE_TUNE: u16 = 0x100a;
const NOISE_REDUCTION: u16 = 0x100e;
const CLARITY: u16 = 0x100f;
const SHADOW_TONE: u16 = 0x1040;
const HIGHLIGHT_TONE: u16 = 0x1041;
const GRAIN_ROUGHNESS: u16 = 0x1047;
const COLOR_CHROME: u16 = 0x1048;
const BW_ADJUSTMENT: u16 = 0x1049;
const BW_MAGENTA_GREEN: u16 = 0x104b;
const GRAIN_SIZE: u16 = 0x104c;
const COLOR_CHROME_BLUE: u16 = 0x104e;
const FILM_MODE: u16 = 0x1401;
const DYNAMIC_RANGE_SETTING: u16 = 0x1402;
const DEVELOPMENT_DYNAMIC_RANGE: u16 = 0x1403;
const D_RANGE_PRIORITY: u16 = 0x1443;
const D_RANGE_PRIORITY_AUTO: u16 = 0x1444;
const D_RANGE_PRIORITY_FIXED: u16 = 0x1445;

/// The settings that make up a Fujifilm film recipe. Absent fields were not
/// recorded by the camera (older bodies lack Clarity, Color Chrome, …).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Recipe {
    pub film_sim: Option<String>,
    /// "Off", "Weak, Small", "Strong, Large", …
    pub grain: Option<String>,
    pub color_chrome: Option<String>,
    pub color_chrome_blue: Option<String>,
    /// "Auto", "Daylight", "5600K", …
    pub white_balance: Option<String>,
    /// White balance shift as (red, blue), each −9…+9.
    pub wb_shift: Option<(i32, i32)>,
    /// "DR100", "DR400", "DR Auto", "D-Range Priority Strong", …
    pub dynamic_range: Option<String>,
    pub highlight: Option<f32>,
    pub shadow: Option<f32>,
    pub color: Option<i32>,
    pub sharpness: Option<i32>,
    pub noise_reduction: Option<i32>,
    pub clarity: Option<i32>,
    /// Monochrome toning: warm (+) / cool (−).
    pub mono_warm_cool: Option<i32>,
    /// Monochrome toning: magenta (+) / green (−).
    pub mono_magenta_green: Option<i32>,
}

impl Recipe {
    /// A canonical fingerprint: two photos with the same key were shot with
    /// the same recipe. Exposure isn't part of a recipe, so it's excluded.
    pub fn key(&self) -> Option<String> {
        let film = self.film_sim.as_deref()?;
        let opt = |v: Option<String>| v.unwrap_or_else(|| "-".into());
        let num = |v: Option<i32>| opt(v.map(|v| v.to_string()));
        let half = |v: Option<f32>| opt(v.map(|v| format!("{v:.1}")));
        Some(
            [
                film.to_owned(),
                opt(self.grain.clone()),
                opt(self.color_chrome.clone()),
                opt(self.color_chrome_blue.clone()),
                opt(self.white_balance.clone()),
                opt(self.wb_shift.map(|(r, b)| format!("{r},{b}"))),
                opt(self.dynamic_range.clone()),
                half(self.highlight),
                half(self.shadow),
                num(self.color),
                num(self.sharpness),
                num(self.noise_reduction),
                num(self.clarity),
                num(self.mono_warm_cool),
                num(self.mono_magenta_green),
            ]
            .join("|"),
        )
    }
}

/// Reads one IFD entry's values as integers, following the offset when the
/// payload doesn't fit in the 4-byte value field.
fn values(mn: &[u8], entry: usize) -> Option<Vec<i64>> {
    let u16_at = |at: usize| mn.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| {
        mn.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };
    let kind = u16_at(entry + 2)?;
    let count = u32_at(entry + 4)? as usize;
    let size = match kind {
        1 | 6 => 1,
        3 | 8 => 2,
        4 | 9 => 4,
        _ => return None,
    };
    if count == 0 || count > 16 {
        return None;
    }
    let start = if size * count <= 4 {
        entry + 8
    } else {
        u32_at(entry + 8)? as usize
    };
    let data = mn.get(start..start + size * count)?;
    Some(
        data.chunks(size)
            .map(|c| match kind {
                1 => c[0] as i64,
                6 => c[0] as i8 as i64,
                3 => u16::from_le_bytes([c[0], c[1]]) as i64,
                8 => i16::from_le_bytes([c[0], c[1]]) as i64,
                4 => u32::from_le_bytes(c.try_into().unwrap()) as i64,
                _ => i32::from_le_bytes(c.try_into().unwrap()) as i64,
            })
            .collect(),
    )
}

pub fn parse_makernote(mn: &[u8]) -> Option<Recipe> {
    if mn.len() < 14 || &mn[..8] != b"FUJIFILM" {
        return None;
    }
    let ifd = u32::from_le_bytes(mn[8..12].try_into().unwrap()) as usize;
    let count = mn
        .get(ifd..ifd + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))? as usize;

    let mut tags = std::collections::HashMap::new();
    for i in 0..count.min(512) {
        let entry = ifd + 2 + i * 12;
        let Some(tag) = mn
            .get(entry..entry + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
        else {
            break;
        };
        if let Some(v) = values(mn, entry) {
            tags.insert(tag, v);
        }
    }
    let one = |tag: u16| tags.get(&tag).and_then(|v| v.first().copied());

    // Monochrome simulations are encoded in Saturation, and FilmMode is absent.
    let saturation = one(SATURATION);
    let mono = saturation.and_then(monochrome_name);
    let film_sim = mono
        .or_else(|| one(FILM_MODE).and_then(film_mode_name))
        .map(str::to_owned);

    let strength = |v: i64| match v {
        0 => Some("Off"),
        32 => Some("Weak"),
        64 => Some("Strong"),
        _ => None,
    };
    let grain = match (one(GRAIN_ROUGHNESS).and_then(strength), one(GRAIN_SIZE)) {
        (Some("Off"), _) => Some("Off".to_owned()),
        (Some(r), Some(16)) => Some(format!("{r}, Small")),
        (Some(r), Some(32)) => Some(format!("{r}, Large")),
        (Some(r), _) => Some(r.to_owned()),
        _ => None,
    };

    let white_balance = one(WHITE_BALANCE).and_then(|wb| match wb {
        0xff0 => one(COLOR_TEMPERATURE).map(|k| format!("{k}K")),
        wb => white_balance_name(wb).map(str::to_owned),
    });
    // Newer bodies store the shift ×20 (so +2 is 40); the camera's range is ±9.
    let wb_shift = tags.get(&WB_FINE_TUNE).filter(|v| v.len() >= 2).map(|v| {
        let step = |x: i64| (if x.abs() > 9 { x / 20 } else { x }) as i32;
        (step(v[0]), step(v[1]))
    });

    let dynamic_range = match (
        one(D_RANGE_PRIORITY),
        one(DYNAMIC_RANGE_SETTING),
        one(DEVELOPMENT_DYNAMIC_RANGE),
    ) {
        (Some(0), ..) => Some(format!(
            "D-Range Priority Auto{}",
            match one(D_RANGE_PRIORITY_AUTO) {
                Some(1) => " (Weak)",
                Some(2) => " (Strong)",
                _ => "",
            }
        )),
        (Some(1), ..) => Some(
            match one(D_RANGE_PRIORITY_FIXED) {
                Some(2) => "D-Range Priority Strong",
                _ => "D-Range Priority Weak",
            }
            .to_owned(),
        ),
        (_, Some(0), _) => Some("DR Auto".to_owned()),
        (_, _, Some(v)) if v >= 100 => Some(format!("DR{v}")),
        _ => None,
    };

    // Tone is stored as −16 per step (so −32 is +2, harder); half steps exist.
    let tone = |tag| one(tag).map(|v| -(v as f32) / 16.0);

    Some(Recipe {
        film_sim,
        grain,
        color_chrome: one(COLOR_CHROME).and_then(strength).map(str::to_owned),
        color_chrome_blue: one(COLOR_CHROME_BLUE).and_then(strength).map(str::to_owned),
        white_balance,
        wb_shift,
        dynamic_range,
        highlight: tone(HIGHLIGHT_TONE),
        shadow: tone(SHADOW_TONE),
        // Colour has no meaning for monochrome sims; their toning is below.
        color: if mono.is_some() {
            None
        } else {
            saturation.and_then(color_level)
        },
        sharpness: one(SHARPNESS).and_then(sharpness_level),
        noise_reduction: one(NOISE_REDUCTION).and_then(noise_reduction_level),
        clarity: one(CLARITY).map(|v| (v / 1000) as i32),
        mono_warm_cool: mono.and(one(BW_ADJUSTMENT)).map(|v| v as i32),
        mono_magenta_green: mono.and(one(BW_MAGENTA_GREEN)).map(|v| v as i32),
    })
}

fn film_mode_name(v: i64) -> Option<&'static str> {
    Some(match v {
        0x000 => "Provia",
        0x100 | 0x110 | 0x130 | 0x300 => "Studio Portrait",
        0x120 => "Astia",
        0x200 | 0x400 => "Velvia",
        0x500 => "Pro Neg. Std",
        0x501 => "Pro Neg. Hi",
        0x600 => "Classic Chrome",
        0x700 => "Eterna",
        0x800 => "Classic Neg.",
        0x900 => "Eterna Bleach Bypass",
        0xA00 => "Nostalgic Neg.",
        0xB00 => "Reala Ace",
        _ => return None,
    })
}

fn monochrome_name(v: i64) -> Option<&'static str> {
    Some(match v {
        0x300 => "Monochrome",
        0x301 => "Monochrome +R",
        0x302 => "Monochrome +Ye",
        0x303 => "Monochrome +G",
        0x310 => "Sepia",
        0x500 => "Acros",
        0x501 => "Acros +R",
        0x502 => "Acros +Ye",
        0x503 => "Acros +G",
        _ => return None,
    })
}

fn white_balance_name(v: i64) -> Option<&'static str> {
    Some(match v {
        0x000 => "Auto",
        0x001 => "Auto (White Priority)",
        0x002 => "Auto (Ambience Priority)",
        0x100 => "Daylight",
        0x200 => "Cloudy",
        0x300 => "Fluorescent 1",
        0x301 => "Fluorescent 2",
        0x302 => "Fluorescent 3",
        0x303 | 0x304 => "Fluorescent",
        0x400 => "Incandescent",
        0x500 => "Flash",
        0x600 => "Underwater",
        0xf00..=0xf04 => "Custom",
        _ => return None,
    })
}

fn color_level(v: i64) -> Option<i32> {
    Some(match v {
        0x000 => 0,
        0x080 => 1,
        0x100 => 2,
        0x0c0 => 3,
        0x0e0 => 4,
        0x180 => -1,
        0x400 => -2,
        0x4c0 => -3,
        0x4e0 => -4,
        _ => return None,
    })
}

fn sharpness_level(v: i64) -> Option<i32> {
    Some(match v {
        0x00 => -4,
        0x01 => -3,
        0x02 => -2,
        0x82 => -1,
        0x03 => 0,
        0x84 => 1,
        0x04 => 2,
        0x05 => 3,
        0x06 => 4,
        _ => return None,
    })
}

fn noise_reduction_level(v: i64) -> Option<i32> {
    Some(match v {
        0x000 => 0,
        0x180 => 1,
        0x100 => 2,
        0x1c0 => 3,
        0x1e0 => 4,
        0x280 => -1,
        0x200 => -2,
        0x2c0 => -3,
        0x2e0 => -4,
        _ => return None,
    })
}

/// Builds a MakerNote from `(tag, type, values)` entries. Test helper.
#[doc(hidden)]
pub fn build_makernote_for_tests(entries: &[(u16, u16, &[i64])]) -> Vec<u8> {
    let header = 12;
    let table = 2 + entries.len() * 12 + 4;
    let mut out = b"FUJIFILM".to_vec();
    out.extend_from_slice(&(header as u32).to_le_bytes());
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let mut extra = Vec::new();
    for &(tag, kind, vals) in entries {
        let size = match kind {
            1 | 6 => 1,
            3 | 8 => 2,
            _ => 4,
        };
        let mut data = Vec::new();
        for &v in vals {
            match size {
                1 => data.push(v as u8),
                2 => data.extend_from_slice(&(v as u16).to_le_bytes()),
                _ => data.extend_from_slice(&(v as u32).to_le_bytes()),
            }
        }
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&(vals.len() as u32).to_le_bytes());
        if data.len() <= 4 {
            data.resize(4, 0);
            out.extend_from_slice(&data);
        } else {
            let offset = header + table + extra.len();
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            extra.extend_from_slice(&data);
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&extra);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typical "Kodachrome-style" Classic Chrome recipe.
    fn classic_chrome() -> Vec<u8> {
        build_makernote_for_tests(&[
            (SHARPNESS, 3, &[0x82]),
            (WHITE_BALANCE, 3, &[0x100]),
            (SATURATION, 3, &[0x100]),
            (WB_FINE_TUNE, 9, &[40, -80]),
            (NOISE_REDUCTION, 3, &[0x2e0]),
            (CLARITY, 9, &[2000]),
            (SHADOW_TONE, 9, &[-8]),
            (HIGHLIGHT_TONE, 9, &[16]),
            (GRAIN_ROUGHNESS, 9, &[32]),
            (COLOR_CHROME, 9, &[64]),
            (GRAIN_SIZE, 3, &[16]),
            (COLOR_CHROME_BLUE, 9, &[32]),
            (FILM_MODE, 3, &[0x600]),
            (DYNAMIC_RANGE_SETTING, 3, &[1]),
            (DEVELOPMENT_DYNAMIC_RANGE, 3, &[400]),
        ])
    }

    #[test]
    fn full_recipe() {
        let r = parse_makernote(&classic_chrome()).unwrap();
        assert_eq!(r.film_sim.as_deref(), Some("Classic Chrome"));
        assert_eq!(r.grain.as_deref(), Some("Weak, Small"));
        assert_eq!(r.color_chrome.as_deref(), Some("Strong"));
        assert_eq!(r.color_chrome_blue.as_deref(), Some("Weak"));
        assert_eq!(r.white_balance.as_deref(), Some("Daylight"));
        assert_eq!(r.wb_shift, Some((2, -4)));
        assert_eq!(r.dynamic_range.as_deref(), Some("DR400"));
        assert_eq!(r.highlight, Some(-1.0));
        assert_eq!(r.shadow, Some(0.5));
        assert_eq!(r.color, Some(2));
        assert_eq!(r.sharpness, Some(-1));
        assert_eq!(r.noise_reduction, Some(-4));
        assert_eq!(r.clarity, Some(2));
        assert_eq!(r.mono_warm_cool, None);
        assert!(
            r.key()
                .unwrap()
                .starts_with("Classic Chrome|Weak, Small|Strong|Weak|Daylight|2,-4|DR400")
        );
    }

    #[test]
    fn monochrome_with_toning_and_kelvin() {
        let mn = build_makernote_for_tests(&[
            (SATURATION, 3, &[0x501]),
            (WHITE_BALANCE, 3, &[0xff0]),
            (COLOR_TEMPERATURE, 3, &[6500]),
            (BW_ADJUSTMENT, 6, &[2]),
            (BW_MAGENTA_GREEN, 6, &[-1]),
            (D_RANGE_PRIORITY, 3, &[1]),
            (D_RANGE_PRIORITY_FIXED, 3, &[2]),
        ]);
        let r = parse_makernote(&mn).unwrap();
        assert_eq!(r.film_sim.as_deref(), Some("Acros +R"));
        assert_eq!(r.white_balance.as_deref(), Some("6500K"));
        assert_eq!(r.color, None);
        assert_eq!(r.mono_warm_cool, Some(2));
        assert_eq!(r.mono_magenta_green, Some(-1));
        assert_eq!(r.dynamic_range.as_deref(), Some("D-Range Priority Strong"));
    }

    #[test]
    fn older_body_unscaled_shift_and_missing_tags() {
        let mn =
            build_makernote_for_tests(&[(FILM_MODE, 3, &[0x200]), (WB_FINE_TUNE, 9, &[-3, 5])]);
        let r = parse_makernote(&mn).unwrap();
        assert_eq!(r.film_sim.as_deref(), Some("Velvia"));
        assert_eq!(r.wb_shift, Some((-3, 5)));
        assert_eq!(r.clarity, None);
        assert_eq!(r.grain, None);
    }

    #[test]
    fn same_settings_same_key() {
        let a = parse_makernote(&classic_chrome()).unwrap();
        let b = parse_makernote(&classic_chrome()).unwrap();
        assert_eq!(a.key(), b.key());
        let other = Recipe {
            clarity: Some(0),
            ..a.clone()
        };
        assert_ne!(a.key(), other.key());
    }

    #[test]
    fn garbage_is_ignored() {
        assert_eq!(parse_makernote(b"NIKON\0\0\0\0\0\0\0\0\0"), None);
        let mut truncated = classic_chrome();
        truncated.truncate(40);
        assert!(parse_makernote(&truncated).is_some_and(|r| r.film_sim.is_none()));
    }
}
