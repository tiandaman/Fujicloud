import { cacheVersion } from './version.svelte';
import type { Album, BackupStatus, Bootstrap, CacheInfo, Deleted, DuplicateGroup, FaceInPhoto, FacesStatus, Person, PetsStatus, ExportPreset, Exported, Detail, Hit, Label, Photo, RecipeRow, Settings, SmartStatus, Source, Subject } from './types';

export interface Api {
  /** False when running in a plain browser against mock data. */
  native: boolean;
  bootstrap(): Promise<Bootstrap>;
  listPhotos(): Promise<Photo[]>;
  listSources(): Promise<Source[]>;
  photoDetail(id: number): Promise<Detail>;
  addSource(path: string): Promise<Source>;
  removeSource(id: number): Promise<void>;
  renameSource(id: number, label: string): Promise<void>;
  rescan(): Promise<void>;
  setRating(ids: number[], rating: number): Promise<number[]>;
  setFlag(ids: number[], flag: number): Promise<number[]>;
  setLabel(ids: number[], label: Label | null): Promise<number[]>;
  setFavorite(ids: number[], favorite: boolean): Promise<number[]>;
  listAlbums(): Promise<Album[]>;
  /** Creates (or reuses) the album with this name and adds `ids`. */
  createAlbum(name: string, ids: number[]): Promise<number>;
  renameAlbum(id: number, name: string): Promise<void>;
  deleteAlbum(id: number): Promise<void>;
  addToAlbum(id: number, ids: number[]): Promise<void>;
  removeFromAlbum(id: number, ids: number[]): Promise<void>;
  facesStatus(): Promise<FacesStatus>;
  setFaces(enabled: boolean): Promise<FacesStatus>;
  petsStatus(): Promise<PetsStatus>;
  setPets(enabled: boolean): Promise<PetsStatus>;
  listPeople(): Promise<Person[]>;
  facesIn(photoId: number): Promise<FaceInPhoto[]>;
  /** Names a person or group; a name already in use merges them. Returns who they are now. */
  namePerson(id: number, name: string): Promise<number>;
  /** Says who one face is. Returns the person. */
  nameFace(faceId: number, name: string): Promise<number>;
  /** "Not this person" for these photos. */
  removeFromPerson(id: number, photoIds: number[]): Promise<void>;
  /** Moves these photos from one person (or pet) to another, or to a new name. */
  moveToPerson(from: number, photoIds: number[], to: { id: number } | { name: string }): Promise<number>;
  /** One person found as several: moves `from` into `into`, which keeps or takes a name. */
  mergePeople(from: number[], into: number): Promise<number>;
  /** Forgets a person's name; their faces are grouped again. */
  deletePerson(id: number): Promise<void>;
  /** Groups the unnamed again around who's named, and takes out of each named one what doesn't look like the photos you confirmed. Returns how many were taken out. */
  regroupPeople(kind: Subject): Promise<number>;
  /** Sends every photo through face (or pet) recognition again; `forgetNames` also forgets every group and name. */
  reidentify(kind: Subject, forgetNames: boolean): Promise<void>;
  /** Hides strangers, or brings them back. */
  hidePeople(ids: number[], hidden: boolean): Promise<void>;
  /** "I don't know them", from one face in a photo. Returns who's hidden. */
  hideFace(faceId: number): Promise<number>;
  faceUrl(faceId: number): string;
  /** Copies of the same file, and near-identical shots. */
  findDuplicates(): Promise<DuplicateGroup[]>;
  /** Moves photos (with their twins and sidecars) to the Recycle Bin. */
  deletePhotos(ids: number[]): Promise<Deleted>;
  /** Exports copies into `folder`; progress arrives as `export` events. */
  exportPhotos(ids: number[], preset: ExportPreset, location: boolean, folder: string): Promise<Exported>;
  /** Starts dragging the photos' files out of the window, into another app. */
  dragOut(ids: number[]): Promise<void>;
  openFolder(path: string): Promise<void>;
  /** Converts a movie the window can't play; `convertedUrl` then serves it. */
  prepareMovie(id: number): Promise<void>;
  convertedUrl(p: Photo): string;
  openInEditor(ids: number[]): Promise<string>;
  reveal(id: number): Promise<void>;
  openDefault(id: number): Promise<void>;
  saveSettings(settings: Settings): Promise<void>;
  backupStatus(): Promise<BackupStatus>;
  backupNow(): Promise<void>;
  /** Inbox folders found on this PC (PC AutoSave, iCloud Photos). */
  suggestInboxes(): Promise<string[]>;
  cacheInfo(): Promise<CacheInfo>;
  /** Rebuilds every thumbnail and preview; returns the new cache version. */
  rebuildThumbnails(): Promise<number>;
  openLogFolder(): Promise<void>;
  listRecipes(): Promise<RecipeRow[]>;
  nameRecipe(key: string, name: string | null): Promise<void>;
  smartStatus(): Promise<SmartStatus>;
  setSmartSearch(enabled: boolean): Promise<SmartStatus>;
  setSmartGpu(enabled: boolean): Promise<SmartStatus>;
  searchSmart(query: string): Promise<Hit[]>;
  similarPhotos(id: number): Promise<Hit[]>;
  pickFolder(title: string): Promise<string | null>;
  listen<T>(event: string, cb: (payload: T) => void): Promise<() => void>;
  onDropPaths(cb: (paths: string[]) => void): Promise<() => void>;
  setFullscreen(on: boolean): Promise<void>;
  isFullscreen(): Promise<boolean>;
  thumbUrl(p: Photo): string;
  previewUrl(p: Photo): string;
  originalUrl(p: Photo): string;
}

