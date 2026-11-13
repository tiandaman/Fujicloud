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
  /** Ask before taking changes made on another PC. */
  reviewSync: boolean;
  editorPath: string | null;
  editRaw: boolean;
  smartSearch: boolean;
  smartGpu: boolean;
  faces: boolean;
  pets: boolean;
  /** Read the text in photos so it can be searched. */
  readText: boolean;
  /** How big a model each analysis uses: left to the PC, or chosen. */
  models: ModelChoice;
  cacheVersion: number;
}

export type ModelChoice = 'auto' | ModelLevel;
export type ModelLevel = 'light' | 'standard' | 'best';

/** What this PC has, and the size of model it gets. */
export interface Models {
  hardware: { processors: number; memoryGb: number; gpu: string | null; gpuMemoryGb: number };
  level: ModelLevel;
  /** What Automatic would choose here. */
  recommended: ModelLevel;
}

export interface Bootstrap {
  /** e.g. "0.2.29": the last number is the CI build it came from. */
  version: string;
  sources: Source[];
  settings: Settings;
  effects: boolean;
  editor: string | null;
  smart: SmartStatus;
  backup: BackupStatus;
  faces: FacesStatus;
  pets: PetsStatus;
  texts: TextStatus;
  models: Models;
}

/** Someone found in the photos: named, or a group of faces waiting for a name. */
export interface Person {
  id: number;
  name: string | null;
  /** The clearest face, for the avatar. */
  cover: number;
  faces: number;
  photoIds: number[];
  /** A stranger, kept out of the way. */
  hidden: boolean;
  kind: Subject;
  /** Holds a photo someone took out of another person: asked about even alone. */
  takenOut: boolean;
}

/** Who a group of faces belongs to. */
export type Subject = 'person' | 'pet';

export interface FaceInPhoto {
  id: number;
  personId: number | null;
  name: string | null;
  /** x, y, width, height as fractions of the picture. */
  bbox: [number, number, number, number];
  kind: Subject;
}

export interface FacesStatus {
  enabled: boolean;
  phase: 'off' | 'downloading' | 'indexing' | 'ready' | 'error';
  /** Bytes while downloading, photos while looking for faces. */
  done: number;
  total: number;
  downloadMb: number;
  /** Photos searched so far. */
  scanned: number;
  error: string | null;
}

export interface PetsStatus extends Omit<FacesStatus, 'phase'> {
  phase: 'off' | 'downloading' | 'indexing' | 'ready' | 'error';
}

/** Reading the text in photos. */
export interface TextStatus {
  enabled: boolean;
  phase: 'off' | 'downloading' | 'indexing' | 'ready' | 'error';
  /** Bytes while downloading, photos while reading. */
  done: number;
  total: number;
  downloadMb: number;
  /** Photos read so far, and how many had something written in them. */
  read: number;
  withText: number;
  error: string | null;
}

export interface BackupStatus {
  /** The backup folder, or null when backup is off. */
  target: string | null;
  /** Unix seconds when the last complete pass finished. */
  lastBackedUp: number | null;
  running: boolean;
  done: number;
  total: number;
  /** Why the last pass didn't finish cleanly. */
  error: string | null;
  /** Reading every photo and its backup copy back, and comparing them. */
  checking: boolean;
  checkDone: number;
  checkTotal: number;
  /** Unix seconds when the last complete check finished. */
  lastChecked: number | null;
  /** Photos whose two copies differ, and ones that couldn't be read. */
  damaged: string[];
  /** For each of `damaged`, which copy looks like the bad one. */
  damagedSide: ('library' | 'backup' | 'unknown')[];
  unreadable: string[];
  checkSame: number;
  /** Not in the backup yet (the next backup copies them). */
  checkWaiting: number;
}

/** An analysis that's on: photos still to look at, and ones it couldn't read. */
export interface Pending {
  todo: number;
  unreadable: number;
  /** Which ones (the first few hundred). */
  unreadableIds: number[];
}

/** A warning or problem from the log. */
export interface LogEntry {
  /** `2026-11-13 17:40:09` */
  at: string;
  problem: boolean;
  text: string;
}

export interface Health {
  photos: number;
  /** What's wrong with the database, if it was checked (`databaseChecked`). */
  databaseProblem: string | null;
  databaseChecked: boolean;
  sidecarsOwed: number;
  sidecarsOn: boolean;
  /** Changes from other PCs waiting for a look. */
  held: number;
  /** Library folders that can't be reached right now. */
  offline: string[];
  /** Photos that couldn't be shown this session. */
  unshown: number;
  unshownIds: number[];
  smart: Pending | null;
  faces: Pending | null;
  pets: Pending | null;
  text: Pending | null;
  backup: BackupStatus;
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

/** Sizes for exported copies (see the Rust `export::Preset`). */
export type ExportPreset = 'original' | 'full' | 'web' | 'instagram';

export interface Exported {
  /** Files written. */
  files: number;
  /** Names of photos that couldn't be exported. */
  failed: string[];
}

/** What `deletePhotos` did. */
export interface Deleted {
  /** Photos taken out of the library, RAW+JPEG twins included. */
  ids: number[];
  /** Where they went: the Recycle Bin, or a library folder's trash. */
  places: string[];
  /** Names of photos that couldn't be moved. */
  failed: string[];
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
  | { type: 'trash' }
  | { type: 'people' }
  | { type: 'pets' }
  | { type: 'person'; id: number }
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
  /** A button in the section's header. */
  action?: { label: string; run: () => void };
}

/** Photos that are copies of each other, the one to keep first. */
export interface DuplicateGroup {
  ids: number[];
  /** Byte-for-byte the same file; otherwise the same picture saved again (resized, re-saved). */
  exact: boolean;
}

/** What someone sets on a photo themselves. */
export interface Marks {
  rating: number;
  /** 1 pick, -1 reject. */
  flag: number;
  label: string | null;
  favorite: boolean;
  albums: string[];
}

/** A change another PC made, waiting to be applied or rejected. */
export interface SyncChange {
  id: number;
  name: string;
  folder: string;
  kind: Kind;
  takenAt: number | null;
  /** The file is gone: deleted on another PC. */
  removed: boolean;
  /** A deleted file is in the library folder's trash, so it can be put back. */
  restorable: boolean;
  before: Marks;
  /** What the other PC saved (none for a deletion). */
  after: Marks | null;
  /** The other half of a RAW+JPEG pair, decided along with it. */
  twins: number[];
  /** Which PC made the change, and when (Unix seconds), where known. */
  by: string | null;
  at: number | null;
}

/** A photo in a library folder's trash. */
export interface TrashItem {
  /** Its place in the listing. */
  n: number;
  sourceId: number;
  name: string;
  /** The folder it was in, within its library folder. */
  folder: string;
  kind: Kind;
  size: number;
  /** When it was deleted (Unix seconds) and by which PC, if this app did. */
  at: number | null;
  by: string | null;
}

export interface Trash {
  items: TrashItem[];
  bytes: number;
  /** Some library folders use the Windows Recycle Bin instead. */
  recycleBin: boolean;
  /** Library folders that can't be reached right now. */
  offline: number;
}

export interface SyncResolved {
  applied: number;
  kept: number;
  restored: number;
  /** Deleted photos that couldn't be put back. */
  failed: string[];
}
