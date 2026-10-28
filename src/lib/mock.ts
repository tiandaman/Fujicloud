// A believable fake library so the UI can be developed and demoed in a plain
// browser (`npm run dev`) without the Rust backend or a camera.

import type { Api } from './api';
import type { Album, BackupStatus, Bootstrap, Detail, FaceInPhoto, FacesStatus, Hit, Kind, Label, Person, PetsStatus, Photo, Recipe, RecipeRow, Settings, SmartStatus, Source } from './types';

const FILMS = ['Classic Chrome', 'Classic Neg.', 'Provia', 'Velvia', 'Acros', 'Nostalgic Neg.', 'Reala Ace', 'Eterna', 'Astia', 'Pro Neg. Hi', 'Acros +R', 'Monochrome'];
const FILM_WEIGHTS = [18, 16, 12, 9, 8, 7, 7, 5, 5, 4, 3, 2];
const BODIES = [
  { camera: 'X-T5', lenses: [['XF23mmF1.4 R LM WR', 23, 1.4], ['XF56mmF1.2 R WR', 56, 1.2], ['XF16-55mmF2.8 R LM WR', 0, 2.8], ['XF70-300mmF4-5.6 R LM OIS WR', 0, 4]] },
  { camera: 'X100VI', lenses: [['', 23, 2]] },
  { camera: 'X-H2', lenses: [['XF33mmF1.4 R LM WR', 33, 1.4], ['XF8-16mmF2.8 R LM WR', 0, 2.8]] },
] as const;

const PLACES = [
  { town: 'Kyoto', region: 'Kyoto', country: 'JP', lat: 35.0116, lon: 135.7681 },
  { town: 'Tokyo', region: 'Tokyo', country: 'JP', lat: 35.6762, lon: 139.6503 },
  { town: 'Lisbon', region: 'Lisbon', country: 'PT', lat: 38.7223, lon: -9.1393 },
  { town: 'Reykjavík', region: 'Capital Region', country: 'IS', lat: 64.1466, lon: -21.9426 },
  { town: 'Vík', region: 'South', country: 'IS', lat: 63.4186, lon: -19.006 },
  { town: 'San Francisco', region: 'California', country: 'US', lat: 37.7749, lon: -122.4194 },
  { town: 'Big Sur', region: 'California', country: 'US', lat: 36.2704, lon: -121.8081 },
  { town: 'New York City', region: 'New York', country: 'US', lat: 40.7128, lon: -74.006 },
  { town: 'Austin', region: 'Texas', country: 'US', lat: 30.2672, lon: -97.7431 },
  { town: 'Mexico City', region: 'Mexico City', country: 'MX', lat: 19.4326, lon: -99.1332 },
  { town: 'Cape Town', region: 'Western Cape', country: 'ZA', lat: -33.9249, lon: 18.4241 },
  { town: 'Sydney', region: 'New South Wales', country: 'AU', lat: -33.8688, lon: 151.2093 },
  { town: 'Hà Nội', region: 'Hanoi', country: 'VN', lat: 21.0278, lon: 105.8342 },
  { town: 'Paris', region: 'Île-de-France', country: 'FR', lat: 48.8566, lon: 2.3522 },
];
const PLACE_WEIGHTS = [10, 8, 7, 4, 3, 9, 4, 6, 12, 3, 3, 4, 3, 5];

const DEFAULT_RECIPE: Recipe = {
  filmSim: null, grain: 'Off', colorChrome: 'Off', colorChromeBlue: 'Off', whiteBalance: 'Auto', wbShift: [0, 0],
  dynamicRange: 'DR100', highlight: 0, shadow: 0, color: 0, sharpness: 0, noiseReduction: 0, clarity: 0,
  monoWarmCool: null, monoMagentaGreen: null,
};
// Typical recipes people shoot; the mock library is built from these.
const RECIPES: Recipe[] = [
  { ...DEFAULT_RECIPE, filmSim: 'Classic Chrome', grain: 'Weak, Small', colorChrome: 'Strong', colorChromeBlue: 'Weak', whiteBalance: 'Daylight', wbShift: [2, -5], dynamicRange: 'DR400', highlight: 0, shadow: 0.5, color: 2, sharpness: 1, noiseReduction: -4, clarity: 2 },
  { ...DEFAULT_RECIPE, filmSim: 'Classic Neg.', grain: 'Weak, Small', colorChrome: 'Strong', colorChromeBlue: 'Off', whiteBalance: 'Auto', wbShift: [1, -2], dynamicRange: 'DR400', highlight: -1, shadow: 1, color: 4, sharpness: -1, noiseReduction: -4, clarity: -2 },
  { ...DEFAULT_RECIPE, filmSim: 'Nostalgic Neg.', grain: 'Strong, Small', colorChrome: 'Weak', colorChromeBlue: 'Weak', whiteBalance: '5600K', wbShift: [3, -4], dynamicRange: 'DR Auto', highlight: -1.5, shadow: 0, color: 1, sharpness: -2, noiseReduction: -4, clarity: 0 },
  { ...DEFAULT_RECIPE, filmSim: 'Acros +R', grain: 'Strong, Large', dynamicRange: 'DR200', highlight: 1, shadow: 2, color: null, sharpness: 1, noiseReduction: -4, clarity: 3, monoWarmCool: 2, monoMagentaGreen: 0 },
  { ...DEFAULT_RECIPE, filmSim: 'Reala Ace', grain: 'Weak, Small', colorChrome: 'Weak', colorChromeBlue: 'Weak', wbShift: [0, -1], dynamicRange: 'DR200', highlight: -0.5, shadow: -0.5, color: 1 },
  { ...DEFAULT_RECIPE, filmSim: 'Eterna', grain: 'Weak, Large', dynamicRange: 'DR400', highlight: -2, shadow: -2, color: -2, sharpness: -2, noiseReduction: -4, clarity: -3 },
  { ...DEFAULT_RECIPE, filmSim: 'Velvia', colorChrome: 'Strong', colorChromeBlue: 'Strong', whiteBalance: 'Daylight', wbShift: [1, -1], highlight: 0.5, shadow: 1, color: 3 },
  { ...DEFAULT_RECIPE, filmSim: 'Provia' },
];
const RECIPE_WEIGHTS = [16, 14, 9, 6, 8, 4, 5, 10];
const recipeKey = (r: Recipe) => JSON.stringify(r);
const recipeByKey = new Map(RECIPES.map((r) => [recipeKey(r), r]));

