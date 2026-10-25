import { api } from './api';
import { formatDay, formatMonth, plural } from './format';
import { countryName, placeName, recipeSummary, recipeTitle } from './recipe';
import { cacheVersion } from './version.svelte';
import type {
  Album,
  BackupStatus,
  Collection,
  DuplicateGroup,
  ExportPreset,
  GroupKey,
  Hit,
  Kind,
  Label,
  LayoutMode,
  Photo,
  RecipeRow,
  ScanProgress,
  Section,
  Settings,
  SmartStatus,
  SortKey,
  Source,
  SyncStatus,
} from './types';

export interface Toast {
  id: number;
  text: string;
  tone: 'info' | 'error';
  action?: { label: string; run: () => void };
}

/** What the grid exposes so the viewer can animate to and from tiles. */
export interface GridHandle {
  rectOf(id: number): DOMRect | null;
  reveal(id: number): void;
}

const PREFS_KEY = 'fujicloud.prefs.v1';
const ALL_KINDS: Kind[] = ['jpeg', 'raf', 'heif', 'tiff', 'psd', 'png', 'dng', 'video'];

/** Kinds that stand in for a pair when stacked (the picture you look at). */
const PRIMARY: Kind[] = ['jpeg', 'heif', 'png'];
/** Kinds folded under a primary twin: RAWs, and the movie of a Live Photo. */
const SECONDARY: Kind[] = ['raf', 'dng', 'video'];

