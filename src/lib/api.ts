import type { Chunk } from './stream';
import { cacheVersion } from './version.svelte';
import type { Album, BackupStatus, Bootstrap, CacheInfo, Deleted, Detail, DuplicateGroup, Exported, ExportPreset, FaceInPhoto, FacesStatus, Hit, Health, Label, LogEntry, ModelChoice, Models, Person, PetsStatus, Photo, RecipeRow, Settings, SmartStatus, Source, Subject, TextStatus, SyncChange, SyncResolved, Trash, TrashItem } from './types';

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
  setReadText(enabled: boolean): Promise<TextStatus>;
  /** Photos with every word of the query written in them. */
  searchText(query: string): Promise<number[]>;
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
  /** Copies of the same file, and of the same picture saved again. */
  findDuplicates(): Promise<DuplicateGroup[]>;
  /** What's in the library folders' trash. */
  listTrash(): Promise<Trash>;
  /** Puts trashed photos back: how many, and the names of those that couldn't be. */
  restoreTrash(ns: number[]): Promise<[number, string[]]>;
  /** Deletes for good: these, or with none given everything in the trash. */
  emptyTrash(ns?: number[]): Promise<number>;
  trashThumbUrl(item: TrashItem): string;
  /** Changes other PCs made, waiting to be applied or rejected. */
  syncReview(): Promise<{ changes: SyncChange[]; total: number }>;
  /** With `everything`, takes every change waiting, the ones not listed too. */
  resolveSyncReview(apply: number[], reject: number[], everything?: boolean): Promise<SyncResolved>;
  /** The thumbnail of a photo by id (one that may be out of the grid). */
  thumbUrlById(id: number): string;
  /** Moves photos (with their twins and sidecars) to the Recycle Bin. */
  deletePhotos(ids: number[]): Promise<Deleted>;
  /** Exports copies into `folder`; progress arrives as `export` events. */
  exportPhotos(ids: number[], preset: ExportPreset, location: boolean, folder: string): Promise<Exported>;
  /** Starts dragging the photos' files out of the window, into another app. */
  /** Gets a drag ready as the button goes down, so it starts the moment the pointer moves. */
  dragPrepare(ids: number[]): void;
  /** Starts dragging these photos' files out; `held` says whether the button is still down. */
  dragOut(ids: number[], held?: () => boolean): Promise<void>;
  openFolder(path: string): Promise<void>;
  /** Converts a movie the window can't play; `convertedUrl` then serves it. */
  prepareMovie(id: number): Promise<void>;
  /**
   * Gets a movie the window can't play going at once: `converted` (it's
   * been converted before; `convertedUrl` serves it) or `streaming` (it's
   * being converted now; `movieChunk` hands over what's made so far).
   */
  playMovie(id: number): Promise<'converted' | 'streaming'>;
  movieChunk(id: number, offset: number): Promise<Chunk>;
  /** The viewer has moved on from a movie it was playing as it converted. */
  leaveMovie(id: number): void;
  /** Fetches the movie converter ahead of time, if it isn't here yet. */
  fetchConverter(): Promise<void>;
  convertedUrl(p: Photo): string;
  openInEditor(ids: number[]): Promise<string>;
  reveal(id: number): Promise<void>;
  openDefault(id: number): Promise<void>;
  saveSettings(settings: Settings): Promise<void>;
  backupStatus(): Promise<BackupStatus>;
  backupNow(): Promise<void>;
  /** Reads every photo and its backup copy back and compares them (in the background). */
  checkBackup(): Promise<void>;
  /** How the library is doing; with `deep`, the database is read back too. */
  libraryHealth(deep: boolean): Promise<Health>;
  /** Tries again what was owed or failed: sidecars, thumbnails, analysis. */
  retryFailed(): Promise<void>;
  /** Inbox folders found on this PC (PC AutoSave, iCloud Photos). */
  suggestInboxes(): Promise<string[]>;
  cacheInfo(): Promise<CacheInfo>;
  /** Rebuilds every thumbnail and preview; returns the new cache version. */
  rebuildThumbnails(): Promise<number>;
  openLogFolder(): Promise<void>;
  /** The log's latest warnings and problems, newest first. */
  recentProblems(): Promise<LogEntry[]>;
  listRecipes(): Promise<RecipeRow[]>;
  nameRecipe(key: string, name: string | null): Promise<void>;
  smartStatus(): Promise<SmartStatus>;
  setSmartSearch(enabled: boolean): Promise<SmartStatus>;
  setSmartGpu(enabled: boolean): Promise<SmartStatus>;
  /** Chooses how big a model each analysis uses; a change analyses photos again. */
  setModels(choice: ModelChoice): Promise<Models>;
  searchSmart(query: string): Promise<Hit[]>;
  similarPhotos(id: number): Promise<Hit[]>;
  pickFolder(title: string): Promise<string | null>;
  listen<T>(event: string, cb: (payload: T) => void): Promise<() => void>;
  /** `hover` hears when files from outside are over the window, and when they've gone. */
  onDropPaths(cb: (paths: string[]) => void, hover?: (over: boolean) => void): Promise<() => void>;
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
  type DragFiles = { paths: string[]; icon: string };
  /** A drag got ready while the button was going down. */
  let prepared: { key: string; files: Promise<DragFiles> } | null = null;
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
    setReadText: (enabled) => invoke('set_read_text', { enabled }),
    searchText: (query) => invoke('search_text', { query }),
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
    listTrash: () => invoke('list_trash'),
    restoreTrash: (ns) => invoke('restore_trash', { ns }),
    emptyTrash: (ns) => invoke('empty_trash', { ns: ns ?? null }),
    trashThumbUrl: (t) => `${base}/x/${t.n}?v=${encodeURIComponent(t.name)}.${t.size}`,
    syncReview: () => invoke('sync_review'),
    resolveSyncReview: (apply, reject, everything = false) => invoke('resolve_sync_review', { apply, reject, everything }),
    deletePhotos: (ids) => invoke('delete_photos', { ids }),
    exportPhotos: (ids, preset, location, folder) => invoke('export_photos', { ids, preset, location, folder }),
    dragPrepare(ids) {
      const key = ids.join(',');
      if (prepared?.key !== key) {
        prepared = { key, files: invoke<DragFiles>('drag_files', { ids }) };
        prepared.files.catch(() => {});
      }
      void import('@crabnebula/tauri-plugin-drag');
    },
    async dragOut(ids, held = () => true) {
      const key = ids.join(',');
      const ready = prepared?.key === key ? prepared.files : invoke<DragFiles>('drag_files', { ids });
      prepared = null;
      const [files, { startDrag }] = await Promise.all([ready, import('@crabnebula/tauri-plugin-drag')]);
      // Let go before it was ready: a drag started now would stick to the pointer.
      if (!held()) return;
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
    playMovie: (id) => invoke('play_movie', { id }),
    movieChunk: async (id, offset) => {
      const all = new Uint8Array(await invoke<ArrayBuffer>('movie_chunk', { id, offset }));
      return all.length ? { state: all[0], bytes: all.subarray(1) } : { state: 2, bytes: all };
    },
    leaveMovie: (id) => void invoke('leave_movie', { id }).catch(() => {}),
    fetchConverter: () => invoke('fetch_converter'),
    convertedUrl: (p) => `${base}/v/${p.id}?v=${p.mtime}`,
    openInEditor: (ids) => invoke('open_in_editor', { ids }),
    reveal: (id) => invoke('reveal', { id }),
    openDefault: (id) => invoke('open_default', { id }),
    saveSettings: (settings) => invoke('save_settings', { settings }),
    suggestInboxes: () => invoke('suggest_inboxes'),
    backupStatus: () => invoke('backup_status'),
    backupNow: () => invoke('backup_now'),
    checkBackup: () => invoke('check_backup'),
    libraryHealth: (deep) => invoke('library_health', { deep }),
    retryFailed: () => invoke('retry_failed'),
    cacheInfo: () => invoke('cache_info'),
    rebuildThumbnails: () => invoke('rebuild_thumbnails'),
    openLogFolder: () => invoke('open_log_folder'),
    recentProblems: () => invoke('recent_problems'),
    listRecipes: () => invoke('list_recipes'),
    nameRecipe: (key, name) => invoke('name_recipe', { key, name }),
    smartStatus: () => invoke('smart_status'),
    setSmartSearch: (enabled) => invoke('set_smart_search', { enabled }),
    setSmartGpu: (enabled) => invoke('set_smart_gpu', { enabled }),
    setModels: (choice) => invoke('set_models', { choice }),
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
    async onDropPaths(cb, hover) {
      const { getCurrentWebview } = await import('@tauri-apps/api/webview');
      return getCurrentWebview().onDragDropEvent((e) => {
        // Photos dragged out and let go over this window aren't new folders.
        const ours = (paths: string[]) => paths.length > 0 && paths.every((p) => outgoing.has(p));
        if (e.payload.type === 'enter') {
          if (!ours(e.payload.paths) && !outgoing.size) hover?.(true);
        } else if (e.payload.type === 'leave') {
          hover?.(false);
        } else if (e.payload.type === 'drop') {
          hover?.(false);
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
    thumbUrlById: (id) => `${base}/t/${id}?v=review.${cacheVersion.n}`,
    originalUrl: (p) => `${base}/o/${p.id}?v=${p.mtime}`,
  };
}

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const api: Api = isTauri ? tauriApi() : (await import('./mock')).mockApi();
