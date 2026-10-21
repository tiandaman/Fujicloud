import { api } from './api';
import { formatDay, formatMonth, plural } from './format';
import type { Collection, GroupKey, Kind, Label, LayoutMode, Photo, ScanProgress, Section, Settings, SortKey, Source, SyncStatus } from './types';

export interface Toast {
  id: number;
  text: string;
  tone: 'info' | 'error';
}

/** What the grid exposes so the viewer can animate to and from tiles. */
export interface GridHandle {
  rectOf(id: number): DOMRect | null;
  reveal(id: number): void;
}

const PREFS_KEY = 'fujicloud.prefs.v1';
const ALL_KINDS: Kind[] = ['jpeg', 'raf', 'heif', 'tiff'];

const sortKeys: Record<SortKey, (p: Photo) => number | string | null> = {
  taken: (p) => p.takenAt,
  added: (p) => p.addedAt,
  name: (p) => p.name.toLowerCase(),
  size: (p) => p.size,
  rating: (p) => p.rating,
  film: (p) => p.filmSim,
  camera: (p) => p.camera,
  focal: (p) => p.focalLength,
  aperture: (p) => p.aperture,
  shutter: (p) => p.shutter,
  iso: (p) => p.iso,
};

function loadPrefs(): Record<string, unknown> {
  try {
    return JSON.parse(localStorage.getItem(PREFS_KEY) ?? '{}');
  } catch {
    return {};
  }
}

class Store {
  // ---- data from the backend ----
  photos = $state.raw<Photo[]>([]);
  sources = $state.raw<Source[]>([]);
  settings = $state.raw<Settings | null>(null);
  editor = $state<string | null>(null);
  effects = $state(false);
  ready = $state(false);

  // ---- view state ----
  collection = $state.raw<Collection>({ type: 'all' });
  query = $state('');
  sort = $state<SortKey>('taken');
  desc = $state(true);
  group = $state<GroupKey>('day');
  mode = $state<LayoutMode>('justified');
  size = $state(220);
  kinds = $state.raw<Kind[]>(ALL_KINDS);
  minRating = $state(0);
  /** Show RAW+JPEG pairs as one tile. */
  stack = $state(true);
  showInfo = $state(false);
  sidebar = $state(true);

  selection = $state.raw<Set<number>>(new Set());
  focusId = $state<number | null>(null);
  anchorId: number | null = null;
  viewerId = $state<number | null>(null);
  /** Tile hidden while the viewer animates out of / back into it. */
  hiddenId = $state<number | null>(null);
  settingsOpen = $state(false);

  // ---- background activity ----
  scan = $state.raw<{ label: string; progress: ScanProgress } | null>(null);
  warm = $state.raw<{ done: number; total: number } | null>(null);
  sync = $state.raw<SyncStatus | null>(null);
  toasts = $state.raw<Toast[]>([]);

  grid: GridHandle | null = null;
  #toastId = 0;

  constructor() {
    const prefs = loadPrefs();
    for (const key of ['sort', 'desc', 'group', 'mode', 'size', 'stack', 'showInfo', 'sidebar'] as const) {
      if (key in prefs) (this as Record<string, unknown>)[key] = prefs[key];
    }
    $effect.root(() => {
      $effect(() => {
        const prefs = { sort: this.sort, desc: this.desc, group: this.group, mode: this.mode, size: this.size, stack: this.stack, showInfo: this.showInfo, sidebar: this.sidebar };
        try {
          localStorage.setItem(PREFS_KEY, JSON.stringify(prefs));
        } catch {
          // Storage can be unavailable; preferences just won't persist.
        }
      });
    });
  }

  // ---- derived views ----

  byId = $derived(new Map(this.photos.map((p) => [p.id, p])));

  /** Photos in the current collection, before toolbar filters (for counts). */
  inCollection = $derived.by(() => {
    const c = this.collection;
    const recent = Date.now() / 1000 - 7 * 86400;
    const test: (p: Photo) => boolean =
      c.type === 'all' ? (p) => p.flag !== -1
      : c.type === 'picks' ? (p) => p.flag === 1
      : c.type === 'rated' ? (p) => p.rating > 0
      : c.type === 'rejected' ? (p) => p.flag === -1
      : c.type === 'recent' ? (p) => p.addedAt >= recent
      : c.type === 'source' ? (p) => p.sourceId === c.id
      : c.type === 'film' ? (p) => p.filmSim === c.value
      : c.type === 'camera' ? (p) => p.camera === c.value
      : (p) => p.lens === c.value;
    return this.photos.filter(test);
  });