/** Every mock photo by id, so an edit can look like its original. */
const library = new Map<number, Photo>();

/** RAW+JPEG twins and edits share a scene with their original. */
const sceneSeed = (p: Photo) => (p.editOf ?? (p.pairId ? Math.min(p.id, p.pairId) : p.id)) * 7919;

/** What each fake photo "shows", for the mock smart search. */
function lightOf(p: Photo): 'golden' | 'dusk' | 'day' {
  const light = rng(sceneSeed(p))();
  return light > 0.78 ? 'golden' : light > 0.64 ? 'dusk' : 'day';
}

function rng(seed: number) {
  return () => {
    seed = (seed * 1664525 + 1013904223) >>> 0;
    return seed / 2 ** 32;
  };
}

function weighted<T>(r: () => number, items: readonly T[], weights: number[]): T {
  const total = weights.reduce((a, b) => a + b, 0);
  let x = r() * total;
  for (let i = 0; i < items.length; i++) {
    x -= weights[i];
    if (x <= 0) return items[i];
  }
  return items[items.length - 1];
}

function buildLibrary(days = 140): Photo[] {
  const r = rng(7);
  const photos: Photo[] = [];
  let id = 1;
  let t = Date.UTC(2026, 9, 21, 18, 40) / 1000;
  let shot = 9999;
  const now = Date.now() / 1000;
  // ~140 outings over two years, each a burst of related shots.
  for (let day = 0; day < days; day++) {
    const recipe = weighted(r, RECIPES, RECIPE_WEIGHTS);
    const film = r() < 0.75 ? recipe.filmSim! : weighted(r, FILMS, FILM_WEIGHTS);
    const key = film === recipe.filmSim ? recipeKey(recipe) : null;
    const place = weighted(r, PLACES, PLACE_WEIGHTS);
    // Location sync on for most outings; a few shots each drop out.
    const geotagged = r() < 0.8;
    const body = BODIES[Math.floor(r() * BODIES.length)];
    const count = 4 + Math.floor(r() ** 2 * 40);
    const rawJpeg = r() < 0.45;
    const folder = `\\\\SHIELD\\Samsung_T7\\Photos\\${new Date(t * 1000).toISOString().slice(0, 4)}\\${new Date(t * 1000).toISOString().slice(0, 10)}`;
    for (let i = 0; i < count; i++) {
      const lens = body.lenses[Math.floor(r() * body.lenses.length)];
      const portrait = r() < 0.22;
      const square = !portrait && r() < 0.05;
      const [w, h] = square ? [4160, 4160] : portrait ? [4160, 6240] : [6240, 4160];
      const focal = lens[1] || [16, 23, 35, 55, 90, 150, 300][Math.floor(r() * 7)];
      const name = `DSCF${String(shot).padStart(4, '0')}`;
      shot = shot <= 1 ? 9999 : shot - 1;
      const rating = r() < 0.7 ? 0 : Math.ceil(r() * 5);
      const flag = r() < 0.06 ? 1 : r() < 0.03 ? -1 : 0;
      const label = r() < 0.05 ? (['Red', 'Yellow', 'Green', 'Blue', 'Purple'] as Label[])[Math.floor(r() * 5)] : null;
      const base = {
        sourceId: 1,
        folder,
        takenAt: Math.round(t),
        width: w,
        height: h,
        camera: body.camera,
        lens: lens[0] || null,
        focalLength: focal,
        aperture: Math.max(lens[2], [1.4, 2, 2.8, 4, 5.6, 8, 11][Math.floor(r() * 7)]),
        shutter: 1 / [15, 30, 60, 125, 250, 500, 1000, 2000, 4000][Math.floor(r() * 9)],
        iso: [125, 160, 200, 400, 800, 1600, 3200, 6400][Math.floor(r() ** 1.6 * 8)],
        filmSim: film,
        rating,
        flag,
        label,
        addedAt: day < 3 ? Math.round(now - day * 3600) : Math.round(t + 86400),
        recipeKey: key,
        ...(geotagged
          ? {
              lat: place.lat + (r() - 0.5) * 0.06,
              lon: place.lon + (r() - 0.5) * 0.06,
              gpsEstimated: r() < 0.1,
              town: place.town,
              region: place.region,
              country: place.country,
            }
          : { lat: null, lon: null, gpsEstimated: false, town: null, region: null, country: null }),
      };
      const kind: Kind = r() < 0.04 ? 'heif' : 'jpeg';
      const jpg: Photo = { ...base, id: id++, name: `${name}.${kind === 'heif' ? 'HIF' : 'JPG'}`, kind, size: 9e6 + r() * 9e6, mtime: Math.round(t), pairId: null, editOf: null, favorite: rating === 5 && r() < 0.5, duration: null };
      photos.push(jpg);
      if (rawJpeg && kind === 'jpeg') {
        const raf: Photo = { ...base, id: id++, name: `${name}.RAF`, kind: 'raf', size: 3.2e7 + r() * 2e7, mtime: Math.round(t), pairId: jpg.id, editOf: null, favorite: jpg.favorite, duration: null };
        jpg.pairId = raf.id;
        photos.push(raf);
      }
      // Now and then the phone came out too: Live Photos, clips, ProRAW and screenshots.
      if (id % 41 === 0) {
        const phone = { ...base, camera: 'iPhone 16 Pro', lens: 'iPhone 16 Pro', filmSim: null, recipeKey: null, editOf: null, favorite: false, mtime: Math.round(t) };
        const n = 4000 + id;
        const pick = id % 4;
        if (pick === 0) {
          const still: Photo = { ...phone, id: id++, name: `IMG_${n}.HEIC`, kind: 'heif', size: 2.4e6, pairId: null, duration: null };
          const movie: Photo = { ...phone, id: id++, name: `IMG_${n}.MOV`, kind: 'video', size: 3.1e6, pairId: still.id, duration: 2.6 };
          still.pairId = movie.id;
          photos.push(still, movie);
        } else if (pick === 1) {
          photos.push({ ...phone, id: id++, name: `IMG_${n}.MOV`, kind: 'video', size: 8.6e7, pairId: null, duration: 12 + (n % 90) });
        } else if (pick === 2) {
          photos.push({ ...phone, id: id++, name: `IMG_${n}.DNG`, kind: 'dng', size: 7.4e7, pairId: null, duration: null });
        } else {
          photos.push({ ...phone, id: id++, name: `IMG_${n}.PNG`, kind: 'png', size: 1.9e6, pairId: null, duration: null, lat: null, lon: null, town: null, region: null, country: null });
        }
      }
      // The keepers get taken into Photoshop.
      if (kind === 'jpeg' && r() < (rating >= 3 ? 0.35 : 0.03)) {
        const x = r();
        const [ext, editKind, size]: [string, Kind, number] = x < 0.45 ? ['-Edit.jpg', 'jpeg', 1.2e7] : x < 0.85 ? ['.psd', 'psd', 1.4e8] : ['.tif', 'tiff', 1.5e8];
        const edited = Math.round(t + 86400 * (1 + r() * 20));
        photos.push({
          ...base,
          id: id++,
          name: `${name}${ext}`,
          kind: editKind,
          size: size * (0.8 + r() * 0.4),
          mtime: edited,
          addedAt: edited,
          pairId: null,
          editOf: jpg.id,
          filmSim: null,
          recipeKey: null,
          rating: Math.max(rating, 4),
          flag: 0,
          favorite: false,
          duration: null,
        });
      }
      t -= 20 + r() * 400;
    }
    t -= 86400 * (1 + r() * 8);
  }
  library.clear();
  for (const p of photos) library.set(p.id, p);
  return photos;
}

