<script lang="ts">
  import { api } from '../api';
  import { formatAgo, formatBytes, formatCount, plural } from '../format';
  import { store } from '../store.svelte';
  import type { CacheInfo, Settings } from '../types';
  import Icon from './Icon.svelte';

  let draft = $state<Settings>(structuredClone($state.snapshot(store.settings!)));
  let cache = $state<CacheInfo | null>(null);
  let newSource = $state('');
  let saving = $state(false);
  let found = $state<string[]>([]);

  api.cacheInfo().then((c) => (cache = c)).catch(() => {});
  api.suggestInboxes().then((s) => (found = s)).catch(() => {});

  // Deleting from a synced folder deletes everywhere: from iCloud Photos, off the iPhone too.
  const isCloud = (path: string) => /icloud|onedrive|dropbox|google ?drive/i.test(path);
  const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).slice(-2).join(' › ');
  const suggestions = $derived(found.filter((f) => !draft.inboxes.some((i) => i.path.toLowerCase() === f.toLowerCase())));

  function addInbox(path: string | null) {
    const p = path?.trim();
    if (!p || draft.inboxes.some((i) => i.path.toLowerCase() === p.toLowerCase())) return;
    draft.inboxes.push({ path: p, cleanup: false });
  }

  // Smart search, the graphics card, faces and pets have switches that apply
  // (and download, and start looking) right away: not part of what's saved.
  const live = (s: Settings | null) => ({ smartSearch: s?.smartSearch, smartGpu: s?.smartGpu, faces: s?.faces, pets: s?.pets, cacheVersion: s?.cacheVersion });
  const comparable = (s: Settings | null) =>
    JSON.stringify({ ...s, smartSearch: undefined, smartGpu: undefined, faces: undefined, pets: undefined, cacheVersion: undefined });
  const dirty = $derived(comparable(draft) !== comparable(store.settings));

  async function pick(key: 'importTo' | 'backupTo' | 'editorPath', title: string) {
    const path = await api.pickFolder(title);
    if (path) draft[key] = path;
  }

  async function save() {
    saving = true;
    const clean = (v: string | null) => (v?.trim() ? v.trim() : null);
    const next = {
      ...$state.snapshot(draft),
      inboxes: draft.inboxes.filter((i) => i.path.trim()).map((i) => ({ path: i.path.trim(), cleanup: i.cleanup && !isCloud(i.path) })),
      importTo: clean(draft.importTo),
      backupTo: clean(draft.backupTo),
      editorPath: clean(draft.editorPath),
      // As they are now, not as they were when this opened.
      ...live(store.settings),
    } as Settings;
    await store.saveSettings(next);
    draft = structuredClone(next);
    saving = false;
  }

  async function addSource() {
    if (!newSource.trim()) return;
    await store.addSource(newSource.trim());
    newSource = '';
  }

  async function rebuild() {
    await store.rebuildThumbnails();
    cache = await api.cacheInfo();
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
            <Icon name={s.offline ? 'offline' : s.path.startsWith('\\\\') ? 'drive' : 'folder'} size={16} />
            <div class="grow">
              <strong>{s.label}</strong>
              <span class="mono faint truncate">{s.path}</span>
              {#if s.offline}<small class="warn">Can't be reached right now. Photos browse from the cache until it's back.</small>{/if}
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
      <h3><Icon name="wifi" size={16} /> Camera and phone sync</h3>
      <p class="hint">
        Have the camera send shots to this PC over Wi-Fi (FUJIFILM <em>PC AutoSave</em>), and let <em>iCloud for Windows</em> download your
        iPhone photos. Fujicloud watches those folders and copies each new photo into your library, organised by date, checking every copy
        against the original.
      </p>
      <div class="flow">
        <div class="node"><Icon name="camera" size={18} /><span>Camera · iPhone</span></div>
        <span class="arrow">Wi-Fi</span>
        <div class="node"><Icon name="folder" size={18} /><span>Inboxes</span></div>
        <span class="arrow">LAN</span>
        <div class="node"><Icon name="drive" size={18} /><span>Shield SSD</span></div>
      </div>
      <div class="row top">
        <span>Inbox folders</span>
        <div class="grow inboxes">
          {#each draft.inboxes as inbox, i (inbox.path)}
            {@const cloud = isCloud(inbox.path)}
            <div class="inbox">
              <Icon name={cloud ? 'backup' : 'folder'} size={15} />
              <div class="grow">
                <span class="truncate" title={inbox.path}>{folderName(inbox.path)}</span>
                <small class="mono faint truncate">{inbox.path}</small>
              </div>
              <label
                class="cleanup"
                class:disabled={cloud}
                title={cloud
                  ? 'Not for synced folders: deleting from iCloud Photos deletes the photo from your iPhone too.'
                  : 'Once the library copy has been checked byte for byte, move the inbox file to the Recycle Bin.'}
              >
                <input type="checkbox" disabled={cloud} bind:checked={inbox.cleanup} />
                Free up space
              </label>
              <button class="icon-btn" title="Stop watching this folder" onclick={() => draft.inboxes.splice(i, 1)}>
                <Icon name="x" size={14} />
              </button>
            </div>
          {/each}
          <div class="inline wrap">
            <button class="btn" onclick={async () => addInbox(await api.pickFolder('Folder the camera or phone saves into'))}>
              <Icon name="plus" size={13} stroke={2} /> Add folder…
            </button>
            {#each suggestions as s (s)}
              <button class="btn ghost" title={s} onclick={() => addInbox(s)}>
                <Icon name="plus" size={13} stroke={2} /> {isCloud(s) ? 'iCloud Photos' : folderName(s)}
              </button>
            {/each}
          </div>
        </div>
      </div>
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
      {#if store.sync?.waiting}
        <p class="warn">
          <Icon name="offline" size={14} /> The library can't be reached. {store.sync.waiting} photo{store.sync.waiting === 1 ? '' : 's'} will be copied
          when it's back.
        </p>
      {:else if store.sync?.active}
        <p class="ok">
          <Icon name="check" size={14} stroke={2.4} /> Watching {store.sync.inboxes?.length === 1
            ? folderName(store.sync.inboxes[0])
            : `${store.sync.inboxes?.length ?? 0} folders`}
        </p>
      {/if}
    </section>

    <section>
      <h3><Icon name="backup" size={16} /> Backup</h3>
      <p class="hint">
        Keeps a second copy of the library on another drive, or in your <em>OneDrive</em> folder so it also goes to the cloud. New and
        changed photos and sidecars are copied and checked against the originals. Photos deleted from the library stay in the backup.
      </p>
      <label class="row">
        <span>Back up to</span>
        <div class="inline grow">
          <input class="field grow" placeholder="E:\Fujicloud Backup (off when empty)" bind:value={draft.backupTo} />
          <button class="btn" onclick={() => pick('backupTo', 'Folder to back the library up to')}>Browse…</button>
        </div>
      </label>
      {#if store.backup?.target}
        <div class="row">
          <div class="grow">
            <span>
              {#if store.backup.running}Backing up… {store.backup.total ? `${formatCount(store.backup.done)} of ${formatCount(store.backup.total)} files` : 'looking for changes'}
              {:else if store.backup.lastBackedUp}Last backed up {formatAgo(store.backup.lastBackedUp)}
              {:else}Not backed up yet{/if}
            </span>
            {#if store.backup.error && !store.backup.running}<small class="warn">{store.backup.error}</small>{/if}
          </div>
          <button class="btn" disabled={store.backup.running} onclick={() => api.backupNow()}>
            <Icon name="backup" size={14} /> Back up now
          </button>
        </div>
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
          <small class="faint">So stars and colour labels show up in Lightroom, Bridge and Camera Raw, and other PCs get your ratings, albums, names and what the models found (no need to look again). Existing develop settings are kept.</small>
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
            {:else if store.smart.phase === 'downloading'}Downloading model… {store.smart.total
                ? `${formatBytes(store.smart.done)} of ${formatBytes(store.smart.total)}`
                : store.smart.done
                  ? `${formatBytes(store.smart.done)} so far`
                  : 'connecting'}
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
      <h3><Icon name="people" size={16} /> People</h3>
      <p class="hint">
        Finds the faces in your photos and groups each person's photos, so you can name them once and find them anywhere. It runs entirely
        on this PC using InsightFace's SCRFD and ArcFace models, as Immich does (downloaded once, {store.faces?.downloadMb ?? 183} MB). Faces never leave your
        network.
      </p>
      <label class="row toggle">
        <div>
          <span>Recognise faces</span>
          <small class="faint">
            {#if !store.faces?.enabled}Off
            {:else if store.faces.phase === 'downloading'}Downloading models… {store.faces.total ? `${formatBytes(store.faces.done)} of ${formatBytes(store.faces.total)}` : 'connecting'}
            {:else if store.faces.phase === 'indexing'}Looking for faces… {formatCount(store.faces.done)} of {formatCount(store.faces.total)}
            {:else if store.faces.phase === 'error'}Problem: {store.faces.error}
            {:else}Ready · {formatCount(store.faces.scanned)} photos searched · {store.people.filter((p) => p.name).length === 1 ? '1 person' : `${store.people.filter((p) => p.name).length} people`} named{/if}
          </small>
        </div>
        <input type="checkbox" class="switch" checked={!!store.faces?.enabled} onchange={(e) => store.setFaces(e.currentTarget.checked)} />
      </label>
    </section>

    <section>
      <h3><Icon name="paw" size={16} /> Pets</h3>
      <p class="hint">
        Finds the cats and dogs in your photos and tells your pets apart, so you can name each one. It runs entirely on this PC, using
        OpenCV's YOLOX to find pets and Meta's DINOv2 to tell them apart (downloaded once, {store.petStatus?.downloadMb ?? 97} MB). Pets that
        look much alike (two black cats) may be grouped together; "Not this pet" on their photos sorts them out.
      </p>
      <label class="row toggle">
        <div>
          <span>Recognise pets</span>
          <small class="faint">
            {#if !store.petStatus?.enabled}Off
            {:else if store.petStatus.phase === 'downloading'}Downloading models… {store.petStatus.total
                ? `${formatBytes(store.petStatus.done)} of ${formatBytes(store.petStatus.total)}`
                : 'connecting'}
            {:else if store.petStatus.phase === 'indexing'}Looking for pets… {formatCount(store.petStatus.done)} of {formatCount(store.petStatus.total)}
            {:else if store.petStatus.phase === 'error'}Problem: {store.petStatus.error}
            {:else}Ready · {formatCount(store.petStatus.scanned)} photos searched · {plural(store.pets.filter((p) => p.name).length, 'pet')} named{/if}
          </small>
        </div>
        <input type="checkbox" class="switch" checked={!!store.petStatus?.enabled} onchange={(e) => store.setPets(e.currentTarget.checked)} />
      </label>
    </section>

    <section>
      <h3><Icon name="photos" size={16} /> Thumbnail cache</h3>
      <div class="row">
        <div class="grow">
          <span>{cache ? formatBytes(cache.bytes) : '…'} on this PC</span>
          <small class="faint mono truncate">{cache?.path ?? ''}</small>
        </div>
        <button class="btn" onclick={() => api.rescan()} title="Look for new, changed and removed photos">
          <Icon name="sync" size={14} /> Rescan library
        </button>
        <button class="btn" onclick={rebuild} title="Make every thumbnail and preview again, e.g. if some look wrong after an update">
          <Icon name="photos" size={14} /> Rebuild thumbnails
        </button>
      </div>
    </section>

    <section>
      <h3><Icon name="info" size={16} /> Help</h3>
      <div class="row">
        <div class="grow">
          <span>Something not working?</span>
          <small class="faint">The log records what Fujicloud did and any problems it ran into. Attach it when reporting an issue.</small>
        </div>
        <button class="btn" onclick={() => api.openLogFolder().catch((e) => store.toast(String(e), 'error'))}>
          <Icon name="folder" size={14} /> Open log folder
        </button>
      </div>
    </section>

    <section class="credits">
      <p class="faint">
        Place names © <a href="https://www.geonames.org" target="_blank" rel="noreferrer">GeoNames</a> (CC BY 4.0). Map © OpenStreetMap
        contributors, tiles by OpenFreeMap; borders from Natural Earth. Smart search uses Google's SigLIP model; People uses InsightFace's SCRFD and ArcFace models (for non-commercial use); Pets uses OpenCV's YOLOX and Meta's DINOv2 models.
      </p>
    </section>

    <section class="keys">
      <h3><Icon name="keyboard" size={16} /> Shortcuts</h3>
      <dl>
        <dt><span class="kbd">Space</span></dt><dd>Open / close photo</dd>
        <dt><span class="kbd">Ctrl</span> + click</dt><dd>Select several</dd>
        <dt>Drag a box</dt><dd>Select what it touches (<span class="kbd">Ctrl</span> adds)</dd>
        <dt>Drag selected</dt><dd>Out to Explorer or Photoshop (<span class="kbd">Alt</span>: any photo)</dd>
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
        <dt><span class="kbd">.</span></dt><dd>Favourite</dd>
        <dt><span class="kbd">C</span></dt><dd>Compare before / after</dd>
        <dt><span class="kbd">Ctrl</span> <span class="kbd">Z</span></dt><dd>Undo rating, flag, label</dd>
        <dt><span class="kbd">Del</span></dt><dd>Delete (Recycle Bin)</dd>
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
  p.warn {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 8px 0 0;
    font-size: 12px;
  }
  .row.top {
    align-items: flex-start;
  }
  .row.top > span:first-child {
    padding-top: 8px;
  }
  .inboxes {
    gap: 8px;
  }
  .inbox {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 6px 6px 12px;
    border-radius: 9px;
    background: rgba(255, 255, 255, 0.03);
    box-shadow: inset 0 0 0 0.5px var(--sep);
    color: var(--text-2);
  }
  .inbox .grow span {
    color: var(--text);
  }
  .cleanup {
    display: flex;
    align-items: center;
    gap: 6px;
    flex: none;
    font-size: 11.5px;
    white-space: nowrap;
  }
  .cleanup.disabled {
    opacity: 0.45;
  }
  .inline.wrap {
    flex-wrap: wrap;
  }
  .btn.ghost {
    background: transparent;
    box-shadow: inset 0 0 0 1px var(--border);
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