  visible = $derived.by(() => {
    const kinds = new Set(this.kinds);
    const q = this.query.trim().toLowerCase();
    const byId = this.byId;
    let list = this.inCollection.filter((p) => {
      if (!kinds.has(p.kind)) return false;
      if (p.rating < this.minRating) return false;
      // A RAF whose JPEG twin is shown is folded into that tile.
      if (this.stack && p.kind === 'raf' && p.pairId != null && byId.get(p.pairId)?.kind === 'jpeg' && kinds.has('jpeg')) return false;
      if (q) {
        const hay = `${p.name} ${p.camera ?? ''} ${p.lens ?? ''} ${p.filmSim ?? ''} ${p.folder}`.toLowerCase();
        if (!q.split(/\s+/).every((t) => hay.includes(t))) return false;
      }
      return true;
    });
    const key = sortKeys[this.sort];
    const dir = this.desc ? -1 : 1;
    list = list
      .map((p) => ({ p, k: key(p) }))
      .sort((a, b) => {
        // Missing values always sink to the end.
        if (a.k == null || b.k == null) return a.k == null ? (b.k == null ? 0 : 1) : -1;
        if (a.k < b.k) return -dir;
        if (a.k > b.k) return dir;
        return dir * ((a.p.takenAt ?? 0) - (b.p.takenAt ?? 0)) || a.p.id - b.p.id;
      })
      .map((x) => x.p);
    return list;
  });

  sections = $derived.by((): Section[] => {
    const list = this.visible;
    const g = this.group;
    if (g === 'none') return [{ key: 'all', title: '', subtitle: '', items: list }];
    const keyOf = (p: Photo): [string, string] => {
      const t = p.takenAt ?? 0;
      switch (g) {
        case 'day': return [String(Math.floor(t / 86400)), formatDay(t)];
        case 'month': {
          const d = new Date(t * 1000);
          return [`${d.getUTCFullYear()}-${d.getUTCMonth()}`, formatMonth(t)];
        }
        case 'year': return [String(new Date(t * 1000).getUTCFullYear()), String(new Date(t * 1000).getUTCFullYear())];
        case 'film': return [p.filmSim ?? '', p.filmSim ?? 'Unknown film simulation'];
        case 'camera': return [p.camera ?? '', p.camera ?? 'Unknown camera'];
        case 'lens': return [p.lens ?? '', p.lens ?? (p.camera?.startsWith('X100') ? 'Fixed 23mm' : 'Unknown lens')];
        case 'folder': return [p.folder, p.folder.split(/[\\/]/).filter(Boolean).slice(-2).join(' / ')];
      }
    };
    // Groups appear in sort order: the first photo of a key decides its spot.
    const map = new Map<string, Section>();
    for (const p of list) {
      const [key, title] = keyOf(p);
      let s = map.get(key);
      if (!s) {
        s = { key, title, subtitle: '', items: [] };
        map.set(key, s);
      }
      s.items.push(p);
    }
    const sections = [...map.values()];
    for (const s of sections) {
      const films = new Set(s.items.map((p) => p.filmSim).filter(Boolean));
      const extra = g === 'day' || g === 'month' ? (films.size === 1 ? ` · ${[...films][0]}` : films.size > 1 ? ` · ${films.size} film sims` : '') : '';
      s.subtitle = plural(s.items.length, 'photo') + extra;
    }
    return sections;
  });

  /** Grid order, which the viewer and filmstrip navigate through. */
  ordered = $derived(this.sections.length === 1 ? this.sections[0].items : this.sections.flatMap((s) => s.items));

  facets = $derived.by(() => {
    const count = (key: (p: Photo) => string | null) => {
      const m = new Map<string, number>();
      for (const p of this.photos) {
        if (p.flag === -1 || (this.stack && p.kind === 'raf' && p.pairId != null)) continue;
        const k = key(p);
        if (k) m.set(k, (m.get(k) ?? 0) + 1);
      }
      return [...m.entries()].sort((a, b) => b[1] - a[1]);
    };
    return { films: count((p) => p.filmSim), cameras: count((p) => p.camera), lenses: count((p) => p.lens) };
  });

  counts = $derived.by(() => {
    let picks = 0, rated = 0, rejected = 0, recent = 0, all = 0;
    const since = Date.now() / 1000 - 7 * 86400;
    for (const p of this.photos) {
      if (this.stack && p.kind === 'raf' && p.pairId != null) continue;
      if (p.flag === -1) rejected++;
      else all++;
      if (p.flag === 1) picks++;
      if (p.rating > 0) rated++;
      if (p.addedAt >= since) recent++;
    }
    return { all, picks, rated, rejected, recent };
  });

  collectionTitle = $derived.by(() => {
    const c = this.collection;
    switch (c.type) {
      case 'all': return 'All Photos';
      case 'picks': return 'Picks';
      case 'rated': return 'Rated';
      case 'rejected': return 'Rejected';
      case 'recent': return 'Recently Added';
      case 'source': return this.sources.find((s) => s.id === c.id)?.label ?? 'Folder';
      default: return c.value;
    }
  });