// ---- synthetic images --------------------------------------------------------

const palettes: Record<string, string[][]> = {
  'Classic Chrome': [['#9fb3b8', '#d9cdb4', '#506066', '#2f3b3e', '#6d7f73']],
  'Classic Neg.': [['#a7c4bc', '#f1d3a6', '#3e5a52', '#1f2e2a', '#b0754a']],
  Provia: [['#6fa8ff', '#cfe6ff', '#2d6a4f', '#1b4332', '#95d5b2']],
  Velvia: [['#ff6f3c', '#ffd23f', '#6a0572', '#2a0a3d', '#ff3c78']],
  'Nostalgic Neg.': [['#e7b77a', '#fbe3c0', '#7a4f2a', '#3c2614', '#c98b4e']],
  'Reala Ace': [['#7cc6fe', '#f5f0e1', '#3a7d44', '#22442a', '#f2a65a']],
  Eterna: [['#7f9aa6', '#c2cbc4', '#40525c', '#222c31', '#8a9a8d']],
  Astia: [['#f4a6a0', '#fde2d0', '#7f8fa6', '#3e4757', '#c9a7c7']],
  'Pro Neg. Hi': [['#b3a6ff', '#ffe0ec', '#4b4e8c', '#262845', '#ff9eb5']],
};

const svgCache = new Map<number, string>();

