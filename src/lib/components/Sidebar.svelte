<script lang="ts">
  import type { Snippet } from 'svelte';
  import { api } from '../api';
  import { filmGradient } from '../film';
  import { countryName, recipeSummary, recipeTitle } from '../recipe';
  import { formatAgo, formatBytes, formatCount } from '../format';
  import { store } from '../store.svelte';
  import type { Collection } from '../types';
  import ContextMenu, { type MenuItem } from './ContextMenu.svelte';
  import Icon, { type IconName } from './Icon.svelte';

  let open = $state({ recipes: true, films: true, countries: true, cameras: true, lenses: false });
  let moreRecipes = $state(false);
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const library: { c: Collection; label: string; icon: IconName; count: () => number | null }[] = [
    { c: { type: 'all' }, label: 'All Photos', icon: 'photos', count: () => store.counts.all },
    { c: { type: 'recent' }, label: 'Recently Added', icon: 'clock', count: () => store.counts.recent },
    { c: { type: 'favorites' }, label: 'Favourites', icon: 'heart', count: () => store.counts.favorites },
    { c: { type: 'map' }, label: 'Places', icon: 'globe', count: () => store.counts.places },
    { c: { type: 'picks' }, label: 'Picks', icon: 'flag', count: () => store.counts.picks },
    { c: { type: 'rated' }, label: 'Rated', icon: 'star', count: () => store.counts.rated },
    { c: { type: 'rejected' }, label: 'Rejected', icon: 'x', count: () => store.counts.rejected },
    {
      c: { type: 'duplicates' },
      label: 'Duplicates',
      icon: 'similar',
      // Known only once looked for: the extra copies, not counting keepers.
      count: () => (store.duplicates ? store.duplicates.reduce((n, g) => n + g.ids.length - 1, 0) : null),
    },
  ];

  function isActive(c: Collection) {
    const cur = store.collection;
    if (cur.type !== c.type) return false;
    if ('id' in c && 'id' in cur) return c.id === cur.id;
    if ('value' in c && 'value' in cur) return c.value === cur.value;
    if ('key' in c && 'key' in cur) return c.key === cur.key;
    return true;
  }

  async function addFolder() {
    const path = await api.pickFolder('Add a photo folder or network drive');
    if (path) store.addSource(path);
  }

  function sourceMenu(e: MouseEvent, id: number) {
    e.preventDefault();
    menu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        { label: 'Rescan', icon: 'sync', action: () => api.rescan() },
        { separator: true },
        { label: 'Remove from library', icon: 'trash', action: () => store.removeSource(id) },
      ],
    };
  }

  const syncing = $derived(!!store.sync?.copying);
  // Keeps "Backed up 5 min ago" current.
  let now = $state(Date.now() / 1000);
  $effect(() => {
    const timer = setInterval(() => (now = Date.now() / 1000), 60_000);
    return () => clearInterval(timer);
  });
  const waiting = $derived(store.sync?.waiting ?? 0);

  // Typing a new album's name, or renaming one (by id).
  let naming = $state<number | 'new' | null>(null);
  let draft = $state('');

  function startNaming(target: number | 'new', current = '') {
    naming = target;
    draft = current;
  }

  async function finishNaming(save: boolean) {
    const target = naming;
    naming = null;
    if (!save || !draft.trim() || target == null) return;
    if (target === 'new') {
      const id = await store.createAlbum(draft);
      if (id != null) store.setCollection({ type: 'album', id });
    } else {
      await store.renameAlbum(target, draft);
    }
  }

  function focusSelect(el: HTMLInputElement) {
    el.focus();
    el.select();
  }

  const named = $derived(store.people.filter((p) => p.name));
  const toName = $derived(store.people.filter((p) => !p.name && p.photoIds.length > 1).length);

  function personMenu(e: MouseEvent, id: number) {
    e.preventDefault();
    menu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        { label: 'All people', icon: 'people', action: () => store.setCollection({ type: 'people' }) },
        { label: 'Merge with…', icon: 'people', action: () => (store.mergePicker = { x: e.clientX, y: e.clientY, id }) },
        { separator: true },
        { label: 'Forget this name', icon: 'x', action: () => store.deletePerson(id) },
      ],
    };
  }

  function albumMenu(e: MouseEvent, id: number, name: string) {
    e.preventDefault();
    menu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        { label: 'Rename', icon: 'edit', action: () => startNaming(id, name) },
        { separator: true },
        { label: 'Delete album', icon: 'trash', action: () => store.deleteAlbum(id) },
      ],
    };
  }
