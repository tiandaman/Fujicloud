// A believable fake library so the UI can be developed and demoed in a plain
// browser (`npm run dev`) without the Rust backend or a camera.

import type { Api } from './api';
import type { Bootstrap, Detail, Kind, Label, Photo, Settings, Source } from './types';

const FILMS = ['Classic Chrome', 'Classic Neg.', 'Provia', 'Velvia', 'Acros', 'Nostalgic Neg.', 'Reala Ace', 'Eterna', 'Astia', 'Pro Neg. Hi', 'Acros +R', 'Monochrome'];
const FILM_WEIGHTS = [18, 16, 12, 9, 8, 7, 7, 5, 5, 4, 3, 2];
const BODIES = [
  { camera: 'X-T5', lenses: [['XF23mmF1.4 R LM WR', 23, 1.4], ['XF56mmF1.2 R WR', 56, 1.2], ['XF16-55mmF2.8 R LM WR', 0, 2.8], ['XF70-300mmF4-5.6 R LM OIS WR', 0, 4]] },
  { camera: 'X100VI', lenses: [['', 23, 2]] },
  { camera: 'X-H2', lenses: [['XF33mmF1.4 R LM WR', 33, 1.4], ['XF8-16mmF2.8 R LM WR', 0, 2.8]] },
] as const;

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
    const film = weighted(r, FILMS, FILM_WEIGHTS);
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
      };
      const kind: Kind = r() < 0.04 ? 'heif' : 'jpeg';
      const jpg: Photo = { ...base, id: id++, name: `${name}.${kind === 'heif' ? 'HIF' : 'JPG'}`, kind, size: 9e6 + r() * 9e6, mtime: Math.round(t), pairId: null };
      photos.push(jpg);
      if (rawJpeg && kind === 'jpeg') {
        const raf: Photo = { ...base, id: id++, name: `${name}.RAF`, kind: 'raf', size: 3.2e7 + r() * 2e7, mtime: Math.round(t), pairId: jpg.id };
        jpg.pairId = raf.id;
        photos.push(raf);
      }
      t -= 20 + r() * 400;
    }
    t -= 86400 * (1 + r() * 8);
  }
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
  const r = rng(p.pairId ? Math.min(p.id, p.pairId) * 7919 : p.id * 7919);
  const mono = !!p.filmSim && (p.filmSim.startsWith('Acros') || p.filmSim.startsWith('Mono'));
  const pal = mono ? ['#9a9a9a', '#e6e6e6', '#4a4a4a', '#1c1c1c', '#7a7a7a'] : (palettes[p.filmSim ?? ''] ?? palettes.Provia)[0];
  // Vary the light a little so a day's shots don't all look the same.
  const light = r();
  const sky = mono ? [pal[0], pal[1]] : light > 0.78 ? ['#f2a65a', '#fbd7a1'] : light > 0.64 ? ['#22304a', '#6b7aa1'] : [pal[0], pal[1]];
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
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${W} ${H}" preserveAspectRatio="xMidYMid slice"><defs><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${sky[0]}"/><stop offset="1" stop-color="${sky[1]}"/></linearGradient><radialGradient id="g"><stop offset="0" stop-color="#fff" stop-opacity=".9"/><stop offset="1" stop-color="${pal[1]}" stop-opacity="0"/></radialGradient></defs><rect width="${W}" height="${H}" fill="url(#s)"/><circle cx="${sunX}" cy="${sunY}" r="${H * 0.35}" fill="url(#g)"/><circle cx="${sunX}" cy="${sunY}" r="${18 + r() * 26}" fill="${pal[4]}" opacity=".85"/><path d="${hill(H * 0.62, H * 0.22, seed)}" fill="${pal[2]}" opacity=".75"/><path d="${hill(H * 0.74, H * 0.16, seed + 1)}" fill="${pal[3]}" opacity=".9"/><path d="${hill(H * 0.88, H * 0.08, seed + 2)}" fill="${pal[3]}"/></svg>`;
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
  let sources: Source[] = empty ? [] : [{ id: 1, path: '\\\\SHIELD\\Samsung_T7\\Photos', label: 'Shield · Samsung T7', count: photos.length }];
  let settings: Settings = {
    inbox: 'C:\\Users\\me\\Pictures\\FUJIFILM PC AutoSave',
    importTo: '\\\\SHIELD\\Samsung_T7\\Photos',
    parallelCopies: 4,
    writeXmp: true,
    editorPath: null,
    editRaw: true,
  };
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

  setTimeout(() => emit('sync', { active: true, inbox: settings.inbox, library: settings.importTo, copying: 0, done: 0, total: 0, lastImported: 0 }), 300);

  return {
    native: false,
    bootstrap: async (): Promise<Bootstrap> => ({ sources, settings, effects: false, editor: 'C:\\Program Files\\Adobe\\Adobe Photoshop 2026\\Photoshop.exe' }),
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
        grain: ['Off', 'Weak', 'Strong'][id % 3],
        dynamicRange: ['DR100', 'DR200', 'DR400', 'DR Auto'][id % 4],
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
    openInEditor: async () => 'Photoshop',
    reveal: async () => {},
    openDefault: async () => {},
    async saveSettings(s) {
      settings = s;
    },
    cacheInfo: async () => ({ bytes: 1.84e9, path: 'C:\\Users\\me\\AppData\\Local\\app.fujicloud.desktop\\cache', backlog: 0 }),
    clearCache: async () => {},
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
