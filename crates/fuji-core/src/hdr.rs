//! HDR HEIFs (Fujifilm's HLG mode, or PQ) shown on ordinary screens.
//!
//! Their pixels are encoded for an HDR display, so read as if they were
//! normal images they look flat and grey. Each pixel is turned into the
//! light an HDR display would show (BT.2100), scaled so HDR reference white
//! (203 nits) sits a little below SDR white, with a smooth shoulder that
//! brings brighter highlights down to fit, then converted from the wide
//! BT.2020 (or P3) colours to sRGB.

use rayon::prelude::*;

/// H.273 transfer characteristics.
pub const PQ: u16 = 16;
pub const HLG: u16 = 18;

/// HDR reference white, in nits (ITU-R BT.2408).
const REFERENCE_WHITE: f32 = 203.0;
/// Where reference white lands on the SDR scale (linear light).
const WHITE_LEVEL: f32 = 0.75;
/// Below this (SDR linear) the curve is a straight line; above it, the
/// shoulder compresses highlights towards 1.
const KNEE: f32 = 0.6;

pub fn is_hdr(transfer: u16) -> bool {
    matches!(transfer, PQ | HLG)
}

/// PQ (SMPTE ST 2084) signal → nits.
pub fn pq_nits(e: f32) -> f32 {
    const M1: f32 = 2610.0 / 16384.0;
    const M2: f32 = 2523.0 / 4096.0 * 128.0;
    const C1: f32 = 3424.0 / 4096.0;
    const C2: f32 = 2413.0 / 4096.0 * 32.0;
    const C3: f32 = 2392.0 / 4096.0 * 32.0;
    let p = e.clamp(0.0, 1.0).powf(1.0 / M2);
    10000.0 * ((p - C1).max(0.0) / (C2 - C3 * p)).powf(1.0 / M1)
}

/// HLG signal → scene light, 0..=1 (the inverse OETF of BT.2100).
pub fn hlg_scene(e: f32) -> f32 {
    const A: f32 = 0.178_832_77;
    const B: f32 = 0.284_668_92;
    const C: f32 = 0.559_910_7;
    let e = e.clamp(0.0, 1.0);
    if e <= 0.5 {
        e * e / 3.0
    } else {
        (((e - C) / A).exp() + B) / 12.0
    }
}

/// The shoulder: linear up to [`KNEE`], then easing towards 1.
pub fn shoulder(y: f32) -> f32 {
    if y <= KNEE {
        y.max(0.0)
    } else {
        let room = 1.0 - KNEE;
        KNEE + room * (1.0 - (-(y - KNEE) / room).exp())
    }
}

