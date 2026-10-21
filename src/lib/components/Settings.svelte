<script lang="ts">
  import { api } from '../api';
  import { formatBytes, formatCount } from '../format';
  import { store } from '../store.svelte';
  import type { CacheInfo, Settings } from '../types';
  import Icon from './Icon.svelte';

  let draft = $state<Settings>({ ...store.settings! });
  let cache = $state<CacheInfo | null>(null);
  let newSource = $state('');
  let saving = $state(false);

  api.cacheInfo().then((c) => (cache = c)).catch(() => {});

  // Smart search has its own switch that applies immediately; ignore it here.
  const comparable = (s: Settings | null) => JSON.stringify({ ...s, smartSearch: undefined });
  const dirty = $derived(comparable(draft) !== comparable(store.settings));

  async function pick(key: 'inbox' | 'importTo' | 'editorPath', title: string) {
    const path = await api.pickFolder(title);
    if (path) draft[key] = path;
  }

  async function save() {
    saving = true;
    const clean = (v: string | null) => (v?.trim() ? v.trim() : null);
    const next = {
      ...$state.snapshot(draft),
      inbox: clean(draft.inbox),
      importTo: clean(draft.importTo),
      editorPath: clean(draft.editorPath),
      smartSearch: store.settings?.smartSearch ?? false,
    };
    await store.saveSettings(next);
    draft = { ...next };
    saving = false;
  }

  async function addSource() {
    if (!newSource.trim()) return;
    await store.addSource(newSource.trim());
    newSource = '';
  }

  async function clearCache() {
    await api.clearCache();
    cache = await api.cacheInfo();
    store.toast('Thumbnail cache cleared. It rebuilds as you browse.');
  }

  function close() {
    store.settingsOpen = false;
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape') close();
  }}
