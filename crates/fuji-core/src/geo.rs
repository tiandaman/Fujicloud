//! Offline reverse geocoding: GPS coordinates → town, region, country.
//!
//! Uses the GeoNames gazetteer (every place with 1000+ people, ~171k entries,
//! built by `scripts/build-places.mjs`) bucketed into a 1° grid, so a lookup
//! only measures distances to the few hundred places in neighbouring cells.
//! Data © GeoNames, CC BY 4.0.

use std::collections::HashMap;
use std::io::Read;
use std::sync::OnceLock;

use serde::Serialize;

static DATA: &[u8] = include_bytes!("../data/places.tsv.gz");

/// Further than this from any town, a photo gets only a region and country.
const TOWN_RADIUS_KM: f64 = 35.0;
/// Further than this from anywhere (open sea, Antarctica), no place at all.
const MAX_RADIUS_KM: f64 = 250.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Place {
    pub town: Option<String>,
    pub region: Option<String>,
    /// ISO 3166-1 alpha-2, e.g. "JP".
    pub country: String,
}

struct Entry {
    name: Box<str>,
    lat: f32,
    lon: f32,
    region: u32,
}

pub struct Gazetteer {
    regions: Vec<(Box<str>, Box<str>)>,
    entries: Vec<Entry>,
    grid: HashMap<(i32, i32), Vec<u32>>,
}

/// The gazetteer, decompressed and indexed on first use (~100 ms).
pub fn gazetteer() -> &'static Gazetteer {
    static G: OnceLock<Gazetteer> = OnceLock::new();
    G.get_or_init(Gazetteer::load)
}

pub fn reverse(lat: f64, lon: f64) -> Option<Place> {
    gazetteer().reverse(lat, lon)
}

fn cell(lat: f64, lon: f64) -> (i32, i32) {
    (lat.floor() as i32, lon.floor() as i32)
}

fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dp = p2 - p1;
    let dl = (lon2 - lon1).to_radians();
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * 6371.0 * a.sqrt().min(1.0).asin()
}

impl Gazetteer {
    fn load() -> Self {
        let mut text = String::new();
        flate2::read::GzDecoder::new(DATA)
            .read_to_string(&mut text)
            .expect("bundled gazetteer is valid gzip");
        let mut regions = Vec::new();
        let mut entries = Vec::new();
        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        let mut in_places = false;
        for line in text.lines() {
            match line {
                "#regions" => continue,
                "#places" => {
                    in_places = true;
                    continue;
                }
                _ => {}
            }
            let mut f = line.split('\t');
            if !in_places {
                if let (Some(cc), Some(name)) = (f.next(), f.next()) {
                    regions.push((cc.into(), name.into()));
                }
                continue;
            }
            let (Some(name), Some(lat), Some(lon), Some(region)) =
                (f.next(), f.next(), f.next(), f.next())
            else {
                continue;
            };
            let (Ok(lat), Ok(lon), Ok(region)) = (
                lat.parse::<i32>(),
                lon.parse::<i32>(),
                region.parse::<u32>(),
            ) else {
                continue;
            };
            let (lat, lon) = (lat as f32 / 1e4, lon as f32 / 1e4);
            grid.entry(cell(lat as f64, lon as f64))
                .or_default()
                .push(entries.len() as u32);
            entries.push(Entry {
                name: name.into(),
                lat,
                lon,
                region,
            });
        }
        Self {
            regions,
            entries,
            grid,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn reverse(&self, lat: f64, lon: f64) -> Option<Place> {
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
            return None;
        }
        let (cy, cx) = cell(lat, lon);
        let mut best: Option<(f64, &Entry)> = None;
        // Rings of cells outward; 3 rings covers ≥ 250 km at any latitude
        // where people live.
        for ring in 0i32..=3 {
            for dy in -ring..=ring {
                for dx in -ring..=ring {
                    if dy.abs() != ring && dx.abs() != ring {
                        continue;
                    }
                    // Wrap across the antimeridian.
                    let x = (cx + dx + 180).rem_euclid(360) - 180;
                    let Some(ids) = self.grid.get(&(cy + dy, x)) else {
                        continue;
                    };
                    for &id in ids {
                        let e = &self.entries[id as usize];
                        let d = haversine_km(lat, lon, e.lat as f64, e.lon as f64);
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, e));
                        }
                    }
                }
            }
            // Anything in a further ring is at least `ring` cells away.
            if let Some((d, _)) = best
                && d < ring as f64 * 111.0 * lat.to_radians().cos().max(0.2)
            {
                break;
            }
        }
        let (d, e) = best?;
        if d > MAX_RADIUS_KM {
            return None;
        }
        let (country, region) = &self.regions[e.region as usize];
        Some(Place {
            town: (d <= TOWN_RADIUS_KM).then(|| e.name.to_string()),
            region: (!region.is_empty()).then(|| region.to_string()),
            country: country.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_places() {
        assert!(gazetteer().len() > 100_000);
        let kyoto = reverse(35.0116, 135.7681).unwrap();
        assert_eq!(kyoto.town.as_deref(), Some("Kyoto"));
        assert_eq!(kyoto.country, "JP");

        let sf = reverse(37.7793, -122.4192).unwrap();
        assert_eq!(sf.country, "US");
        assert_eq!(sf.region.as_deref(), Some("California"));

        // Across the antimeridian: Fiji.
        assert_eq!(reverse(-16.5, 179.99).unwrap().country, "FJ");
    }

    #[test]
    fn middle_of_nowhere() {
        // South Pacific.
        assert_eq!(reverse(-40.0, -130.0), None);
        assert_eq!(reverse(91.0, 0.0), None);
    }
}