  /** Targets for rating/flag shortcuts: the selection, else the focused photo. */
  targets = $derived.by((): number[] => {
    if (this.viewerId != null) return [this.viewerId];
    if (this.selection.size) return [...this.selection];
    return this.focusId != null ? [this.focusId] : [];
  });

  // ---- loading ----

  async init() {
    const boot = await api.bootstrap();
    this.sources = boot.sources;
    this.settings = boot.settings;
    this.effects = boot.effects;
    this.editor = boot.editor;
    await this.refresh();
    this.ready = true;

    let refreshTimer: ReturnType<typeof setTimeout> | undefined;
    await api.listen('library-changed', () => {
      clearTimeout(refreshTimer);
      refreshTimer = setTimeout(() => this.refresh(), 250);
    });
    await api.listen<[string, ScanProgress]>('scan', ([label, progress]) => {
      this.scan = progress.phase === 'done' ? null : { label, progress };
    });
    await api.listen('scan-done', () => (this.scan = null));
    await api.listen<string>('scan-error', (msg) => this.toast(msg, 'error'));
    await api.listen<{ done: number; total: number }>('warm', (w) => (this.warm = w.done >= w.total ? null : w));
    await api.listen<SyncStatus>('sync', (s) => {
      const finished = this.sync?.copying && !s.copying && s.lastImported;
      this.sync = s;
      if (finished) this.toast(`Imported ${plural(s.lastImported!, 'new photo')} from your camera`);
      if (s.error) this.toast(s.error, 'error');
    });
    await api.onDropPaths((paths) => paths.forEach((p) => this.addSource(p)));
  }

  async refresh() {
    const [photos, sources] = await Promise.all([api.listPhotos(), api.listSources()]);
    this.photos = photos;
    this.sources = sources;
  }

  // ---- actions ----

  toast(text: string, tone: Toast['tone'] = 'info') {
    const t = { id: ++this.#toastId, text, tone };
    this.toasts = [...this.toasts.slice(-2), t];
    setTimeout(() => (this.toasts = this.toasts.filter((x) => x !== t)), tone === 'error' ? 6000 : 2600);
  }

  /** Optimistically patches photos (and their RAW+JPEG twins). */
  #patch(ids: number[], patch: Partial<Photo>) {
    const set = new Set(ids);
    for (const id of ids) {
      const twin = this.byId.get(id)?.pairId;
      if (twin != null) set.add(twin);
    }
    const before = this.photos;
    this.photos = this.photos.map((p) => (set.has(p.id) ? { ...p, ...patch } : p));
    return () => (this.photos = before);
  }

  async rate(rating: number, ids = this.targets) {
    if (!ids.length) return;
    const undo = this.#patch(ids, { rating });
    try {
      await api.setRating(ids, rating);
      this.toast(rating ? `${'★'.repeat(rating)}${ids.length > 1 ? ` · ${ids.length} photos` : ''}` : 'Rating cleared');
    } catch (e) {
      undo();
      this.toast(String(e), 'error');
    }
  }

  async flag(flag: number, ids = this.targets) {
    if (!ids.length) return;
    const undo = this.#patch(ids, { flag });
    try {
      await api.setFlag(ids, flag);
      this.toast(flag === 1 ? 'Marked as pick' : flag === -1 ? 'Rejected' : 'Flag cleared');
    } catch (e) {
      undo();
      this.toast(String(e), 'error');
    }
  }

  async label(label: Label | null, ids = this.targets) {
    if (!ids.length) return;
    const current = ids.length === 1 ? this.byId.get(ids[0])?.label : undefined;
    const next = current === label ? null : label;
    const undo = this.#patch(ids, { label: next });
    try {
      await api.setLabel(ids, next);
    } catch (e) {
      undo();
      this.toast(String(e), 'error');
    }
  }

  async edit(ids = this.targets) {
    if (!ids.length) return;
    try {
      const app = await api.openInEditor(ids);
      this.toast(`Opening ${ids.length > 1 ? plural(ids.length, 'photo') : ''} in ${app}`.replace('  ', ' '));
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  async addSource(path: string) {
    try {
      const s = await api.addSource(path);
      this.sources = await api.listSources();
      this.collection = { type: 'source', id: s.id };
      this.toast(`Added ${s.label}. Indexing…`);
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  async removeSource(id: number) {
    await api.removeSource(id);
    if (this.collection.type === 'source' && this.collection.id === id) this.collection = { type: 'all' };
    await this.refresh();
  }

  async saveSettings(s: Settings) {
    try {
      await api.saveSettings(s);
      this.settings = s;
      this.toast('Settings saved');
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  setCollection(c: Collection) {
    this.collection = c;
    this.selection = new Set();
  }

  open(id: number) {
    this.focusId = id;
    this.viewerId = id;
  }
}

export const store = new Store();