/// Linear light → sRGB's curve, 0..=1.
fn srgb_encode(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// Linear RGB in the given H.273 primaries → linear sRGB (BT.709).
fn to_srgb_primaries(primaries: u16) -> [[f32; 3]; 3] {
    match primaries {
        // BT.2020
        9 => [
            [1.660_491, -0.587_641_1, -0.072_849_86],
            [-0.124_550_5, 1.132_899_9, -0.008_349_42],
            [-0.018_150_76, -0.100_578_9, 1.118_729_7],
        ],
        // Display P3
        12 => [
            [1.224_940_2, -0.224_940_2, 0.0],
            [-0.042_056_955, 1.042_057, 0.0],
            [-0.019_637_555, -0.078_636_05, 1.098_273_6],
        ],
        _ => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
    }
}

/// Converts 16-bit HDR RGB (as signalled: still PQ- or HLG-encoded) to
/// 8-bit sRGB.
pub fn to_sdr(rgb16: &[u16], transfer: u16, primaries: u16) -> Vec<u8> {
    // Signal → light, once per possible 16-bit value.
    let light: Vec<f32> = (0..=u16::MAX)
        .map(|v| {
            let e = v as f32 / 65535.0;
            if transfer == PQ {
                pq_nits(e) / REFERENCE_WHITE
            } else {
                hlg_scene(e)
            }
        })
        .collect();
    // Linear → sRGB 8-bit, finely enough that no step shows.
    const STEPS: usize = 16384;
    let encode: Vec<u8> = (0..=STEPS)
        .map(|i| (srgb_encode(i as f32 / STEPS as f32) * 255.0).round() as u8)
        .collect();
    let to_srgb = to_srgb_primaries(primaries);
    let lum = if primaries == 9 {
        [0.2627, 0.6780, 0.0593]
    } else {
        [0.2126, 0.7152, 0.0722]
    };

    let mut out = vec![0u8; rgb16.len()];
    out.par_chunks_mut(3 * 4096)
        .zip(rgb16.par_chunks(3 * 4096))
        .for_each(|(out, rgb)| {
            for (o, px) in out
                .as_chunks_mut::<3>()
                .0
                .iter_mut()
                .zip(rgb.as_chunks::<3>().0)
            {
                let mut c = [
                    light[px[0] as usize],
                    light[px[1] as usize],
                    light[px[2] as usize],
                ];
                if transfer == HLG {
                    // The HLG system gamma for a 1000-nit display (BT.2100),
                    // in units of reference white.
                    let ys = lum[0] * c[0] + lum[1] * c[1] + lum[2] * c[2];
                    let gain = 1000.0 * ys.max(1e-6).powf(0.2) / REFERENCE_WHITE;
                    c = c.map(|v| v * gain);
                }
                // Reference white to WHITE_LEVEL; the brightest channel
                // decides how much the shoulder pulls a pixel down, so
                // colours keep their hue.
                let c = c.map(|v| v * WHITE_LEVEL);
                let peak = c[0].max(c[1]).max(c[2]);
                let scale = if peak > KNEE {
                    shoulder(peak) / peak
                } else {
                    1.0
                };
                for (k, row) in to_srgb.iter().enumerate() {
                    let v = (row[0] * c[0] + row[1] * c[1] + row[2] * c[2]) * scale;
                    o[k] = encode[(v.clamp(0.0, 1.0) * STEPS as f32).round() as usize];
                }
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tolerance: f32) -> bool {
        (a - b).abs() <= tolerance
    }

    #[test]
    fn the_standard_curves_hit_their_reference_points() {
        assert!(close(pq_nits(1.0), 10000.0, 1.0));
        assert!(close(pq_nits(0.0), 0.0, 1e-6));
        // BT.2408: 100 nits ≈ 50.8% PQ, reference white 203 nits ≈ 58%.
        assert!(close(pq_nits(0.5081), 100.0, 1.0), "{}", pq_nits(0.5081));
        assert!(close(pq_nits(0.5806), 203.0, 2.0), "{}", pq_nits(0.5806));
        // HLG: the curve's two halves meet at 1/12, and 1.0 is full scale.
        assert!(close(hlg_scene(0.5), 1.0 / 12.0, 1e-5));
        assert!(close(hlg_scene(1.0), 1.0, 1e-4));
    }

    #[test]
    fn hlg_reference_white_is_203_nits() {
        // 75% HLG grey on a 1000-nit display (BT.2408).
        let e = hlg_scene(0.75);
        let nits = 1000.0 * e.powf(0.2) * e;
        assert!(close(nits, 203.0, 2.0), "{nits}");
    }

    #[test]
    fn the_shoulder_is_smooth_and_never_clips() {
        assert_eq!(shoulder(0.3), 0.3);
        assert!(
            close(shoulder(KNEE + 1e-4), KNEE + 1e-4, 1e-5),
            "continuous"
        );
        let mut last = 0.0;
        for i in 0..2000 {
            let y = shoulder(i as f32 / 100.0);
            // Far past peak white it reaches 1.0 in f32, never more.
            assert!(y >= last && y <= 1.0, "rising and at most 1 at {i}");
            last = y;
        }
        assert!(shoulder(5.0) > 0.99);
    }

    fn sdr(signal: [f32; 3], transfer: u16) -> [u8; 3] {
        let px = signal.map(|v| (v * 65535.0).round() as u16);
        to_sdr(&px, transfer, 9).try_into().unwrap()
    }

    #[test]
    fn hdr_greys_land_where_an_sdr_photo_would_have_them() {
        // Reference white: bright, but with room left for highlights.
        let white = sdr([0.75; 3], HLG);
        assert!(white.iter().all(|&v| (215..=240).contains(&v)), "{white:?}");
        // The same white from a PQ file looks the same.
        let pq_white = sdr([0.5806; 3], PQ);
        assert!(
            pq_white.iter().zip(white).all(|(&a, b)| a.abs_diff(b) <= 3),
            "{pq_white:?}"
        );
        // Highlights roll off instead of clipping…
        let bright = sdr([0.9; 3], HLG)[0];
        let peak = sdr([1.0; 3], HLG)[0];
        assert!(
            white[0] < bright && bright < peak,
            "{white:?} {bright} {peak}"
        );
        // …and black stays black, shadows stay dark.
        assert_eq!(sdr([0.0; 3], HLG), [0, 0, 0]);
        assert!(sdr([0.25; 3], HLG)[0] < 110);
    }

    #[test]
    fn wide_gamut_colours_keep_their_hue() {
        let [r, g, b] = sdr([0.7, 0.2, 0.2], HLG);
        assert!(r > 150 && g < r / 2 && b < r / 2, "{r} {g} {b}");
        let [r, g, b] = sdr([0.2, 0.2, 0.7], HLG);
        assert!(b > r && b > g, "{r} {g} {b}");
    }
}
