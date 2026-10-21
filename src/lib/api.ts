import type { Bootstrap, CacheInfo, Detail, Hit, Label, Photo, RecipeRow, Settings, SmartStatus, Source } from './types';

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
  openInEditor(ids: number[]): Promise<string>;
  reveal(id: number): Promise<void>;
  openDefault(id: number): Promise<void>;
  saveSettings(settings: Settings): Promise<void>;
  cacheInfo(): Promise<CacheInfo>;
  clearCache(): Promise<void>;
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
    openInEditor: (ids) => invoke('open_in_editor', { ids }),
    reveal: (id) => invoke('reveal', { id }),
    openDefault: (id) => invoke('open_default', { id }),
    saveSettings: (settings) => invoke('save_settings', { settings }),
    cacheInfo: () => invoke('cache_info'),
    clearCache: () => invoke('clear_cache'),
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
        if (e.payload.type === 'drop') cb(e.payload.paths);
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
    thumbUrl: (p) => `${base}/t/${p.id}?v=${p.mtime}`,
    previewUrl: (p) => `${base}/p/${p.id}?v=${p.mtime}`,
    originalUrl: (p) => `${base}/o/${p.id}?v=${p.mtime}`,
  };
}

const isTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

export const api: Api = isTauri ? tauriApi() : (await import('./mock')).mockApi();
