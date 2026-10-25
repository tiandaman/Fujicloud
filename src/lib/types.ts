export type Kind = 'jpeg' | 'raf' | 'heif' | 'tiff' | 'psd' | 'png' | 'dng' | 'video';

/** One indexed file. Mirrors `fuji_core::db::PhotoRow`. */
export interface Photo {
  id: number;
  sourceId: number;
  name: string;
  folder: string;
  kind: Kind;
  size: number;
  mtime: number;
  /** Camera-clock time as naive seconds; format with `timeZone: 'UTC'`. */
  takenAt: number | null;
  width: number;
  height: number;
  camera: string | null;
  lens: string | null;
  focalLength: number | null;
  aperture: number | null;
  shutter: number | null;
  iso: number | null;
  filmSim: string | null;
  rating: number;
  /** 1 pick, -1 reject, 0 none. */
  flag: number;
  label: Label | null;
  addedAt: number;
  /** The other half of a RAW+JPEG pair. */
  pairId: number | null;
  recipeKey: string | null;
  lat: number | null;
  lon: number | null;
  /** The position was borrowed from a geotagged shot taken around the same time. */
  gpsEstimated: boolean;
  town: string | null;
  region: string | null;
  /** ISO 3166-1 alpha-2. */
  country: string | null;
  /** For an edit (Photoshop save or export), the camera original. */
  editOf: number | null;
  favorite: boolean;
  /** Videos: length in seconds. */
  duration: number | null;
}

export interface Album {
  id: number;
  name: string;
  createdAt: number;
  ids: number[];
}

/** The camera's film recipe. Mirrors `fuji_core::fuji::Recipe`. */
export interface Recipe {
  filmSim: string | null;
  grain: string | null;
  colorChrome: string | null;
  colorChromeBlue: string | null;
  whiteBalance: string | null;
  wbShift: [number, number] | null;
  dynamicRange: string | null;
  highlight: number | null;
  shadow: number | null;
  color: number | null;
  sharpness: number | null;
  noiseReduction: number | null;
  clarity: number | null;
  monoWarmCool: number | null;
  monoMagentaGreen: number | null;
}

export interface RecipeRow {
  key: string;
  name: string | null;
  recipe: Recipe;
  count: number;
}

export interface SmartStatus {
  enabled: boolean;
  phase: 'off' | 'downloading' | 'loading' | 'indexing' | 'ready' | 'error';
  /** Bytes while downloading, photos while indexing. */
  done: number;
  total: number;
  indexed: number;
  downloadMb: number;
  error: string | null;
  /** The graphics card found on this PC, if any. */
  gpu: { name: string; vramMb: number } | null;
  useGpu: boolean;
  /** "GPU" or "CPU" once the model is loaded. */
  runningOn: string | null;
  gpuError: string | null;
}

export interface Hit {
  id: number;
  score: number;
}

export type Label = 'Red' | 'Yellow' | 'Green' | 'Blue' | 'Purple';

export interface Detail extends Photo {
  path: string;
  orientation: number;
  exposureComp: number | null;
  altitude: number | null;
  recipe: Recipe | null;
  exif: [string, string][];
}

export interface Source {
  id: number;
  path: string;
  label: string;
  count: number;
  /** Can't be reached right now (the Shield is off); browsing from the cache. */
  offline?: boolean;
}

/** A folder new shots arrive in: PC AutoSave, iCloud Photos, … */
export interface Inbox {
  path: string;
  /** Recycle the inbox file once the library's copy has been checked. */
  cleanup: boolean;
}

export interface Settings {
  inboxes: Inbox[];
  importTo: string | null;
  backupTo: string | null;
  parallelCopies: number;
  writeXmp: boolean;
  editorPath: string | null;
  editRaw: boolean;
  smartSearch: boolean;
  smartGpu: boolean;
}

export interface Bootstrap {
  sources: Source[];
  settings: Settings;
  effects: boolean;
  editor: string | null;
  smart: SmartStatus;
}

export interface ScanProgress {
  sourceId: number;
  phase: 'listing' | 'reading' | 'done';
  done: number;
  total: number;
}

export interface SyncStatus {
  active: boolean;
  inboxes?: string[];
  library?: string;
  copying?: number;
  done?: number;
  total?: number;
  lastImported?: number;
  /** Photos waiting for the library to be reachable. */
  waiting?: number;
  error?: string | null;
}

export interface CacheInfo {
  bytes: number;
  path: string;
  backlog: number;
}

export type Collection =
  | { type: 'all' }
  | { type: 'picks' }
  | { type: 'rated' }
  | { type: 'rejected' }
  | { type: 'recent' }
  | { type: 'favorites' }
  | { type: 'album'; id: number }
  | { type: 'duplicates' }
  | { type: 'source'; id: number }
  | { type: 'film'; value: string }
  | { type: 'camera'; value: string }
  | { type: 'lens'; value: string }
  | { type: 'recipe'; key: string }
  | { type: 'country'; value: string }
  | { type: 'map' }
  | { type: 'ids'; ids: number[]; title: string }
  | { type: 'similar'; id: number };

export type SortKey =
  | 'taken'
  | 'added'
  | 'name'
  | 'size'
  | 'rating'
  | 'film'
  | 'camera'
  | 'focal'
  | 'aperture'
  | 'shutter'
  | 'iso';

export type GroupKey = 'none' | 'day' | 'month' | 'year' | 'film' | 'recipe' | 'place' | 'camera' | 'lens' | 'folder';

export type LayoutMode = 'justified' | 'grid';

export interface Section {
  key: string;
  title: string;
  subtitle: string;
  items: Photo[];
}