/** Rating, pick/reject, colour label and favourite: what Ctrl+Z can undo. */
type Field = 'rating' | 'flag' | 'label' | 'favorite';
interface Change {
  field: Field;
  ids: number[];
  before: Map<number, Photo[Field]>;
  after: Photo[Field];
}

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
  recipes = $state.raw<RecipeRow[]>([]);
  albums = $state.raw<Album[]>([]);
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
  /** Some file types are hidden by the toolbar filter. */
  kindsFiltered = $derived(this.kinds.length < ALL_KINDS.length);
  minRating = $state(0);
  /** Show RAW+JPEG pairs as one tile. */
  stack = $state(true);
  showInfo = $state(false);
  sidebar = $state(true);

  selection = $state.raw<Set<number>>(new Set());
  focusId = $state<number | null>(null);
  anchorId: number | null = null;
  viewerId = $state<number | null>(null);
  /** Navigate the viewer through these instead of the grid (e.g. from the map). */
  viewerList = $state.raw<number[] | null>(null);
  /** Tile hidden while the viewer animates out of / back into it. */
  hiddenId = $state<number | null>(null);
  settingsOpen = $state(false);
  /** The "add to album" popover. */
  albumPicker = $state.raw<{ x: number; y: number; ids: number[] } | null>(null);
  /** Duplicate groups, once looked for. */
  duplicates = $state.raw<DuplicateGroup[] | null>(null);
  findingDuplicates = $state(false);
  /** The photo to keep in each duplicate group. */
  keepers = $derived(new Set((this.duplicates ?? []).map((g) => g.ids[0])));
  /** The export sheet, for these photos. */
  exportDialog = $state.raw<{ ids: number[] } | null>(null);
  /** Export progress while one runs. */
  exporting = $state.raw<{ done: number; total: number } | null>(null);
  /** A question waiting for an answer (see `ask`). */
  confirmation = $state.raw<{ title: string; body: string; action: string; resolve: (ok: boolean) => void } | null>(null);
  /** Before/after comparison, shown over everything else. */
  compare = $state.raw<{ before: number; after: number } | null>(null);
  compareMode = $state<'split' | 'slider'>('split');
  /** Where the map should fly to next. */
  mapFocus = $state.raw<{ lat: number; lon: number; zoom?: number } | null>(null);

  // ---- smart search ----
  smart = $state.raw<SmartStatus | null>(null);
  smartHits = $state.raw<{ query: string; scores: Map<number, number> } | null>(null);
  smartBusy = $state(false);
  similarHits = $state.raw<{ id: number; scores: Map<number, number> } | null>(null);

  // ---- background activity ----
  scan = $state.raw<{ label: string; progress: ScanProgress } | null>(null);
  warm = $state.raw<{ done: number; total: number } | null>(null);
  sync = $state.raw<SyncStatus | null>(null);
  toasts = $state.raw<Toast[]>([]);
  backup = $state.raw<BackupStatus | null>(null);
  /** Bumped when an offline source comes back, so missing thumbnails retry. */
  reachEpoch = $state(0);

  grid: GridHandle | null = null;
  #toastId = 0;
  #undo: Change[] = [];
  #redo: Change[] = [];

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

      // Typed searches also run through smart search, debounced.
      $effect(() => {
        const q = this.query.trim();
        const usable = this.smartReady;
        if (!usable || q.length < 3) {
          this.smartHits = null;
          return;
        }
        const t = setTimeout(() => this.#searchSmart(q), 280);
        return () => clearTimeout(t);
      });
    });
  }

  // ---- derived views ----

  byId = $derived(new Map(this.photos.map((p) => [p.id, p])));

  /** Original id → its edits, newest first. */
  editsOf = $derived.by(() => {
    const m = new Map<number, Photo[]>();
    for (const p of this.photos) {
      if (p.editOf == null) continue;
      const list = m.get(p.editOf) ?? [];
      list.push(p);
      m.set(p.editOf, list);
    }
    for (const list of m.values()) list.sort((a, b) => b.mtime - a.mtime);
    return m;
  });
  recipeByKey = $derived(new Map(this.recipes.map((r) => [r.key, r])));

  smartReady = $derived(!!this.smart?.enabled && (this.smart.phase === 'ready' || this.smart.phase === 'indexing') && this.smart.indexed > 0);

  /** True while results are ordered by relevance rather than the sort menu. */
  ranked = $derived(this.collection.type === 'similar' || (!!this.smartHits && this.smartHits.query === this.query.trim()));

  /** True for a RAW or Live Photo movie shown as part of its twin's tile. */
  folded(p: Photo, kinds?: Set<Kind>): boolean {
    if (!this.stack || p.pairId == null || !SECONDARY.includes(p.kind)) return false;
    const twin = this.byId.get(p.pairId);
    return !!twin && PRIMARY.includes(twin.kind) && (!kinds || kinds.has(twin.kind));
  }

  albumById = $derived(new Map(this.albums.map((a) => [a.id, a])));

  /** The moving half of a Live Photo, if `p` is the still. */
  liveMovie(p: Photo | undefined): Photo | undefined {
    const twin = p?.pairId != null && p.kind !== 'video' ? this.byId.get(p.pairId) : undefined;
    return twin?.kind === 'video' ? twin : undefined;
  }

  /** What a tile's corner badge says about the files behind it. */
  stackBadge(p: Photo): string | null {
    const twin = p.pairId != null ? this.byId.get(p.pairId) : undefined;
    if (twin && this.stack) {
      if (twin.kind === 'video' || p.kind === 'video') return 'LIVE';
      return `RAW+${p.kind === 'heif' ? 'HEIF' : 'JPG'}`;
    }
    return p.kind === 'raf' ? 'RAW' : p.kind === 'dng' ? 'DNG' : null;
  }

  /** Photos in the current collection, before toolbar filters (for counts). */
  inCollection = $derived.by(() => {
    const c = this.collection;
    const recent = Date.now() / 1000 - 7 * 86400;
    const similar = this.similarHits?.scores;
    const ids =
      c.type === 'ids' ? new Set(c.ids)
      : c.type === 'album' ? new Set(this.albumById.get(c.id)?.ids ?? [])
      : c.type === 'duplicates' ? new Set((this.duplicates ?? []).flatMap((g) => g.ids))
      : null;
    const test: (p: Photo) => boolean =
      c.type === 'all' || c.type === 'map' ? (p) => p.flag !== -1
      : c.type === 'picks' ? (p) => p.flag === 1
      : c.type === 'rated' ? (p) => p.rating > 0
      : c.type === 'rejected' ? (p) => p.flag === -1
      : c.type === 'recent' ? (p) => p.addedAt >= recent
      : c.type === 'favorites' ? (p) => p.favorite
      : c.type === 'album' ? (p) => ids!.has(p.id)
      : c.type === 'source' ? (p) => p.sourceId === c.id
      : c.type === 'film' ? (p) => p.filmSim === c.value
      : c.type === 'camera' ? (p) => p.camera === c.value
      : c.type === 'lens' ? (p) => p.lens === c.value
      : c.type === 'recipe' ? (p) => p.recipeKey === c.key && p.flag !== -1
      : c.type === 'country' ? (p) => p.country === c.value && p.flag !== -1
      : c.type === 'ids' || c.type === 'duplicates' ? (p) => ids!.has(p.id)
      : (p) => !!similar && (similar.has(p.id) || (p.pairId != null && similar.has(p.pairId)));
    return this.photos.filter(test);
  });

  visible = $derived.by(() => {
    const kinds = new Set(this.kinds);
    const q = this.query.trim().toLowerCase();
    const smart = this.smartHits && this.smartHits.query.toLowerCase() === q ? this.smartHits.scores : null;
    const similar = this.collection.type === 'similar' ? this.similarHits?.scores : null;
    const scoreOf = (scores: Map<number, number>, p: Photo) => scores.get(p.id) ?? (p.pairId != null ? scores.get(p.pairId) : undefined);
    const textMatch = new Set<number>();

    let list = this.inCollection.filter((p) => {
      if (!kinds.has(p.kind)) return false;
      if (p.rating < this.minRating) return false;
      // A RAW (or a Live Photo's movie) whose twin is shown is folded into that tile.
      if (this.folded(p, kinds)) return false;
      if (q) {
        const recipe = p.recipeKey ? this.recipeByKey.get(p.recipeKey)?.name : null;
        const hay = `${p.name} ${p.camera ?? ''} ${p.lens ?? ''} ${p.filmSim ?? ''} ${recipe ?? ''} ${p.folder} ${placeName(p) ?? ''}`.toLowerCase();
        const exact = q.split(/\s+/).every((t) => hay.includes(t));
        if (exact) textMatch.add(p.id);
        else if (!smart || scoreOf(smart, p) === undefined) return false;
      }
      return true;
    });

    const ranking = smart ?? similar;
    if (ranking) {
      // Exact text matches first, then by how well the picture matches.
      return list
        .map((p) => ({ p, exact: textMatch.has(p.id) ? 1 : 0, s: scoreOf(ranking, p) ?? -1 }))
        .sort((a, b) => b.exact - a.exact || b.s - a.s || (b.p.takenAt ?? 0) - (a.p.takenAt ?? 0))
        .map((x) => x.p);
    }

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
    if (this.ranked) {
      const title = this.collection.type === 'similar' ? 'Most similar first' : `Best matches for “${this.query.trim()}”`;
      return [{ key: 'ranked', title, subtitle: plural(list.length, 'photo'), items: list }];
    }
    if (this.collection.type === 'duplicates') return this.#duplicateSections(list);
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
        case 'recipe': {
          const row = p.recipeKey ? this.recipeByKey.get(p.recipeKey) : undefined;
          return [p.recipeKey ?? '', row ? `${recipeTitle(row)} · ${recipeSummary(row.recipe)}` : 'No recipe recorded'];
        }
        case 'place': {
          const name = placeName(p);
          return [name ?? '', name ?? 'No location'];
        }
        case 'camera': return [p.camera ?? '', p.camera ?? 'Unknown camera'];
        case 'lens': return [p.lens ?? '', p.lens ?? (p.camera?.startsWith('X100') ? 'Fixed 23mm' : 'Unknown lens')];
        case 'folder': return [p.folder, p.folder.split(/[\\/]/).filter(Boolean).slice(-2).join(' / ')];
        default: return ['', ''];
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
      const extras: string[] = [];
      if (g === 'day' || g === 'month') {
        const places = new Set(s.items.map((p) => p.town ?? (p.country ? countryName(p.country) : null)).filter(Boolean));
        if (places.size === 1) extras.push([...places][0]!);
        else if (places.size > 1) extras.push(`${places.size} places`);
        const films = new Set(s.items.map((p) => p.filmSim).filter(Boolean));
        if (films.size === 1) extras.push([...films][0]!);
        else if (films.size > 1) extras.push(`${films.size} film sims`);
      }
      s.subtitle = [plural(s.items.length, 'photo'), ...extras].join(' · ');
    }
    return sections;
  });

  /** Grid order, which the viewer and filmstrip navigate through. */
  ordered = $derived(this.sections.length === 1 ? this.sections[0].items : this.sections.flatMap((s) => s.items));

  navOrder = $derived.by(() => {
    if (!this.viewerList) return this.ordered;
    return this.viewerList.map((id) => this.byId.get(id)).filter((p): p is Photo => !!p);
  });

  facets = $derived.by(() => {
    const count = (key: (p: Photo) => string | null) => {
      const m = new Map<string, number>();
      for (const p of this.photos) {
        if (p.flag === -1 || this.folded(p)) continue;
        const k = key(p);
        if (k) m.set(k, (m.get(k) ?? 0) + 1);
      }
      return [...m.entries()].sort((a, b) => b[1] - a[1]);
    };
    return {
      films: count((p) => p.filmSim),
      cameras: count((p) => p.camera),
      lenses: count((p) => p.lens),
      countries: count((p) => p.country),
    };
  });

  counts = $derived.by(() => {
    let picks = 0, rated = 0, rejected = 0, recent = 0, all = 0, places = 0, favorites = 0;
    const since = Date.now() / 1000 - 7 * 86400;
    for (const p of this.photos) {
      if (this.folded(p)) continue;
      if (p.flag === -1) rejected++;
      else all++;
      if (p.flag === 1) picks++;
      if (p.rating > 0) rated++;
      if (p.addedAt >= since) recent++;
      if (p.lat != null && p.flag !== -1) places++;
      if (p.favorite) favorites++;
    }
    return { all, picks, rated, rejected, recent, places, favorites };
  });

  collectionTitle = $derived.by(() => {
    const c = this.collection;
    switch (c.type) {
      case 'all': return 'All Photos';
      case 'picks': return 'Picks';
      case 'rated': return 'Rated';
      case 'rejected': return 'Rejected';
      case 'recent': return 'Recently Added';
      case 'favorites': return 'Favourites';
      case 'album': return this.albumById.get(c.id)?.name ?? 'Album';
      case 'duplicates': return 'Duplicates';
      case 'map': return 'Places';
      case 'source': return this.sources.find((s) => s.id === c.id)?.label ?? 'Folder';
      case 'recipe': {
        const row = this.recipeByKey.get(c.key);
        return row ? recipeTitle(row) : 'Recipe';
      }
      case 'country': return countryName(c.value);
      case 'ids': return c.title;
      case 'similar': return `Similar to ${this.byId.get(c.id)?.name ?? 'photo'}`;
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
    cacheVersion.n = boot.settings.cacheVersion;
    this.effects = boot.effects;
    this.editor = boot.editor;
    this.smart = boot.smart;
    this.backup = boot.backup;
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
      const before = this.sync;
      const finished = before?.copying && !s.copying && s.lastImported;
      this.sync = s;
      if (finished) this.toast(`Imported ${plural(s.lastImported!, 'new photo')}`);
      // Retries repeat the same problem; say it once.
      if (s.error && !before?.error) this.toast(s.error, 'error');
    });
    await api.listen<{ done: number; total: number }>('export', (p) => {
      if (this.exporting) this.exporting = p;
    });
    await api.listen<BackupStatus>('backup', (b) => {
      const before = this.backup;
      this.backup = b;
      if (b.error && b.error !== before?.error && !b.running) this.toast(`Backup: ${b.error}`, 'error');
    });
    await api.listen('sources-changed', async () => {
      const before = new Set(this.sources.filter((s) => s.offline).map((s) => s.id));
      this.sources = await api.listSources();
      for (const s of this.sources) {
        if (s.offline && !before.has(s.id)) this.toast(`${s.label} can't be reached. Browsing from the cache until it's back.`, 'error');
        if (!s.offline && before.has(s.id)) {
          this.toast(`${s.label} is back`);
          this.reachEpoch++;
        }
      }
    });
    await api.listen<SmartStatus>('smart', (s) => {
      const finished = this.smart?.phase === 'indexing' && s.phase === 'ready';
      this.smart = s;
      if (s.phase === 'error' && s.error) this.toast(s.error, 'error');
      // Re-run the current search so newly analysed photos show up.
      if (finished && this.smartHits) this.#searchSmart(this.smartHits.query);
    });
    await api.onDropPaths((paths) => paths.forEach((p) => this.addSource(p)));
  }

  async refresh() {
    const [photos, sources, recipes, albums] = await Promise.all([api.listPhotos(), api.listSources(), api.listRecipes(), api.listAlbums()]);
    this.photos = photos;
    this.sources = sources;
    this.recipes = recipes;
    this.albums = albums;
  }

  // ---- actions ----

  toast(text: string, tone: Toast['tone'] = 'info', action?: Toast['action']) {
    const t = { id: ++this.#toastId, text, tone, action };
    this.toasts = [...this.toasts.slice(-2), t];
    setTimeout(() => (this.toasts = this.toasts.filter((x) => x !== t)), tone === 'error' || action ? 6000 : 2600);
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

  /** Sets one field on photos (and their twins), with undo. */
  async #set(field: Field, ids: number[], value: Photo[Field], message: string | null) {
    if (!ids.length) return;
    const all = [...new Set(ids.flatMap((id) => [id, this.byId.get(id)?.pairId ?? id]))];
    const before = new Map(all.map((id) => [id, this.byId.get(id)?.[field] ?? null] as [number, Photo[Field]]));
    const revert = this.#patch(ids, { [field]: value });
    try {
      await this.#send(field, ids, value);
      this.#undo = [...this.#undo.slice(-99), { field, ids: all, before, after: value }];
      this.#redo = [];
      if (message) this.toast(message);
    } catch (e) {
      revert();
      this.toast(String(e), 'error');
    }
  }

  #send(field: Field, ids: number[], value: Photo[Field]) {
    switch (field) {
      case 'rating': return api.setRating(ids, value as number);
      case 'flag': return api.setFlag(ids, value as number);
      case 'label': return api.setLabel(ids, value as Label | null);
      case 'favorite': return api.setFavorite(ids, value as boolean);
    }
  }

  async undo() {
    const change = this.#undo.pop();
    if (!change) return this.toast('Nothing to undo');
    // Put each photo back how it was, in one call per earlier value.
    const groups = new Map<Photo[Field], number[]>();
    for (const [id, v] of change.before) groups.set(v, [...(groups.get(v) ?? []), id]);
    try {
      for (const [value, ids] of groups) {
        this.#patch(ids, { [change.field]: value });
        await this.#send(change.field, ids, value);
      }
      this.#redo.push(change);
      this.toast(`Undid ${describe(change)}`);
    } catch (e) {
      this.toast(String(e), 'error');
      await this.refresh();
    }
  }

  async redo() {
    const change = this.#redo.pop();
    if (!change) return this.toast('Nothing to redo');
    try {
      this.#patch(change.ids, { [change.field]: change.after });
      await this.#send(change.field, change.ids, change.after);
      this.#undo.push(change);
      this.toast(`Redid ${describe(change)}`);
    } catch (e) {
      this.toast(String(e), 'error');
      await this.refresh();
    }
  }

  rate(rating: number, ids = this.targets) {
    return this.#set('rating', ids, rating, rating ? `${'★'.repeat(rating)}${ids.length > 1 ? ` · ${ids.length} photos` : ''}` : 'Rating cleared');
  }

  flag(flag: number, ids = this.targets) {
    return this.#set('flag', ids, flag, flag === 1 ? 'Marked as pick' : flag === -1 ? 'Rejected' : 'Flag cleared');
  }

  label(label: Label | null, ids = this.targets) {
    const current = ids.length === 1 ? this.byId.get(ids[0])?.label : undefined;
    return this.#set('label', ids, current === label ? null : label, null);
  }

  /** Toggles: favourites them unless they all already are. */
  favorite(ids = this.targets) {
    if (!ids.length) return;
    const on = !ids.every((id) => this.byId.get(id)?.favorite);
    return this.#set('favorite', ids, on, on ? 'Added to Favourites' : 'Removed from Favourites');
  }

  // ---- albums ----

  async createAlbum(name: string, ids: number[] = []) {
    try {
      const id = await api.createAlbum(name, ids);
      this.albums = await api.listAlbums();
      this.toast(ids.length ? `Added ${plural(ids.length, 'photo')} to “${name.trim()}”` : `Created “${name.trim()}”`);
      return id;
    } catch (e) {
      this.toast(String(e), 'error');
      return null;
    }
  }

  async addToAlbum(id: number, ids = this.targets) {
    if (!ids.length) return;
    await api.addToAlbum(id, ids);
    this.albums = await api.listAlbums();
    this.toast(`Added ${plural(ids.length, 'photo')} to “${this.albumById.get(id)?.name ?? 'album'}”`);
  }

  async removeFromAlbum(id: number, ids = this.targets) {
    if (!ids.length) return;
    await api.removeFromAlbum(id, ids);
    this.albums = await api.listAlbums();
    this.toast(`Removed ${plural(ids.length, 'photo')} from “${this.albumById.get(id)?.name ?? 'album'}”`);
  }

  async renameAlbum(id: number, name: string) {
    if (!name.trim()) return;
    try {
      await api.renameAlbum(id, name.trim());
      this.albums = await api.listAlbums();
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  async deleteAlbum(id: number) {
    const name = this.albumById.get(id)?.name;
    await api.deleteAlbum(id);
    this.albums = await api.listAlbums();
    if (this.collection.type === 'album' && this.collection.id === id) this.setCollection({ type: 'all' });
    this.toast(`Deleted the album “${name}”. The photos are still in your library.`);
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

  /** Asks before doing something that can't be undone with Ctrl+Z. */
  ask(title: string, body: string, action: string): Promise<boolean> {
    this.confirmation?.resolve(false);
    return new Promise((resolve) => (this.confirmation = { title, body, action, resolve }));
  }

  answer(ok: boolean) {
    this.confirmation?.resolve(ok);
    this.confirmation = null;
  }

  /** Moves photos to the Recycle Bin after asking. RAW twins go too. */
  async deletePhotos(ids = this.targets, what = 'photo') {
    if (!ids.length) return;
    const photos = ids.map((id) => this.byId.get(id)).filter((p) => !!p);
    const twins = photos.some((p) => p.pairId != null) ? ' RAW+JPEG twins go together.' : '';
    // Windows can't recycle from network drives; those go to a trash folder.
    const remote = photos.filter((p) => p.folder.startsWith('\\\\')).length;
    const they = ids.length === 1 ? 'It goes' : 'They go';
    const where =
      remote === 0
        ? `${they} to the Recycle Bin.`
        : remote === photos.length
          ? `${they} to a .fujicloud-trash folder in the library folder, since Windows has no Recycle Bin for network drives. Delete that folder to free up the space.`
          : `${they} to the Recycle Bin, or for network drives, to a .fujicloud-trash folder in the library folder.`;
    const ok = await this.ask(`Delete ${plural(ids.length, what)}?`, where + twins, 'Delete');
    if (!ok) return;
    try {
      const r = await api.deletePhotos(ids);
      const gone = new Set(r.ids);
      const list = this.viewerList ?? this.visible.map((p) => p.id);
      if (this.viewerId != null && gone.has(this.viewerId)) {
        const at = list.indexOf(this.viewerId);
        const next = list.slice(at + 1).find((id) => !gone.has(id)) ?? list.slice(0, Math.max(at, 0)).findLast((id) => !gone.has(id));
        this.viewerId = next ?? null;
      }
      if (this.viewerList) this.viewerList = this.viewerList.filter((id) => !gone.has(id));
      this.photos = this.photos.filter((p) => !gone.has(p.id));
      this.selection = new Set([...this.selection].filter((id) => !gone.has(id)));
      if (this.focusId != null && gone.has(this.focusId)) this.focusId = null;
      const moved = ids.filter((id) => gone.has(id)).length;
      if (moved) this.toast(`Moved ${plural(moved, what)} to ${r.places.join(' and ') || 'the Recycle Bin'}`);
      if (r.failed.length) {
        this.toast(`${plural(r.failed.length, 'file')} couldn't be deleted: ${r.failed.slice(0, 3).join(', ')}${r.failed.length > 3 ? '…' : ''}`, 'error');
      }
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  /** Deletes every rejected photo (one tile per RAW+JPEG pair). */
  deleteRejected() {
    return this.deletePhotos(
      this.photos.filter((p) => p.flag === -1 && !this.folded(p)).map((p) => p.id),
      'rejected photo',
    );
  }

  /** Asks where, then exports copies of photos. */
  async exportPhotos(ids: number[], preset: ExportPreset, location: boolean) {
    const folder = await api.pickFolder(`Export ${plural(ids.length, 'photo')} to`);
    if (!folder) return;
    this.exportDialog = null;
    this.exporting = { done: 0, total: ids.length };
    try {
      const r = await api.exportPhotos(ids, preset, location, folder);
      const show = { label: 'Show folder', run: () => api.openFolder(folder) };
      if (r.files) this.toast(`Exported ${plural(r.files, 'file')}`, 'info', show);
      if (r.failed.length) {
        this.toast(`${plural(r.failed.length, 'photo')} couldn't be exported: ${r.failed.slice(0, 3).join(', ')}${r.failed.length > 3 ? '…' : ''}`, 'error');
      }
    } catch (e) {
      this.toast(String(e), 'error');
    } finally {
      this.exporting = null;
    }
  }

  async rebuildThumbnails() {
    try {
      cacheVersion.n = await api.rebuildThumbnails();
      if (this.settings) this.settings = { ...this.settings, cacheVersion: cacheVersion.n };
      this.toast('Rebuilding thumbnails and previews. Photos fill back in as they are done.');
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  async nameRecipe(key: string, name: string | null) {
    await api.nameRecipe(key, name);
    this.recipes = await api.listRecipes();
  }

  async setSmartSearch(enabled: boolean) {
    try {
      this.smart = await api.setSmartSearch(enabled);
      if (this.settings) this.settings = { ...this.settings, smartSearch: enabled };
      if (!enabled) this.smartHits = null;
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  async setSmartGpu(enabled: boolean) {
    try {
      this.smart = await api.setSmartGpu(enabled);
      if (this.settings) this.settings = { ...this.settings, smartGpu: enabled };
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  async #searchSmart(query: string) {
    this.smartBusy = true;
    try {
      const hits = await api.searchSmart(query);
      // Ignore results for a query the user has already typed past.
      if (query === this.query.trim()) this.smartHits = { query, scores: toScores(hits) };
    } catch (e) {
      this.toast(String(e), 'error');
    } finally {
      this.smartBusy = false;
    }
  }

  async findSimilar(id: number) {
    try {
      const hits = await api.similarPhotos(id);
      this.similarHits = { id, scores: toScores(hits) };
      this.viewerId = null;
      this.query = '';
      this.setCollection({ type: 'similar', id });
    } catch (e) {
      this.toast(String(e), 'error');
    }
  }

  /** The before/after pair for a photo: an edit with its original, or an
   * original with its newest edit. */
  comparisonFor(p: Photo | undefined): { before: number; after: number } | null {
    if (!p) return null;
    if (p.editOf != null && this.byId.has(p.editOf)) return { before: p.editOf, after: p.id };
    const original = p.kind === 'raf' && p.pairId != null ? this.byId.get(p.pairId) : p;
    const edit = this.editsOf.get(p.id)?.[0] ?? (original ? this.editsOf.get(original.id)?.[0] : undefined);
    return edit ? { before: p.id, after: edit.id } : null;
  }

  /** What to compare for a selection: two photos side by side (original or
   * older one first), or one photo against its edit or original. */
  comparisonOf(ids: number[]): { before: number; after: number } | null {
    if (ids.length === 1) return this.comparisonFor(this.byId.get(ids[0]));
    if (ids.length !== 2) return null;
    const [a, b] = ids.map((id) => this.byId.get(id));
    if (!a || !b) return null;
    const rank = (p: Photo) => [p.editOf != null ? 1 : 0, p.takenAt ?? p.mtime, p.mtime];
    const [ra, rb] = [rank(a), rank(b)];
    const aFirst = ra[0] !== rb[0] ? ra[0] < rb[0] : ra[1] !== rb[1] ? ra[1] < rb[1] : ra[2] <= rb[2];
    return aFirst ? { before: a.id, after: b.id } : { before: b.id, after: a.id };
  }

  openCompare(pair: { before: number; after: number } | null) {
    if (pair) this.compare = pair;
  }

  showOnMap(p: Photo) {
    if (p.lat == null || p.lon == null) return;
    this.viewerId = null;
    this.mapFocus = { lat: p.lat, lon: p.lon, zoom: 13 };
    this.setCollection({ type: 'map' });
  }

  setCollection(c: Collection) {
    this.collection = c;
    this.selection = new Set();
    // The library may have changed since last time: look again.
    if (c.type === 'duplicates') this.findDuplicates();
  }

  async findDuplicates() {
    if (this.findingDuplicates) return;
    this.findingDuplicates = true;
    try {
      this.duplicates = await api.findDuplicates();
    } catch (e) {
      this.toast(String(e), 'error');
    } finally {
      this.findingDuplicates = false;
    }
  }

  /** One section per duplicate group, the keeper first, in the order found. */
  #duplicateSections(list: Photo[]): Section[] {
    const shown = new Set(list.map((p) => p.id));
    return (this.duplicates ?? []).flatMap((g, i) => {
      const items = g.ids.filter((id) => shown.has(id)).map((id) => this.byId.get(id)!);
      if (items.length < 2) return [];
      const extras = items.slice(1).map((p) => p.id);
      const day = items[0].takenAt ? ` · ${formatDay(items[0].takenAt)}` : '';
      return [
        {
          key: `dupes-${i}`,
          title: g.exact ? 'Identical copies' : 'Near-identical shots',
          subtitle: `${plural(items.length, 'photo')}${day}`,
          items,
          action: { label: extras.length === 1 ? 'Reject the extra' : `Reject the ${extras.length} extras`, run: () => this.flag(-1, extras) },
        },
      ];
    });
  }

  /** Rejects every duplicate but the keeper of each group. */
  rejectExtras() {
    const extras = this.sections.flatMap((s) => s.items.slice(1).map((p) => p.id));
    if (extras.length) this.flag(-1, extras);
  }

  open(id: number, list: number[] | null = null) {
    this.focusId = id;
    this.viewerList = list;
    this.viewerId = id;
  }
}

function describe(c: Change): string {
  const n = c.ids.length > 2 ? ` on ${plural(c.ids.length, 'photo')}` : '';
  switch (c.field) {
    case 'rating': return `rating${n}`;
    case 'flag': return `pick/reject${n}`;
    case 'label': return `colour label${n}`;
    case 'favorite': return `favourite${n}`;
  }
}

function toScores(hits: Hit[]) {
  return new Map(hits.map((h) => [h.id, h.score]));
}

export const store = new Store();
