//! Fujifilm MakerNote decoding.
//!
//! The MakerNote is a little-endian IFD prefixed with `FUJIFILM` and a 4-byte
//! offset; all value offsets are relative to the start of the MakerNote.
//! We only need a couple of tags, so a tiny parser beats a full dependency.

const FILM_MODE: u16 = 0x1401;
const SATURATION: u16 = 0x1003;
const GRAIN_EFFECT: u16 = 0x1047;
const DYNAMIC_RANGE_SETTING: u16 = 0x1402;
const DEVELOPMENT_DYNAMIC_RANGE: u16 = 0x1403;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FujiInfo {
    pub film_simulation: Option<String>,
    pub grain: Option<String>,
    pub dynamic_range: Option<String>,
}

pub fn parse_makernote(mn: &[u8]) -> Option<FujiInfo> {
    if mn.len() < 14 || &mn[..8] != b"FUJIFILM" {
        return None;
    }
    let u16_at = |at: usize| mn.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| {
        mn.get(at..at + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
    };

    let ifd = u32_at(8)? as usize;
    let count = u16_at(ifd)? as usize;
    let mut film_mode = None;
    let mut saturation = None;
    let mut grain = None;
    let mut dr_setting = None;
    let mut dr_dev = None;

    for i in 0..count.min(512) {
        let entry = ifd + 2 + i * 12;
        let (Some(tag), Some(kind), Some(n)) =
            (u16_at(entry), u16_at(entry + 2), u32_at(entry + 4))
        else {
            break;
        };
        // Every tag we care about is a single SHORT (type 3) or LONG (type 4),
        // which is stored inline in the value field.
        let value = match (kind, n) {
            (3, 1) => u16_at(entry + 8).map(u32::from),
            (4, 1) => u32_at(entry + 8),
            _ => None,
        };
        match tag {
            FILM_MODE => film_mode = value,
            SATURATION => saturation = value,
            GRAIN_EFFECT => grain = value,
            DYNAMIC_RANGE_SETTING => dr_setting = value,
            DEVELOPMENT_DYNAMIC_RANGE => dr_dev = value,
            _ => {}
        }
    }

    // Monochrome simulations are encoded in Saturation, and FilmMode is absent.
    let film_simulation = saturation
        .and_then(monochrome_name)
        .or_else(|| film_mode.and_then(film_mode_name))
        .map(str::to_owned);

    let grain = grain.and_then(|g| match g {
        0 => Some("Off"),
        32 => Some("Weak"),
        64 => Some("Strong"),
        _ => None,
    });

    let dynamic_range = match (dr_setting, dr_dev) {
        (Some(0), _) => Some("DR Auto".to_owned()),
        (_, Some(v)) if v >= 100 => Some(format!("DR{v}")),
        _ => None,
    };

    Some(FujiInfo {
        film_simulation,
        grain: grain.map(str::to_owned),
        dynamic_range,
    })
}

fn film_mode_name(v: u32) -> Option<&'static str> {
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

fn monochrome_name(v: u32) -> Option<&'static str> {
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

/// Builds a MakerNote with the given `(tag, short value)` entries. Test helper.
#[doc(hidden)]
pub fn build_makernote_for_tests(entries: &[(u16, u16)]) -> Vec<u8> {
    let mut mn = b"FUJIFILM".to_vec();
    mn.extend_from_slice(&12u32.to_le_bytes());
    mn.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for &(tag, value) in entries {
        mn.extend_from_slice(&tag.to_le_bytes());
        mn.extend_from_slice(&3u16.to_le_bytes());
        mn.extend_from_slice(&1u32.to_le_bytes());
        mn.extend_from_slice(&value.to_le_bytes());
        mn.extend_from_slice(&[0, 0]);
    }
    mn.extend_from_slice(&0u32.to_le_bytes());
    mn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_simulation() {
        let mn =
            build_makernote_for_tests(&[(SATURATION, 0), (FILM_MODE, 0x600), (GRAIN_EFFECT, 32)]);
        let info = parse_makernote(&mn).unwrap();
        assert_eq!(info.film_simulation.as_deref(), Some("Classic Chrome"));
        assert_eq!(info.grain.as_deref(), Some("Weak"));
    }

    #[test]
    fn monochrome_overrides_film_mode() {
        let mn = build_makernote_for_tests(&[(SATURATION, 0x501)]);
        assert_eq!(
            parse_makernote(&mn).unwrap().film_simulation.as_deref(),
            Some("Acros +R")
        );
    }

    #[test]
    fn dynamic_range() {
        let mn = build_makernote_for_tests(&[
            (DYNAMIC_RANGE_SETTING, 1),
            (DEVELOPMENT_DYNAMIC_RANGE, 400),
        ]);
        assert_eq!(
            parse_makernote(&mn).unwrap().dynamic_range.as_deref(),
            Some("DR400")
        );
    }

    #[test]
    fn garbage_is_ignored() {
        assert_eq!(parse_makernote(b"NIKON\0\0\0\0\0\0\0\0\0"), None);
        let mut truncated = build_makernote_for_tests(&[(FILM_MODE, 0x800)]);
        truncated.truncate(20);
        assert_eq!(parse_makernote(&truncated).unwrap().film_simulation, None);
    }
}