</script>

{#snippet item(c: Collection, label: string, count: number | null, lead: Snippet)}
  <button class="item" class:active={isActive(c)} onclick={() => store.setCollection(c)}>
    {@render lead()}
    <span class="label truncate">{label}</span>
    {#if count != null}<span class="count tabular">{formatCount(count)}</span>{/if}
  </button>
{/snippet}

{#snippet nameInput()}
  <div class="item naming">
    <span class="ico"><Icon name="album" size={16} /></span>
    <input
      class="name"
      placeholder="Album name"
      bind:value={draft}
      use:focusSelect
      onkeydown={(e) => {
        if (e.key === 'Enter') finishNaming(true);
        if (e.key === 'Escape') finishNaming(false);
        e.stopPropagation();
      }}
      onblur={() => finishNaming(true)}
    />
  </div>
{/snippet}

{#snippet group(key: 'recipes' | 'films' | 'countries' | 'cameras' | 'lenses', title: string)}
  <button class="heading" onclick={() => (open[key] = !open[key])}>
    {title}
    <span class="chev" class:open={open[key]}><Icon name="next" size={11} stroke={2.4} /></span>
  </button>
{/snippet}

<nav class="sidebar">
  <div class="brand">
    <img src="/icon.svg" alt="" width="22" height="22" />
    <span>Fujicloud</span>
    {#if store.version}
      {@const build = store.version.split('.').at(-1)}
      <span class="version tabular" title="Version {store.version}{build !== '0' ? ` · Build #${build} on GitHub` : ''}">v{store.version}</span>
    {/if}
  </div>

  <div class="scroll">
    <div class="heading static">Library</div>
    {#each library as l (l.label)}
      {#snippet lead()}<span class="ico"><Icon name={l.icon} size={16} /></span>{/snippet}
      {@render item(l.c, l.label, l.count(), lead)}
    {/each}

    <div class="heading static">
      Sources
      <button class="add" title="Add folder or network drive" onclick={addFolder}><Icon name="plus" size={14} stroke={2} /></button>
    </div>
    {#each store.sources as s (s.id)}
      <div
        oncontextmenu={(e) => sourceMenu(e, s.id)}
        role="presentation"
        class:offline={s.offline}
        title={s.offline ? `${s.path}\nCan't be reached right now. Browsing from the cache; changes are saved when it's back.` : s.path}
      >
        {#snippet lead()}<span class="ico"><Icon name={s.offline ? 'offline' : s.path.startsWith('\\\\') ? 'drive' : 'folder'} size={16} /></span>{/snippet}
        {@render item({ type: 'source', id: s.id }, s.label, s.count, lead)}
      </div>
    {/each}
    {#if !store.sources.length}
      <button class="item ghost" onclick={addFolder}>
        <span class="ico"><Icon name="plus" size={16} /></span>
        <span class="label">Add a folder…</span>
      </button>
    {/if}
    <div class="heading static">
      Albums
      <button class="add" title="New album" onclick={() => startNaming('new')}><Icon name="plus" size={14} stroke={2} /></button>
    </div>
    {#each store.albums as a (a.id)}
      {#if naming === a.id}
        {@render nameInput()}
      {:else}
        <div oncontextmenu={(e) => albumMenu(e, a.id, a.name)} ondblclick={() => startNaming(a.id, a.name)} role="presentation">
          {#snippet lead()}<span class="ico"><Icon name="album" size={16} /></span>{/snippet}
          {@render item({ type: 'album', id: a.id }, a.name, a.ids.filter((id) => { const p = store.byId.get(id); return !!p && !store.folded(p); }).length, lead)}
        </div>
      {/if}
    {/each}
    {#if naming === 'new'}
      {@render nameInput()}
    {:else if !store.albums.length}
      <button class="item ghost" onclick={() => startNaming('new')}>
        <span class="ico"><Icon name="plus" size={16} /></span>
        <span class="label">New album…</span>
      </button>
    {/if}

    <div class="heading static">
      People
      {#if toName}<span class="to-name" title="Groups of faces waiting for a name">{toName} to name</span>{/if}
    </div>
    {#snippet peopleLead()}<span class="ico"><Icon name="people" size={16} /></span>{/snippet}
    {@render item({ type: 'people' }, store.faces?.enabled ? 'Everyone' : 'Find people…', store.faces?.enabled ? named.length : null, peopleLead)}
    {#each named.slice(0, 8) as p (p.id)}
      <div oncontextmenu={(e) => personMenu(e, p.id)} role="presentation">
        {#snippet lead()}<img class="avatar" src={api.faceUrl(p.cover)} alt="" draggable="false" />{/snippet}
        {@render item({ type: 'person', id: p.id }, p.name!, p.photoIds.length, lead)}
      </div>
    {/each}

    {#if store.recipes.length}
      {@render group('recipes', 'Recipes')}
      {#if open.recipes}
        {#each store.recipes.slice(0, moreRecipes ? 40 : 6) as r (r.key)}
          <button class="item recipe" class:active={isActive({ type: 'recipe', key: r.key })} title={recipeSummary(r.recipe)} onclick={() => store.setCollection({ type: 'recipe', key: r.key })}>
            <span class="swatch" style:background={filmGradient(r.recipe.filmSim)}></span>
            <span class="label two">
              <span class="truncate">{recipeTitle(r)}</span>
              <small class="truncate">{r.name ? r.recipe.filmSim : recipeSummary(r.recipe)}</small>
            </span>
            <span class="count tabular">{formatCount(r.count)}</span>
          </button>
        {/each}
        {#if store.recipes.length > 6}
          <button class="item ghost more" onclick={() => (moreRecipes = !moreRecipes)}>
            <span class="ico"></span><span class="label">{moreRecipes ? 'Show fewer' : `${store.recipes.length - 6} more recipes`}</span>
          </button>
        {/if}
      {/if}
    {/if}

    {#if store.facets.films.length}
      {@render group('films', 'Film Simulations')}
      {#if open.films}
        {#each store.facets.films as [name, n] (name)}
          {#snippet lead()}<span class="swatch" style:background={filmGradient(name)}></span>{/snippet}
          {@render item({ type: 'film', value: name }, name, n, lead)}
        {/each}
      {/if}
    {/if}

    {#if store.facets.countries.length}
      {@render group('countries', 'Countries')}
      {#if open.countries}
        {#each store.facets.countries as [code, n] (code)}
          {#snippet lead()}<span class="ico"><Icon name="pin" size={16} /></span>{/snippet}
          {@render item({ type: 'country', value: code }, countryName(code), n, lead)}
        {/each}
      {/if}
    {/if}

    {#if store.facets.cameras.length}
      {@render group('cameras', 'Cameras')}
      {#if open.cameras}
        {#each store.facets.cameras as [name, n] (name)}
          {#snippet lead()}<span class="ico"><Icon name="camera" size={16} /></span>{/snippet}
          {@render item({ type: 'camera', value: name }, name, n, lead)}
        {/each}
      {/if}
    {/if}

    {#if store.facets.lenses.length}
      {@render group('lenses', 'Lenses')}
      {#if open.lenses}
        {#each store.facets.lenses as [name, n] (name)}
          {#snippet lead()}<span class="ico"><Icon name="lens" size={16} /></span>{/snippet}
          {@render item({ type: 'lens', value: name }, name.replace(/^XF/, 'XF '), n, lead)}
        {/each}
      {/if}
    {/if}

  </div>

  <footer>
    {#if store.scan}
      <div class="status">
        <span class="spin"><Icon name="sync" size={14} /></span>
        <div class="grow">
          <div class="line">
            <span class="truncate">{store.scan.progress.phase === 'listing' ? 'Looking for photos' : 'Indexing'} · {store.scan.label}</span>
            {#if store.scan.progress.total}<span class="faint tabular">{formatCount(store.scan.progress.done)}/{formatCount(store.scan.progress.total)}</span>{/if}
          </div>
          <div class="bar"><i style:width="{store.scan.progress.total ? (store.scan.progress.done / store.scan.progress.total) * 100 : 8}%" class:indeterminate={!store.scan.progress.total}></i></div>
        </div>
      </div>
    {:else if store.exporting}
      <div class="status">
        <span class="ico small"><Icon name="export" size={14} /></span>
        <div class="grow">
          <div class="line"><span>Exporting</span><span class="faint tabular">{store.exporting.done} of {store.exporting.total}</span></div>
          <div class="bar"><i style:width="{(store.exporting.done / Math.max(1, store.exporting.total)) * 100}%"></i></div>
        </div>
      </div>
    {:else if store.warm}
      <div class="status">
        <span class="ico small"><Icon name="photos" size={14} /></span>
        <div class="grow">
          <div class="line"><span>Building thumbnails</span><span class="faint tabular">{Math.round((store.warm.done / store.warm.total) * 100)}%</span></div>
          <div class="bar"><i style:width="{(store.warm.done / store.warm.total) * 100}%"></i></div>
        </div>
      </div>
    {/if}

    {#if store.smart?.enabled && (store.smart.phase === 'downloading' || store.smart.phase === 'indexing' || store.smart.phase === 'loading')}
      <div
        class="status"
        title={store.smart.phase === 'downloading' && store.smart.total ? `${formatBytes(store.smart.done)} of ${formatBytes(store.smart.total)}` : undefined}
      >
        <span class="ico small sparkle"><Icon name="sparkle" size={14} /></span>
        <div class="grow">
          <div class="line">
            <span>{store.smart.phase === 'downloading' ? 'Downloading search model' : store.smart.phase === 'loading' ? 'Starting smart search' : store.smart.runningOn ? `Analysing on ${store.smart.runningOn}` : 'Analysing photos'}</span>
            {#if store.smart.total}<span class="faint tabular">{Math.round((store.smart.done / store.smart.total) * 100)}%</span>
            {:else if store.smart.phase === 'downloading' && store.smart.done}<span class="faint tabular">{formatBytes(store.smart.done)}</span>{/if}
          </div>
          <div class="bar"><i style:width="{store.smart.total ? (store.smart.done / store.smart.total) * 100 : 8}%" class:indeterminate={!store.smart.total}></i></div>
        </div>
      </div>
    {/if}

    {#if store.backup?.target}
      <button
        class="backup-line"
        class:warn={!!store.backup.error && !store.backup.running}
        title={store.backup.error ?? `Backing up to ${store.backup.target}`}
        onclick={() => (store.settingsOpen = true)}
      >
        <span class:pulse={store.backup.running}><Icon name="backup" size={13} /></span>
        <span class="truncate">
          {#if store.backup.running}Backing up{store.backup.total ? ` · ${formatCount(store.backup.done)} of ${formatCount(store.backup.total)}` : '…'}
          {:else if store.backup.lastBackedUp}Backed up {formatAgo(store.backup.lastBackedUp, now)}{store.backup.error ? ' · needs attention' : ''}
          {:else}Not backed up yet{/if}
        </span>
      </button>
    {/if}
    <button class="sync" class:on={store.sync?.active} onclick={() => (store.settingsOpen = true)}>
      <span class="sync-ico" class:pulse={syncing} class:waiting={waiting && !syncing}><Icon name={waiting && !syncing ? 'offline' : 'wifi'} size={15} /></span>
      <div class="grow">
        <strong>{store.sync?.active ? (syncing ? 'Importing new photos' : waiting ? 'Waiting for the library' : 'Sync on') : 'Sync off'}</strong>
        <span class="faint truncate">
          {#if syncing}{store.sync?.done ?? 0} of {store.sync?.total ?? 0} copied to library
          {:else if waiting}{waiting} photo{waiting === 1 ? '' : 's'} queued
          {:else if store.sync?.active}Watching for new shots
          {:else}Set up in Settings{/if}
        </span>
      </div>
      <Icon name="gear" size={15} />
    </button>
  </footer>
</nav>

{#if menu}
  <ContextMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    width: 236px;
    flex: none;
    height: 100%;
    background: var(--sidebar);
    border-right: 1px solid var(--sep);
  }
  .brand {
    display: flex;
    align-items: center;
    gap: 9px;
    height: 52px;
    padding: 0 18px;
    font: 650 14.5px var(--font-display);
    letter-spacing: -0.01em;
    flex: none;
  }
  .version {
    margin-left: auto;
    padding: 1px 6px;
    border-radius: 5px;
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-3);
    font: 500 10.5px var(--font);
    letter-spacing: 0;
  }
  .scroll {
    flex: 1;
    overflow-y: auto;
    padding: 0 10px 12px;
  }
  .heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    width: 100%;
    margin: 16px 0 4px;
    padding: 0 8px;
    height: 22px;
    font-size: 11px;
    font-weight: 650;
    color: var(--text-3);
    letter-spacing: 0.02em;
  }
  .heading.static:first-child {
    margin-top: 2px;
  }
  button.heading:hover {
    color: var(--text-2);
  }
  .chev {
    display: grid;
    transition: transform 160ms var(--ease-out);
  }
  .chev.open {
    transform: rotate(90deg);
  }
  .add {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 5px;
    color: var(--text-2);
  }
  .add:hover {
    background: var(--hover);
    color: var(--text);
  }
  .item {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border-radius: 7px;
    text-align: left;
    color: var(--text);
    transition: background 100ms;
  }
  .item:hover {
    background: var(--hover);
  }
  .item.naming:hover {
    background: none;
  }
  .name {
    flex: 1;
    min-width: 0;
    height: 24px;
    padding: 0 7px;
    border-radius: 6px;
    background: var(--bg-input);
    box-shadow: inset 0 0 0 1px var(--accent);
    color: var(--text);
    font: inherit;
    outline: none;
  }
  .item.active {
    background: var(--active);
  }
  .item.active .ico {
    color: var(--accent);
  }
  .item.ghost {
    color: var(--text-2);
  }
  .ico {
    display: grid;
    place-items: center;
    width: 18px;
    color: var(--text-2);
    flex: none;
  }
  .swatch {
    width: 14px;
    height: 14px;
    margin: 0 2px;
    border-radius: 4px;
    flex: none;
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.25);
  }
  .label {
    flex: 1;
    min-width: 0;
  }
  .label.two {
    display: flex;
    flex-direction: column;
    line-height: 1.2;
  }
  .label.two small {
    color: var(--text-3);
    font-size: 10.5px;
  }
  .item.recipe {
    height: 38px;
  }
  .item.more {
    height: 26px;
    font-size: 12px;
  }
  .sparkle {
    color: #bf9bff;
  }
  .count {
    font-size: 11.5px;
    color: var(--text-3);
  }

  footer {
    flex: none;
    padding: 8px 10px 10px;
    border-top: 1px solid var(--sep);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .status {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 8px;
    font-size: 11.5px;
  }
  .grow {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
  }
  .line {
    display: flex;
    justify-content: space-between;
    gap: 8px;
  }
  .bar {
    height: 3px;
    margin-top: 5px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.1);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 300ms var(--ease);
  }
  .bar i.indeterminate {
    width: 30% !important;
    animation: slide 1.1s ease-in-out infinite;
  }
  @keyframes slide {
    from {
      transform: translateX(-100%);
    }
    to {
      transform: translateX(340%);
    }
  }
  .spin {
    display: grid;
    color: var(--accent);
    animation: spin 1.2s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .avatar {
    width: 18px;
    height: 18px;
    margin: 0 1px;
    flex: none;
    border-radius: 50%;
    object-fit: cover;
    box-shadow: 0 0 0 0.5px var(--border);
  }
  .to-name {
    margin-left: auto;
    padding: 0 6px;
    border-radius: 8px;
    background: color-mix(in srgb, var(--accent) 22%, transparent);
    color: var(--text);
    font-size: 10.5px;
    font-weight: 600;
    letter-spacing: 0;
    text-transform: none;
  }
  .backup-line {
    display: flex;
    align-items: center;
    gap: 7px;
    width: 100%;
    margin-bottom: 4px;
    padding: 3px 10px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text-3);
    font: inherit;
    font-size: 11.5px;
    text-align: left;
    cursor: default;
  }
  .backup-line:hover {
    background: rgba(255, 255, 255, 0.05);
    color: var(--text-2);
  }
  .backup-line.warn {
    color: #ffb340;
  }
  .sync {
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    padding: 8px;
    border-radius: 9px;
    text-align: left;
    color: var(--text-3);
    background: rgba(255, 255, 255, 0.035);
    box-shadow: inset 0 0 0 0.5px var(--sep);
  }
  .sync:hover {
    background: var(--hover);
    color: var(--text-2);
  }
  .sync strong {
    font-size: 12px;
    font-weight: 600;
    color: var(--text-2);
  }
  .sync span {
    font-size: 11px;
  }
  .sync-ico {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.06);
    flex: none;
  }
  .sync.on .sync-ico {
    color: var(--pick);
    background: rgba(48, 209, 88, 0.14);
  }
  .sync.on .sync-ico.waiting {
    color: #ffb340;
    background: rgba(255, 179, 64, 0.14);
  }
  .offline :global(.item) {
    opacity: 0.62;
  }
  .offline :global(.ico) {
    color: #ffb340;
  }
  .sync.on strong {
    color: var(--text);
  }
  .pulse {
    animation: pulse 1.2s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.45;
    }
  }
</style>