function tauriApi(): Api {
  // WebView2 serves custom schemes as http://<scheme>.localhost.
  const base = navigator.userAgent.includes('Windows') ? 'http://fuji.localhost' : 'fuji://localhost';
  /** Files being dragged out of the window right now. */
  let outgoing = new Set<string>();
  const core = import('@tauri-apps/api/core');
  const invoke = async <T>(cmd: string, args?: Record<string, unknown>) => (await core).invoke<T>(cmd, args);

  return {
    native: true,
    bootstrap: () => invoke('bootstrap'),
    listPhotos: () => invoke('list_photos'),
    listSources: () => invoke('list_sources'),
    photoDetail: (id) => invoke('photo_detail', { id }),
    addSource: (path) => invoke('add_source', { path }),
    removeSource: (id) => invoke('remove_source', { id }),
    renameSource: (id, label) => invoke('rename_source', { id, label }),
    rescan: () => invoke('rescan'),
    setRating: (ids, rating) => invoke('set_rating', { ids, rating }),
    setFlag: (ids, flag) => invoke('set_flag', { ids, flag }),
    setLabel: (ids, label) => invoke('set_label', { ids, label }),
    setFavorite: (ids, favorite) => invoke('set_favorite', { ids, favorite }),
    listAlbums: () => invoke('list_albums'),
    createAlbum: (name, ids) => invoke('create_album', { name, ids }),
    renameAlbum: (id, name) => invoke('rename_album', { id, name }),
    deleteAlbum: (id) => invoke('delete_album', { id }),
    addToAlbum: (id, ids) => invoke('add_to_album', { id, ids }),
    removeFromAlbum: (id, ids) => invoke('remove_from_album', { id, ids }),
    facesStatus: () => invoke('faces_status'),
    setFaces: (enabled) => invoke('set_faces', { enabled }),
    petsStatus: () => invoke('pets_status'),
    setPets: (enabled) => invoke('set_pets', { enabled }),
    listPeople: () => invoke('list_people'),
    facesIn: (id) => invoke('faces_in', { id }),
    namePerson: (id, name) => invoke('name_person', { id, name }),
    nameFace: (id, name) => invoke('name_face', { id, name }),
    removeFromPerson: (id, photoIds) => invoke('remove_from_person', { id, photoIds }),
    moveToPerson: (from, photoIds, to) =>
      invoke('move_to_person', { from, photoIds, to: 'id' in to ? to.id : null, name: 'name' in to ? to.name : null }),
    deletePerson: (id) => invoke('delete_person', { id }),
    mergePeople: (from, into) => invoke('merge_people', { from, into }),
    regroupPeople: (kind) => invoke('regroup_people', { kind }),
    reidentify: (kind, forgetNames) => invoke('reidentify', { kind, forgetNames }),
    hidePeople: (ids, hidden) => invoke('hide_people', { ids, hidden }),
    hideFace: (id) => invoke('hide_face', { id }),
    faceUrl: (id) => `${base}/f/${id}?v=${cacheVersion.n}`,
    findDuplicates: () => invoke('find_duplicates'),
    deletePhotos: (ids) => invoke('delete_photos', { ids }),
    exportPhotos: (ids, preset, location, folder) => invoke('export_photos', { ids, preset, location, folder }),
    async dragOut(ids) {
      const files = await invoke<{ paths: string[]; icon: string }>('drag_files', { ids });
      const { startDrag } = await import('@crabnebula/tauri-plugin-drag');
      outgoing = new Set(files.paths);
      try {
        await startDrag({ item: files.paths, icon: files.icon });
      } finally {
        // The drop event can come just after the drag ends.
        setTimeout(() => (outgoing = new Set()), 1000);
      }
    },
    openFolder: (path) => invoke('open_folder', { path }),
    prepareMovie: (id) => invoke('prepare_movie', { id }),
    convertedUrl: (p) => `${base}/v/${p.id}?v=${p.mtime}`,
    openInEditor: (ids) => invoke('open_in_editor', { ids }),
    reveal: (id) => invoke('reveal', { id }),
    openDefault: (id) => invoke('open_default', { id }),
    saveSettings: (settings) => invoke('save_settings', { settings }),
    suggestInboxes: () => invoke('suggest_inboxes'),
    backupStatus: () => invoke('backup_status'),
    backupNow: () => invoke('backup_now'),
    cacheInfo: () => invoke('cache_info'),
    rebuildThumbnails: () => invoke('rebuild_thumbnails'),
    openLogFolder: () => invoke('open_log_folder'),
    listRecipes: () => invoke('list_recipes'),
    nameRecipe: (key, name) => invoke('name_recipe', { key, name }),
    smartStatus: () => invoke('smart_status'),
    setSmartSearch: (enabled) => invoke('set_smart_search', { enabled }),
    setSmartGpu: (enabled) => invoke('set_smart_gpu', { enabled }),
    searchSmart: (query) => invoke('search_smart', { query }),
    similarPhotos: (id) => invoke('similar_photos', { id }),
    async pickFolder(title) {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const picked = await open({ directory: true, multiple: false, title });
      return typeof picked === 'string' ? picked : null;
    },
    async listen(event, cb) {
      const { listen } = await import('@tauri-apps/api/event');
      return listen(event, (e) => cb(e.payload as never));
    },
    async onDropPaths(cb) {
      const { getCurrentWebview } = await import('@tauri-apps/api/webview');
      return getCurrentWebview().onDragDropEvent((e) => {
        // Photos dragged out and let go over this window aren't new folders.
        if (e.payload.type === 'drop') {
          const paths = e.payload.paths.filter((p) => !outgoing.has(p));
          if (paths.length) cb(paths);
        }
      });
    },
    async setFullscreen(on) {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().setFullscreen(on);
    },
    async isFullscreen() {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      return getCurrentWindow().isFullscreen();
    },
    // The mtime makes each URL immutable, so the webview can cache forever.
    thumbUrl: (p) => `${base}/t/${p.id}?v=${p.mtime}.${cacheVersion.n}`,
    previewUrl: (p) => `${base}/p/${p.id}?v=${p.mtime}.${cacheVersion.n}`,
    originalUrl: (p) => `${base}/o/${p.id}?v=${p.mtime}`,
  };
}

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const api: Api = isTauri ? tauriApi() : (await import('./mock')).mockApi();