function sceneFor(p: Photo): string {
  const cached = svgCache.get(p.id);
  if (cached) return cached;
  const r = rng(sceneSeed(p));
  const edit = p.editOf != null;
  const film = (edit ? library.get(p.editOf!)?.filmSim : p.filmSim) ?? null;
  const mono = !!film && (film.startsWith('Acros') || film.startsWith('Mono'));
  const pal = mono ? ['#9a9a9a', '#e6e6e6', '#4a4a4a', '#1c1c1c', '#7a7a7a'] : (palettes[film ?? ''] ?? palettes.Provia)[0];
  // Vary the light a little so a day's shots don't all look the same.
  r();
  const light = lightOf(p);
  const sky = mono ? [pal[0], pal[1]] : light === 'golden' ? ['#f2a65a', '#fbd7a1'] : light === 'dusk' ? ['#22304a', '#6b7aa1'] : [pal[0], pal[1]];
  const W = 600;
  const H = Math.round((600 * p.height) / p.width);
  const sunX = 80 + r() * (W - 160);
  const sunY = H * (0.18 + r() * 0.3);
  const hill = (base: number, amp: number, seed: number) => {
    const rr = rng(seed);
    let d = `M0 ${H} L0 ${base}`;
    for (let x = 0; x <= W; x += 40) d += ` L${x} ${base - rr() * amp}`;
    return `${d} L${W} ${H} Z`;
  };
  const seed = Math.floor(r() * 1e6);
  // An "edit" is the same scene, graded: punchier, warmer, with a vignette.
  const grade = edit
    ? mono
      ? '<filter id="e"><feComponentTransfer><feFuncR type="linear" slope="1.22" intercept="-.06"/><feFuncG type="linear" slope="1.22" intercept="-.06"/><feFuncB type="linear" slope="1.22" intercept="-.06"/></feComponentTransfer></filter>'
      : '<filter id="e"><feColorMatrix type="saturate" values="1.5"/><feComponentTransfer><feFuncR type="linear" slope="1.2" intercept="-.03"/><feFuncG type="linear" slope="1.12" intercept="-.06"/><feFuncB type="linear" slope="1" intercept="-.08"/></feComponentTransfer></filter>'
    : '';
  const vignette = edit
    ? `<radialGradient id="v" cx=".5" cy=".5" r=".75"><stop offset=".55" stop-color="#000" stop-opacity="0"/><stop offset="1" stop-color="#000" stop-opacity=".55"/></radialGradient>`
    : '';
  const scene = `<rect width="${W}" height="${H}" fill="url(#s)"/><circle cx="${sunX}" cy="${sunY}" r="${H * 0.35}" fill="url(#g)"/><circle cx="${sunX}" cy="${sunY}" r="${18 + r() * 26}" fill="${pal[4]}" opacity=".85"/><path d="${hill(H * 0.62, H * 0.22, seed)}" fill="${pal[2]}" opacity=".75"/><path d="${hill(H * 0.74, H * 0.16, seed + 1)}" fill="${pal[3]}" opacity=".9"/><path d="${hill(H * 0.88, H * 0.08, seed + 2)}" fill="${pal[3]}"/>`;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid slice"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${sky[0]}"/><stop offset="1" stop-color="${sky[1]}"/></linearGradient><radialGradient id="g"><stop offset="0" stop-color="#fff" stop-opacity=".9"/><stop offset="1" stop-color="${pal[1]}" stop-opacity="0"/></radialGradient>${grade}${vignette}</defs>${edit ? `<g filter="url(#e)">${scene}</g><rect width="${W}" height="${H}" fill="url(#v)"/>` : scene}</svg>`;
  const url = `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
  svgCache.set(p.id, url);
  return url;
}

// ---- the mock API ------------------------------------------------------------