/>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="sheet" role="dialog" aria-modal="true" aria-labelledby="settings-title">
  <header>
    <h2 id="settings-title">Settings</h2>
    <button class="icon-btn" aria-label="Close" onclick={close}><Icon name="x" size={16} /></button>
  </header>

  <div class="body">
    <section>
      <h3><Icon name="drive" size={16} /> Library</h3>
      <p class="hint">
        Folders Fujicloud keeps indexed. For the SSD on your NVIDIA Shield, turn on <em>Settings → Device Preferences → Storage →
        Transfer files over local network</em> on the Shield, then add it as <code>\\SHIELD\&lt;drive name&gt;</code>.
      </p>
      <ul class="sources">
        {#each store.sources as s (s.id)}
          <li>
            <Icon name={s.path.startsWith('\\\\') ? 'drive' : 'folder'} size={16} />
            <div class="grow">
              <strong>{s.label}</strong>
              <span class="mono faint truncate">{s.path}</span>
            </div>
            <span class="faint tabular">{formatCount(s.count)} files</span>
            <button class="icon-btn" title="Remove from library (files are not touched)" onclick={() => store.removeSource(s.id)}>
              <Icon name="trash" size={15} />
            </button>
          </li>
        {/each}
      </ul>
      <div class="inline">
        <input class="field grow" placeholder="\\SHIELD\Samsung_T7\Photos" bind:value={newSource} onkeydown={(e) => e.key === 'Enter' && addSource()} />
        <button class="btn" onclick={addSource} disabled={!newSource.trim()}>Add path</button>
        <button class="btn" onclick={async () => { const p = await api.pickFolder('Add a photo folder'); if (p) store.addSource(p); }}>Browse…</button>
      </div>
    </section>

    <section>
      <h3><Icon name="wifi" size={16} /> Camera sync</h3>
      <p class="hint">
        Have the camera send shots to this PC over Wi-Fi (FUJIFILM <em>PC AutoSave</em>, or any app that saves to a folder). Fujicloud
        watches that folder and copies each new photo into your library, organised by date, several files at a time.
      </p>
      <div class="flow">
        <div class="node"><Icon name="camera" size={18} /><span>Camera</span></div>
        <span class="arrow">Wi-Fi</span>
        <div class="node"><Icon name="folder" size={18} /><span>Inbox</span></div>
        <span class="arrow">LAN</span>
        <div class="node"><Icon name="drive" size={18} /><span>Shield SSD</span></div>
      </div>
      <label class="row">
        <span>Inbox folder</span>
        <div class="inline grow">
          <input class="field grow" placeholder="C:\Users\you\Pictures\FUJIFILM PC AutoSave" bind:value={draft.inbox} />
          <button class="btn" onclick={() => pick('inbox', 'Folder the camera transfers into')}>Browse…</button>
        </div>
      </label>
      <label class="row">
        <span>Copy new photos to</span>
        <div class="inline grow">
          <input class="field grow" placeholder="\\SHIELD\Samsung_T7\Photos" bind:value={draft.importTo} />
          <button class="btn" onclick={() => pick('importTo', 'Library folder for new photos')}>Browse…</button>
        </div>
      </label>
      <label class="row">
        <span>Parallel copies</span>
        <div class="inline grow">
          <input type="range" min="1" max="8" bind:value={draft.parallelCopies} class="grow" />
          <span class="tabular value">{draft.parallelCopies}</span>
        </div>
      </label>
      {#if store.sync?.active}
        <p class="ok"><Icon name="check" size={14} stroke={2.4} /> Watching {store.sync.inbox}</p>
      {/if}
    </section>

    <section>
      <h3><Icon name="edit" size={16} /> Editing</h3>
      <label class="row">
        <span>Editor</span>
        <div class="inline grow">
          <input class="field grow" placeholder={store.editor ?? 'Photoshop not found. Paste the path to Photoshop.exe'} bind:value={draft.editorPath} />
        </div>
      </label>
      <label class="row toggle">
        <div>
          <span>Open the RAW for RAW+JPEG pairs</span>
          <small class="faint">Photoshop opens RAFs in Camera Raw.</small>
        </div>
        <input type="checkbox" class="switch" bind:checked={draft.editRaw} />
      </label>
      <label class="row toggle">
        <div>
          <span>Write ratings to XMP sidecars</span>
          <small class="faint">So stars and colour labels show up in Lightroom, Bridge and Camera Raw. Existing develop settings are kept.</small>
        </div>
        <input type="checkbox" class="switch" bind:checked={draft.writeXmp} />
      </label>
    </section>

    <section>
      <h3><Icon name="sparkle" size={16} /> Smart search</h3>
      <p class="hint">
        Find photos by describing them, like <em>“golden hour at the beach”</em> or <em>“dog in the snow”</em>, and right-click any photo to
        find similar ones. Uses Google's SigLIP model, downloaded once ({store.smart?.downloadMb ?? 210} MB). It runs entirely on this PC, so
        your photos never leave your network.
      </p>
      <label class="row toggle">
        <div>
          <span>Enable smart search</span>
          <small class="faint">
            {#if !store.smart?.enabled}Off
            {:else if store.smart.phase === 'downloading'}Downloading model… {store.smart.total ? `${formatBytes(store.smart.done)} of ${formatBytes(store.smart.total)}` : ''}
            {:else if store.smart.phase === 'loading'}Starting…
            {:else if store.smart.phase === 'indexing'}Analysing photos{store.smart.runningOn ? ` on the ${store.smart.runningOn}` : ''}… {formatCount(store.smart.done)} of {formatCount(store.smart.total)}
            {:else if store.smart.phase === 'error'}Problem: {store.smart.error}
            {:else}Ready · {formatCount(store.smart.indexed)} photos analysed{/if}
          </small>
        </div>
        <input type="checkbox" class="switch" checked={!!store.smart?.enabled} onchange={(e) => store.setSmartSearch(e.currentTarget.checked)} />
      </label>
      {#if store.smart?.enabled && store.smart.total && store.smart.phase !== 'ready'}
        <div class="progress"><i style:width="{(store.smart.done / store.smart.total) * 100}%"></i></div>
      {/if}
      <label class="row toggle" class:disabled={!store.smart?.gpu}>
        <div>
          <span>Use the graphics card</span>
          <small class="faint">
            {#if store.smart?.gpu}
              {store.smart.gpu.name}{store.smart.gpu.vramMb >= 1024 ? ` · ${Math.round(store.smart.gpu.vramMb / 1024)} GB` : ''}. Analyses photos many
              times faster than the CPU (DirectML, works with NVIDIA, AMD and Intel).
            {:else}
              No DirectX 12 graphics card found, so photos are analysed on the CPU.
            {/if}
          </small>
          {#if store.smart?.gpuError}
            <small class="warn">The GPU couldn't start, so the CPU is used instead: {store.smart.gpuError}</small>
          {/if}
        </div>
        <input
          type="checkbox"
          class="switch"
          disabled={!store.smart?.gpu}
          checked={!!store.smart?.gpu && !!store.smart?.useGpu}
          onchange={(e) => store.setSmartGpu(e.currentTarget.checked)}
        />
      </label>
    </section>

    <section>
      <h3><Icon name="photos" size={16} /> Thumbnail cache</h3>
      <div class="row">
        <div class="grow">
          <span>{cache ? formatBytes(cache.bytes) : '…'} on this PC</span>
          <small class="faint mono truncate">{cache?.path ?? ''}</small>
        </div>
        <button class="btn" onclick={() => api.rescan()}><Icon name="sync" size={14} /> Rescan library</button>
        <button class="btn danger" onclick={clearCache}>Clear</button>
      </div>
    </section>

    <section class="credits">
      <p class="faint">
        Place names © <a href="https://www.geonames.org" target="_blank" rel="noreferrer">GeoNames</a> (CC BY 4.0). Map © OpenStreetMap
        contributors, tiles by OpenFreeMap; borders from Natural Earth. Smart search uses Google's SigLIP model.
      </p>
    </section>

    <section class="keys">
      <h3><Icon name="keyboard" size={16} /> Shortcuts</h3>
      <dl>
        <dt><span class="kbd">Space</span></dt><dd>Open / close photo</dd>
        <dt><span class="kbd">Ctrl</span> + click</dt><dd>Select several</dd>
        <dt><span class="kbd">←</span> <span class="kbd">→</span></dt><dd>Previous / next</dd>
        <dt><span class="kbd">1</span>–<span class="kbd">5</span></dt><dd>Rate (<span class="kbd">0</span> clears)</dd>
        <dt><span class="kbd">P</span> <span class="kbd">X</span> <span class="kbd">U</span></dt><dd>Pick / reject / unflag</dd>
        <dt><span class="kbd">6</span>–<span class="kbd">9</span></dt><dd>Red, yellow, green, blue label</dd>
        <dt><span class="kbd">Z</span></dt><dd>Zoom to 100%</dd>
        <dt><span class="kbd">Ctrl</span> <span class="kbd">E</span></dt><dd>Edit in Photoshop</dd>
        <dt><span class="kbd">I</span></dt><dd>Info panel</dd>
        <dt><span class="kbd">F</span></dt><dd>Full screen viewer</dd>
        <dt><span class="kbd">Ctrl</span> <span class="kbd">F</span></dt><dd>Search</dd>
        <dt><span class="kbd">Ctrl</span> + scroll</dt><dd>Thumbnail size</dd>
      </dl>
    </section>
  </div>

  <footer>
    <button class="btn subtle" onclick={close}>Close</button>
    <button class="btn primary" disabled={!dirty || saving} onclick={save}>{saving ? 'Saving…' : 'Save changes'}</button>
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 70;
    background: rgba(0, 0, 0, 0.45);
    animation: fade 180ms ease-out;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .sheet {
    position: fixed;
    z-index: 71;
    top: 50%;
    left: 50%;
    width: min(680px, calc(100vw - 48px));
    max-height: min(820px, calc(100vh - 48px));
    display: flex;
    flex-direction: column;
    transform: translate(-50%, -50%);
    border-radius: var(--radius-lg);
    background: #1e1e21;
    box-shadow: var(--shadow-lg);
    animation: rise 260ms var(--ease-out);
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, -48%) scale(0.98);
    }
  }
  header,
  footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 14px 16px 12px 22px;
    flex: none;
  }
  header {
    border-bottom: 1px solid var(--sep);
  }
  footer {
    justify-content: flex-end;
    gap: 8px;
    border-top: 1px solid var(--sep);
    padding: 12px 16px;
  }
  h2 {
    margin: 0;
    font: 650 17px var(--font-display);
  }
  .body {
    overflow-y: auto;
    padding: 4px 22px 10px;
  }
  section {
    padding: 16px 0;
    border-bottom: 1px solid var(--sep);
  }
  section:last-child {
    border: 0;
  }
  h3 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 8px;
    font: 600 13.5px var(--font-display);
  }
  h3 :global(svg) {
    color: var(--accent);
  }
  .hint {
    margin: 0 0 12px;
    color: var(--text-2);
    font-size: 12.5px;
    line-height: 1.5;
  }
  em {
    font-style: normal;
    color: var(--text);
  }
  code,
  .mono {
    font-family: var(--mono);
    font-size: 11.5px;
  }
  code {
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--bg-elev-2);
    color: var(--text);
  }
  .sources {
    list-style: none;
    margin: 0 0 10px;
    padding: 0;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 0.5px var(--sep);
  }
  .sources:empty {
    display: none;
  }
  .sources li {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 8px 8px 14px;
    color: var(--text-2);
  }
  .sources li + li {
    border-top: 1px solid var(--sep);
  }
  .grow {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .grow strong {
    color: var(--text);
    font-weight: 550;
  }
  .inline {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-direction: row;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 6px 0;
  }
  .row > span:first-child {
    width: 140px;
    flex: none;
    color: var(--text-2);
  }
  .row.toggle {
    justify-content: space-between;
  }
  .row.toggle div {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  small {
    font-size: 11.5px;
  }
  .value {
    width: 18px;
    text-align: right;
  }
  .flow {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    margin: 4px 0 14px;
    padding: 14px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 0.5px var(--sep);
  }
  .node {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 5px;
    width: 86px;
    font-size: 11.5px;
    color: var(--text-2);
  }
  .node :global(svg) {
    width: 34px;
    height: 34px;
    padding: 8px;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.08);
    box-shadow: inset 0 0 0 0.5px var(--border);
    color: var(--text);
  }
  .arrow {
    flex: 1;
    max-width: 90px;
    position: relative;
    text-align: center;
    font-size: 10.5px;
    color: var(--text-3);
    padding-bottom: 6px;
    margin-bottom: 18px;
    border-bottom: 1.5px dashed rgba(255, 255, 255, 0.2);
  }
  .row.disabled {
    opacity: 0.6;
  }
  .warn {
    color: #ffb340;
  }
  .progress {
    height: 4px;
    margin-top: 4px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }
  .progress i {
    display: block;
    height: 100%;
    background: #bf9bff;
    transition: width 300ms var(--ease);
  }
  .ok {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px 0 0;
    color: var(--pick);
    font-size: 12px;
  }
  .credits p {
    margin: 0;
    font-size: 11.5px;
    line-height: 1.5;
  }
  .credits a {
    color: var(--text-2);
  }
  .keys dl {
    display: grid;
    grid-template-columns: 130px 1fr 130px 1fr;
    gap: 8px 12px;
    margin: 0;
    font-size: 12px;
    color: var(--text-2);
  }
  .keys dd {
    margin: 0;
  }
</style>
