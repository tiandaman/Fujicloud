export type Kind = 'jpeg' | 'raf' | 'heif' | 'tiff';

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
}

export type Label = 'Red' | 'Yellow' | 'Green' | 'Blue' | 'Purple';

export interface Detail extends Photo {
  path: string;
  orientation: number;
  exposureComp: number | null;
  grain: string | null;
  dynamicRange: string | null;
  exif: [string, string][];
}

export interface Source {
  id: number;
  path: string;
  label: string;
  count: number;
}

export interface Settings {
  inbox: string | null;
  importTo: string | null;
  parallelCopies: number;
  writeXmp: boolean;
  editorPath: string | null;
  editRaw: boolean;
}

export interface Bootstrap {
  sources: Source[];
  settings: Settings;
  effects: boolean;
  editor: string | null;
}

export interface ScanProgress {
  sourceId: number;
  phase: 'listing' | 'reading' | 'done';
  done: number;
  total: number;
}

export interface SyncStatus {
  active: boolean;
  inbox?: string;
  library?: string;
  copying?: number;
  done?: number;
  total?: number;
  lastImported?: number;
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
  | { type: 'source'; id: number }
  | { type: 'film'; value: string }
  | { type: 'camera'; value: string }
  | { type: 'lens'; value: string };

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

export type GroupKey = 'none' | 'day' | 'month' | 'year' | 'film' | 'camera' | 'lens' | 'folder';

export type LayoutMode = 'justified' | 'grid';

export interface Section {
  key: string;
  title: string;
  subtitle: string;
  items: Photo[];
}