export function mockApi(): Api {
  const empty = new URLSearchParams(location.search).has('empty');
  // `?days=3000` builds a ~50k photo library for performance testing.
  const days = Number(new URLSearchParams(location.search).get('days')) || 140;
  let photos = empty ? [] : buildLibrary(days);
  // `?offline` shows the library as if the Shield were switched off.
  const offline = new URLSearchParams(location.search).has('offline');
  let sources: Source[] = empty ? [] : [{ id: 1, path: '\\\\SHIELD\\Samsung_T7\\Photos', label: 'Shield · Samsung T7', count: photos.length, offline }];
  let settings: Settings = {
    inboxes: [{ path: 'C:\\Users\\me\\Pictures\\FUJIFILM PC AutoSave', cleanup: false }],
    importTo: '\\\\SHIELD\\Samsung_T7\\Photos',
    backupTo: 'E:\\Fujicloud Backup',
    parallelCopies: 4,
    writeXmp: true,
    editorPath: null,
    editRaw: true,
    smartSearch: false,
    smartGpu: true,
    faces: true,
    pets: true,
    cacheVersion: 0,
  };
  let backup: BackupStatus = {
    target: settings.backupTo,
    lastBackedUp: Date.now() / 1000 - 2.2 * 3600,
    running: false,
    done: 0,
    total: 0,
    error: offline ? "Shield · Samsung T7 can't be reached, so it wasn't backed up" : null,
  };
  const recipeNames = new Map<string, string>([[recipeKey(RECIPES[0]), 'Kodachrome 64-ish']]);
  // People: a few named, a few groups waiting for a name.
  // Searched as the app does: no movies, no RAW behind a JPEG.
  const shown = photos.filter((p) => p.kind !== 'raf' && p.kind !== 'video' && p.editOf == null);
  let people: Person[] = empty
    ? []
    : [
        ['Ana', 7],
        ['Kenji', 11],
        ['Mum', 13],
        [null, 17],
        [null, 29],
        ['Sam', 19],
        [null, 41],
      ].map(([name, every], i) => ({
        id: i + 1,
        name: name as string | null,
        cover: (i + 1) * 1000,
        faces: 0,
        photoIds: shown.filter((p) => p.id % (every as number) === 0).slice(0, 60 - i * 6).map((p) => p.id),
        hidden: false, takenOut: false,
        kind: 'person' as const,
      }));
  // Three cats: one named, two waiting for a name.
  if (!empty)
    people.push(
      ...(
        [
          ['Mochi', 23],
          [null, 31],
          [null, 43],
        ] as const
      ).map(([name, every], i) => ({
        id: 21 + i,
        name,
        cover: (21 + i) * 1000,
        faces: 0,
        photoIds: shown.filter((p) => p.id % every === 1).slice(0, 40 - i * 8).map((p) => p.id),
        hidden: false, takenOut: false,
        kind: 'pet' as const,
      })),
    );
  people = people.map((p) => ({ ...p, faces: p.photoIds.length }));
  let pets: PetsStatus = { enabled: true, phase: 'ready', done: 0, total: 0, downloadMb: 352, scanned: photos.length, error: null };
  const hiddenFaces = new Set<number>();
  let faces: FacesStatus = { enabled: true, phase: 'ready', done: 0, total: 0, downloadMb: 183, scanned: photos.length, error: null };
  const avatar = (seed: number) => {
    const hue = (seed * 47) % 360;
    // A cat: a round face with ears.
    const cat = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' fill='hsl(${hue} 30% 30%)'/><path d='M24 30 30 6l20 18 20-18 6 24' fill='hsl(${hue} 45% 70%)'/><circle cx='50' cy='52' r='30' fill='hsl(${hue} 45% 70%)'/><circle cx='39' cy='48' r='4' fill='#222'/><circle cx='61' cy='48' r='4' fill='#222'/><path d='M46 60h8l-4 5z' fill='#c66'/></svg>`;
    if (seed >= 21 && seed < 40) return `data:image/svg+xml;utf8,${encodeURIComponent(cat)}`;
    const svg = `<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><rect width='100' height='100' fill='hsl(${hue} 35% 38%)'/><circle cx='50' cy='42' r='20' fill='hsl(${hue} 40% 78%)'/><ellipse cx='50' cy='96' rx='34' ry='26' fill='hsl(${hue} 40% 78%)'/></svg>`;
    return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
  };

  let smart: SmartStatus = {
    enabled: false,
    phase: 'off',
    done: 0,
    total: 0,
    indexed: 0,
    downloadMb: 300,
    error: null,
    gpu: { name: 'NVIDIA GeForce RTX 4070 Laptop GPU', vramMb: 8188 },
    useGpu: true,
    runningOn: null,
    gpuError: null,
  };
  let albums: Album[] = empty
    ? []
    : [
        { id: 1, name: 'Best of the year', createdAt: Date.now() / 1000, ids: photos.filter((p) => p.rating === 5).slice(0, 40).map((p) => p.id) },
        { id: 2, name: 'Iceland', createdAt: Date.now() / 1000, ids: photos.filter((p) => p.country === 'IS').slice(0, 60).map((p) => p.id) },
      ];
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach((cb) => cb(payload));
  const delay = (ms = 0) => new Promise((r) => setTimeout(r, ms));
  const withPairs = (ids: number[]) => {
    const set = new Set(ids);
    for (const p of photos) if (set.has(p.id) && p.pairId) set.add(p.pairId);
    return set;
  };
  const update = (ids: number[], patch: Partial<Photo>) => {
    const set = withPairs(ids);
    photos = photos.map((p) => (set.has(p.id) ? { ...p, ...patch } : p));
    return [...set];
  };

  setTimeout(
    () =>
      emit('sync', {
        active: true,
        inboxes: settings.inboxes.map((i) => i.path),
        library: settings.importTo,
        copying: 0,
        done: 0,
        total: 0,
        lastImported: 0,
        waiting: offline ? 3 : 0,
        error: null,
      }),
    300,
  );

  return {
    native: false,
    bootstrap: async (): Promise<Bootstrap> => ({ version: '0.2.0', sources, settings, effects: false, editor: 'C:\\Program Files\\Adobe\\Adobe Photoshop 2026\\Photoshop.exe', smart, backup, faces, pets }),
    listPhotos: async () => photos,
    listSources: async () => sources,
    async photoDetail(id) {
      await delay(30);
      const p = photos.find((x) => x.id === id)!;
      const detail: Detail = {
        ...p,
        path: `${p.folder}\\${p.name}`,
        orientation: 1,
        exposureComp: [0, 0, 0, -0.3, 0.3, 0.7, -1][id % 7],
        altitude: p.lat != null ? 40 + (id % 300) : null,
        recipe: p.recipeKey ? (recipeByKey.get(p.recipeKey) ?? null) : p.filmSim ? { ...DEFAULT_RECIPE, filmSim: p.filmSim } : null,
        exif: [
          ['Make', 'FUJIFILM'],
          ['Model', p.camera ?? ''],
          ['LensModel', p.lens ?? ''],
          ['ExposureProgram', 'aperture priority'],
          ['MeteringMode', 'pattern'],
          ['WhiteBalance', 'auto'],
          ['ColorSpace', 'sRGB'],
          ['Software', 'Digital Camera X-T5 Ver4.10'],
        ],
      };
      return detail;
    },
    async addSource(path) {
      const s = { id: sources.length + 1, path, label: path.split(/[\\/]/).filter(Boolean).pop() ?? path, count: 0 };
      sources = [...sources, s];
      return s;
    },
    async removeSource(id) {
      sources = sources.filter((s) => s.id !== id);
      photos = photos.filter((p) => p.sourceId !== id);
      emit('library-changed', null);
    },
    async renameSource(id, label) {
      sources = sources.map((s) => (s.id === id ? { ...s, label } : s));
    },
    async rescan() {
      for (let i = 0; i <= 10; i++) {
        await delay(120);
        emit('scan', [sources[0]?.label ?? 'Library', { sourceId: 1, phase: i === 10 ? 'done' : 'reading', done: i * 100, total: 1000 }]);
      }
      emit('scan-done', null);
    },
    setRating: async (ids, rating) => update(ids, { rating }),
    setFlag: async (ids, flag) => update(ids, { flag }),
    setLabel: async (ids, label) => update(ids, { label }),
    setFavorite: async (ids, favorite) => update(ids, { favorite }),
    listAlbums: async () => albums,
    async createAlbum(name, ids) {
      const existing = albums.find((a) => a.name.toLowerCase() === name.trim().toLowerCase());
      const album = existing ?? { id: Math.max(0, ...albums.map((a) => a.id)) + 1, name: name.trim(), createdAt: Date.now() / 1000, ids: [] };
      album.ids = [...new Set([...album.ids, ...withPairs(ids)])];
      albums = existing ? albums.map((a) => (a.id === album.id ? album : a)) : [...albums, album];
      return album.id;
    },
    async renameAlbum(id, name) {
      albums = albums.map((a) => (a.id === id ? { ...a, name } : a));
    },
    async deleteAlbum(id) {
      albums = albums.filter((a) => a.id !== id);
    },
    async addToAlbum(id, ids) {
      albums = albums.map((a) => (a.id === id ? { ...a, ids: [...new Set([...a.ids, ...withPairs(ids)])] } : a));
    },
    async removeFromAlbum(id, ids) {
      const gone = withPairs(ids);
      albums = albums.map((a) => (a.id === id ? { ...a, ids: a.ids.filter((x) => !gone.has(x)) } : a));
    },
    facesStatus: async () => faces,
    async setFaces(enabled) {
      settings = { ...settings, faces: enabled };
      faces = { ...faces, enabled, phase: enabled ? 'ready' : 'off' };
      return faces;
    },
    petsStatus: async () => pets,
    async setPets(enabled) {
      settings = { ...settings, pets: enabled };
      pets = { ...pets, enabled, phase: enabled ? 'ready' : 'off' };
      return pets;
    },
    listPeople: async () => people,
    async facesIn(photoId) {
      const found: FaceInPhoto[] = people
        .filter((p) => !p.hidden && p.photoIds.includes(photoId))
        .map((p, i) => ({ id: p.cover + photoId, personId: p.id, name: p.name, bbox: [0.2 + i * 0.3, 0.2, 0.15, 0.2], kind: p.kind }));
      if (photoId % 5 === 0 && !hiddenFaces.has(90000 + photoId))
        found.push({ id: 90000 + photoId, personId: null, name: null, bbox: [0.7, 0.3, 0.1, 0.14], kind: 'person' });
      return found;
    },
    async namePerson(id, name) {
      const same = people.find((p) => p.id !== id && p.name?.toLowerCase() === name.trim().toLowerCase());
      if (same) {
        const gone = people.find((p) => p.id === id)!;
        people = people.filter((p) => p.id !== id).map((p) => (p === same ? { ...p, photoIds: [...new Set([...p.photoIds, ...gone.photoIds])] } : p));
        return same.id;
      }
      people = people.map((p) => (p.id === id ? { ...p, name: name.trim(), hidden: false } : p));
      return id;
    },
    async nameFace(faceId, name) {
      const photoId = faceId >= 90000 ? faceId - 90000 : faceId % 1000;
      const kind = people.find((p) => faceId > p.cover && faceId - p.cover < 1000)?.kind ?? 'person';
      let person = people.find((p) => p.kind === kind && p.name?.toLowerCase() === name.trim().toLowerCase());
      if (!person) {
        person = { id: people.length + 100, name: name.trim(), cover: faceId, faces: 0, photoIds: [], hidden: false, kind, takenOut: false };
        people = [...people, person];
      }
      people = people.map((p) => (p.id === person!.id ? { ...p, photoIds: [...p.photoIds, photoId], faces: p.faces + 1 } : p));
      return person.id;
    },
    async removeFromPerson(id, photoIds) {
      const from = people.find((p) => p.id === id)!;
      people = people.map((p) => (p.id === id ? { ...p, photoIds: p.photoIds.filter((x) => !photoIds.includes(x)) } : p));
      // As the app does: each taken out is asked about on its own.
      let next = Math.max(...people.map((p) => p.id)) + 1;
      for (const photo of photoIds) {
        people = [...people, { id: next++, name: null, cover: from.cover, faces: 1, photoIds: [photo], hidden: false, kind: from.kind, takenOut: true }];
      }
    },
    async moveToPerson(from, photoIds, to) {
      const source = people.find((p) => p.id === from)!;
      let target = 'id' in to ? people.find((p) => p.id === to.id) : people.find((p) => p.name === to.name && p.kind === source.kind);
      if (!target) {
        target = { id: Math.max(...people.map((p) => p.id)) + 1, name: 'name' in to ? to.name : null, cover: source.cover, faces: 0, photoIds: [], hidden: false, kind: source.kind, takenOut: false };
        people = [...people, target];
      }
      const id = target.id;
      people = people
        .map((p) =>
          p.id === from
            ? { ...p, photoIds: p.photoIds.filter((x) => !photoIds.includes(x)) }
            : p.id === id
              ? { ...p, photoIds: [...new Set([...p.photoIds, ...photoIds])], hidden: false }
              : p,
        )
        .filter((p) => p.name || p.photoIds.length);
      return id;
    },
    async mergePeople(from, into) {
      const target = people.find((p) => p.id === into)!;
      const merged = people.filter((p) => from.includes(p.id) && p.id !== into);
      const name = target.name ?? merged.find((p) => p.name)?.name ?? null;
      const photoIds = [...new Set([...target.photoIds, ...merged.flatMap((p) => p.photoIds)])];
      people = people
        .filter((p) => !merged.includes(p))
        .map((p) => (p.id === into ? { ...p, name, photoIds, faces: photoIds.length } : p));
      return into;
    },
    async deletePerson(id) {
      people = people.map((p) => (p.id === id ? { ...p, name: null } : p));
    },
    async regroupPeople(kind) {
      await delay(500);
      return people.filter((p) => p.kind === kind && !p.name).length ? 1 : 0;
    },
    async reidentify(kind, forgetNames) {
      await delay(200);
      if (forgetNames) people = people.map((p) => (p.kind === kind ? { ...p, name: null } : p));
    },
    async hidePeople(ids, hidden) {
      people = people.map((p) => (ids.includes(p.id) ? { ...p, hidden } : p));
    },
    async hideFace(faceId) {
      const group = people.find((p) => !p.name && faceId > p.cover && faceId - p.cover < 1000);
      if (group) {
        people = people.map((p) => (p === group ? { ...p, hidden: true } : p));
        return group.id;
      }
      hiddenFaces.add(faceId);
      const id = 500 + hiddenFaces.size;
      people = [...people, { id, name: null, cover: faceId, faces: 1, photoIds: [faceId - 90000], hidden: true, kind: 'person', takenOut: false }];
      return id;
    },
    faceUrl: (id) => avatar(Math.floor(id / 1000) || id),
    async findDuplicates() {
      await delay(500);
      // Bursts: shots of one outing a few seconds apart, best rated first.
      const shown = photos.filter((p) => p.kind !== 'raf' && p.editOf == null && p.flag !== -1);
      const groups = [];
      for (let i = 0; i + 2 < shown.length && groups.length < 12; i += 37) {
        const ids = shown.slice(i, i + 2 + (i % 2)).sort((a, b) => b.rating - a.rating).map((p) => p.id);
        groups.push({ ids, exact: i % 3 === 0 });
      }
      return groups;
    },
    async deletePhotos(ids) {
      const gone = withPairs(ids);
      photos = photos.filter((p) => !gone.has(p.id));
      albums = albums.map((a) => ({ ...a, ids: a.ids.filter((x) => !gone.has(x)) }));
      return { ids: [...gone], places: ['the Recycle Bin'], failed: [] };
    },
    async exportPhotos(ids) {
      for (let i = 1; i <= ids.length; i++) {
        await delay(60);
        emit('export', { done: i, total: ids.length });
      }
      return { files: ids.length, failed: [] };
    },
    dragOut: async () => {},
    async prepareMovie(id) {
      for (let i = 1; i <= 10; i++) {
        await delay(120);
        emit('movie-progress', { id, progress: i / 10 });
      }
      throw new Error('no converter in the browser preview');
    },
    convertedUrl: (p) => sceneFor(p),
    openFolder: async () => {},
    openInEditor: async () => 'Photoshop',
    reveal: async () => {},
    openDefault: async () => {},
    async saveSettings(s) {
      settings = s;
    },
    backupStatus: async () => backup,
    async backupNow() {
      backup = { ...backup, running: true, done: 0, total: 40, error: null };
      emit('backup', backup);
      for (let i = 1; i <= 40; i += 8) {
        await delay(120);
        backup = { ...backup, done: i };
        emit('backup', backup);
      }
      backup = { ...backup, running: false, done: 40, lastBackedUp: Date.now() / 1000 };
      emit('backup', backup);
    },
    suggestInboxes: async () => ['C:\\Users\\me\\Pictures\\FUJIFILM PC AutoSave', 'C:\\Users\\me\\Pictures\\iCloud Photos\\Photos'],
    async listRecipes(): Promise<RecipeRow[]> {
      const counts = new Map<string, number>();
      for (const p of photos) if (p.recipeKey && p.kind !== 'raf' && p.flag !== -1) counts.set(p.recipeKey, (counts.get(p.recipeKey) ?? 0) + 1);
      return [...counts]
        .sort((a, b) => b[1] - a[1])
        .map(([key, count]) => ({ key, count, name: recipeNames.get(key) ?? null, recipe: recipeByKey.get(key)! }));
    },
    async nameRecipe(key, name) {
      if (name?.trim()) recipeNames.set(key, name.trim());
      else recipeNames.delete(key);
    },
    smartStatus: async () => smart,
    async setSmartSearch(enabled) {
      if (!enabled) {
        smart = { ...smart, enabled: false, phase: 'off' };
        return smart;
      }
      // Simulate the one-time download, then indexing.
      const total = photos.filter((p) => p.kind !== 'raf').length;
      smart = { ...smart, enabled: true, phase: 'downloading', done: 0, total: 210e6 };
      const step = (i: number) => {
        if (!smart.enabled) return;
        if (i <= 10) smart = { ...smart, phase: 'downloading', done: i * 21e6, total: 210e6 };
        else if (i <= 20) smart = { ...smart, phase: 'indexing', done: Math.round(((i - 10) / 10) * total), total, indexed: Math.round(((i - 10) / 10) * total) };
        else smart = { ...smart, phase: 'ready', done: total, total };
        if (i > 10) smart = { ...smart, runningOn: smart.useGpu ? 'GPU' : 'CPU' };
        emit('smart', smart);
        if (i <= 20) setTimeout(() => step(i + 1), 120);
      };
      setTimeout(() => step(1), 50);
      return smart;
    },
    async setSmartGpu(enabled) {
      smart = { ...smart, useGpu: enabled, runningOn: smart.runningOn ? (enabled ? 'GPU' : 'CPU') : null, downloadMb: enabled ? 300 : 210 };
      return smart;
    },
    async searchSmart(query): Promise<Hit[]> {
      await delay(60);
      const q = query.toLowerCase();
      const want = {
        golden: /sunset|golden|warm|orange|evening glow/.test(q),
        dusk: /dusk|night|blue hour|evening|dark/.test(q),
        day: /day|sky|bright|blue sky|sunny/.test(q),
        land: /mountain|landscape|hill|nature|sun/.test(q),
      };
      const town = PLACES.find((pl) => q.includes(pl.town.toLowerCase()) || q.includes(pl.country.toLowerCase()));
      return photos
        .filter((p) => p.kind !== 'raf')
        .map((p) => {
          const light = lightOf(p);
          let score = 0;
          if (want[light]) score += 0.12;
          if (want.land) score += 0.04;
          if (town && p.town === town.town) score += 0.08;
          return { id: p.id, score: score + ((p.id * 7919) % 100) / 5000 };
        })
        .filter((h) => h.score > 0.05)
        .sort((a, b) => b.score - a.score)
        .slice(0, 400);
    },
    async similarPhotos(id): Promise<Hit[]> {
      const me = photos.find((p) => p.id === id)!;
      return photos
        .filter((p) => p.id !== id && p.kind !== 'raf' && lightOf(p) === lightOf(me) && p.filmSim === me.filmSim)
        .map((p) => ({ id: p.id, score: 0.9 - Math.abs((p.takenAt ?? 0) - (me.takenAt ?? 0)) / 1e9 }))
        .sort((a, b) => b.score - a.score)
        .slice(0, 120);
    },
    cacheInfo: async () => ({ bytes: 1.84e9, path: 'C:\\Users\\me\\AppData\\Local\\app.fujicloud.desktop\\cache', backlog: 0 }),
    async rebuildThumbnails() {
      settings = { ...settings, cacheVersion: settings.cacheVersion + 1 };
      return settings.cacheVersion;
    },
    openLogFolder: async () => {},
    pickFolder: async () => window.prompt('Folder path (mock)', '\\\\SHIELD\\Samsung_T7\\Photos'),
    async listen(event, cb) {
      const set = listeners.get(event) ?? new Set();
      set.add(cb as (p: unknown) => void);
      listeners.set(event, set);
      return () => set.delete(cb as (p: unknown) => void);
    },
    onDropPaths: async () => () => {},
    async setFullscreen(on) {
      if (on) await document.documentElement.requestFullscreen?.();
      else if (document.fullscreenElement) await document.exitFullscreen();
    },
    isFullscreen: async () => !!document.fullscreenElement,
    thumbUrl: sceneFor,
    previewUrl: sceneFor,
    originalUrl: sceneFor,
  };
}
