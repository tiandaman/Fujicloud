// Builds crates/fuji-core/data/places.tsv.gz from the GeoNames gazetteer
// (via the `cities.json` package): every town of 1000+ people, compacted for
// offline reverse geocoding. Run with `node scripts/build-places.mjs`.
//
// Data © GeoNames (https://www.geonames.org), CC BY 4.0.

import { readFileSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { gzipSync } from 'node:zlib';

const require = createRequire(import.meta.url);
const cities = require('cities.json/cities.json');
const admin1 = new Map(require('cities.json/admin1.json').map((a) => [a.code, a.name]));

// Region table: "CC\tRegion name", referenced by index from each place.
const regions = new Map();
const regionIndex = (country, code) => {
  const key = `${country}\t${admin1.get(`${country}.${code}`) ?? ''}`;
  if (!regions.has(key)) regions.set(key, regions.size);
  return regions.get(key);
};

const places = cities.map((c) => {
  const lat = Math.round(Number(c.lat) * 1e4);
  const lon = Math.round(Number(c.lng) * 1e4);
  return `${c.name}\t${lat}\t${lon}\t${regionIndex(c.country, c.admin1)}`;
});

const text = ['#regions', ...regions.keys(), '#places', ...places, ''].join('\n');
const out = new URL('../crates/fuji-core/data/places.tsv.gz', import.meta.url);
writeFileSync(out, gzipSync(text, { level: 9 }));
const pkg = JSON.parse(readFileSync(new URL('../node_modules/cities.json/package.json', import.meta.url), 'utf8'));
console.log(`${places.length} places, ${regions.size} regions from cities.json ${pkg.version}`);
