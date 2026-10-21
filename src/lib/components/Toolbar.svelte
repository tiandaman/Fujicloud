<script lang="ts">
  import { formatCount, plural } from '../format';
  import { store } from '../store.svelte';
  import type { GroupKey, Kind, SortKey } from '../types';
  import ContextMenu, { type MenuItem } from './ContextMenu.svelte';
  import Icon from './Icon.svelte';

  let search: HTMLInputElement;
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const sorts: [SortKey, string][] = [
    ['taken', 'Date taken'],
    ['added', 'Date added'],
    ['name', 'File name'],
    ['rating', 'Rating'],
    ['film', 'Film simulation'],
    ['camera', 'Camera'],
    ['focal', 'Focal length'],
    ['aperture', 'Aperture'],
    ['shutter', 'Shutter speed'],
    ['iso', 'ISO'],
    ['size', 'File size'],
  ];
  const groups: [GroupKey, string][] = [
    ['day', 'Day'],
    ['month', 'Month'],
    ['year', 'Year'],
    ['film', 'Film simulation'],
    ['recipe', 'Recipe'],
    ['place', 'Place'],
    ['camera', 'Camera'],
    ['lens', 'Lens'],
    ['folder', 'Folder'],
    ['none', 'No grouping'],
  ];
  const kinds: [Kind, string][] = [
    ['jpeg', 'JPEG'],
    ['raf', 'RAW (RAF)'],
    ['heif', 'HEIF'],
    ['tiff', 'TIFF'],
  ];

  function openAt(e: MouseEvent, items: MenuItem[]) {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    menu = { x: r.left, y: r.bottom + 6, items };
  }

  function sortMenu(e: MouseEvent) {
    openAt(e, [
      ...sorts.map(([k, label]) => ({ label, checked: store.sort === k, action: () => (store.sort = k) })),
      { separator: true },
      { label: 'Newest / largest first', checked: store.desc, action: () => (store.desc = true) },
      { label: 'Oldest / smallest first', checked: !store.desc, action: () => (store.desc = false) },
    ]);
  }

  function groupMenu(e: MouseEvent) {
    openAt(e, groups.map(([k, label]) => ({ label, checked: store.group === k, action: () => (store.group = k) })));
  }

  function filterMenu(e: MouseEvent) {
    openAt(e, [
      ...kinds.map(([k, label]) => ({
        label,
        checked: store.kinds.includes(k),
        action: () => {
          const next = store.kinds.includes(k) ? store.kinds.filter((x) => x !== k) : [...store.kinds, k];
          store.kinds = next.length ? next : [k];
        },
      })),
      { separator: true },
      ...[0, 1, 2, 3, 4, 5].map((r) => ({
        label: r ? `${'★'.repeat(r)} & up` : 'Any rating',
        checked: store.minRating === r,
        action: () => (store.minRating = r),
      })),
      { separator: true },
      { label: 'Stack RAW+JPEG pairs', checked: store.stack, action: () => (store.stack = !store.stack) },
    ]);
  }

  const filtered = $derived(store.minRating > 0 || store.kinds.length < 4 || !store.stack);
  const sortLabel = $derived(sorts.find(([k]) => k === store.sort)?.[1]);

  export function focusSearch() {
    search?.focus();
    search?.select();
  }
</script>

<header class="toolbar">
  <button class="icon-btn" class:on={!store.sidebar} title="Toggle sidebar" onclick={() => (store.sidebar = !store.sidebar)}>
    <Icon name="sidebar" size={17} />
  </button>

  <div class="title">
    <h1 class="truncate">{store.collectionTitle}</h1>
    {#if store.collection.type === 'similar' || store.collection.type === 'ids'}
      <button class="clear" title="Back to all photos" aria-label="Back to all photos" onclick={() => store.setCollection({ type: 'all' })}><Icon name="x" size={12} stroke={2.2} /></button>
    {/if}
    <span class="faint tabular">
      {store.selection.size ? `${formatCount(store.selection.size)} selected · ` : ''}{plural(store.visible.length, 'photo')}
    </span>
  </div>

  <div class="search" class:smart={store.smartReady}>
    <span class="lead" class:busy={store.smartBusy}><Icon name={store.smartReady ? 'sparkle' : 'search'} size={14} /></span>
    <input
      bind:this={search}
      class="field"
      type="search"
      placeholder={store.smartReady ? 'Describe a photo, e.g. “sunset over the sea”' : 'Search name, lens, film, place…'}
      spellcheck="false"
      bind:value={store.query}
      onkeydown={(e) => {
        if (e.key === 'Escape') {
          store.query = '';
          search.blur();
        }
      }}
    />
    {#if !store.smart?.enabled}
      <button
        class="enable"
        title="Turn on smart search: find photos by describing them (downloads a {store.smart?.downloadMb ?? 210} MB model once, runs on this PC)"
        aria-label="Turn on smart search"
        onclick={() => {
          store.setSmartSearch(true);
          store.toast('Smart search is setting up. Progress shows in the sidebar.');
        }}
      >
        <Icon name="sparkle" size={14} />
      </button>
    {/if}
  </div>

  {#if store.collection.type !== 'map'}
  <div class="seg" role="radiogroup" aria-label="Layout">
    <button role="radio" aria-checked={store.mode === 'justified'} class:on={store.mode === 'justified'} title="Photo rows" onclick={() => (store.mode = 'justified')}>
      <Icon name="rows" size={15} />
    </button>
    <button role="radio" aria-checked={store.mode === 'grid'} class:on={store.mode === 'grid'} title="Square grid" onclick={() => (store.mode = 'grid')}>
      <Icon name="grid" size={15} />
    </button>
  </div>

  <input class="size" type="range" min="90" max="480" step="1" bind:value={store.size} title="Thumbnail size (Ctrl + scroll)" aria-label="Thumbnail size" />

  {#if store.ranked}
    <span class="pill relevance" title="Results are ordered by how well they match">
      <Icon name="sparkle" size={14} /> Best match first
    </span>
  {:else}
    <button class="pill" onclick={sortMenu} title="Sort">
      <Icon name="sort" size={14} />
      <span>{sortLabel}</span>
      <Icon name="down" size={12} stroke={2} />
    </button>
  {/if}
  <button class="pill" onclick={groupMenu} title="Group">
    <span class="faint">Group</span>
    <span>{groups.find(([k]) => k === store.group)?.[1]}</span>
    <Icon name="down" size={12} stroke={2} />
  </button>
  <button class="icon-btn" class:on={filtered} title="Filter" onclick={filterMenu}>
    <Icon name="filter" size={17} />
  </button>

  {/if}

  <span class="divider"></span>
  <button class="icon-btn" class:on={store.showInfo} title="Info (I)" onclick={() => (store.showInfo = !store.showInfo)}>
    <Icon name="info" size={17} />
  </button>
  <button class="icon-btn" title="Settings (Ctrl+,)" onclick={() => (store.settingsOpen = true)}>
    <Icon name="gear" size={17} />
  </button>
</header>

{#if menu}
  <ContextMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .toolbar {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 52px;
    padding: 0 12px 0 10px;
    flex: none;
    border-bottom: 1px solid var(--sep);
  }
  .title {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
    flex: 1;
    padding-left: 4px;
  }
  h1 {
    margin: 0;
    font: 650 17px/1.2 var(--font-display);
    letter-spacing: -0.015em;
    min-width: 0;
  }
  .title span {
    font-size: 12px;
    white-space: nowrap;
  }
  .search {
    position: relative;
    display: flex;
    align-items: center;
    color: var(--text-3);
  }
  .lead {
    position: absolute;
    left: 10px;
    display: grid;
    pointer-events: none;
  }
  .search.smart .lead {
    color: #bf9bff;
  }
  .lead.busy {
    animation: pulse 0.9s ease-in-out infinite;
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .enable {
    position: absolute;
    right: 5px;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    color: var(--text-3);
  }
  .enable:hover {
    color: #bf9bff;
    background: rgba(191, 155, 255, 0.14);
  }
  .relevance {
    color: #d4bfff;
  }
  .relevance :global(svg) {
    color: #bf9bff !important;
  }
  .clear {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    background: var(--active);
    color: var(--text-2);
    align-self: center;
    flex: none;
  }
  .clear:hover {
    background: rgba(255, 255, 255, 0.18);
    color: var(--text);
  }
  .search input {
    width: 220px;
    height: 30px;
    padding-left: 30px;
    transition:
      width 200ms var(--ease-out),
      box-shadow 120ms,
      background 120ms;
  }
  .search input:focus,
  .search.smart input {
    width: 300px;
  }
  .search input::-webkit-search-cancel-button {
    filter: invert(0.7);
  }
  .seg {
    display: flex;
    padding: 2px;
    border-radius: 8px;
    background: var(--bg-input);
    box-shadow: inset 0 0 0 0.5px var(--border);
    margin-left: 6px;
  }
  .seg button {
    display: grid;
    place-items: center;
    width: 30px;
    height: 26px;
    border-radius: 6px;
    color: var(--text-2);
    transition:
      background 120ms,
      color 120ms;
  }
  .seg button.on {
    background: rgba(255, 255, 255, 0.14);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }
  .size {
    width: 92px;
    margin: 0 6px;
  }
  .pill {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 30px;
    padding: 0 10px;
    border-radius: 8px;
    color: var(--text);
    white-space: nowrap;
  }
  .pill:hover {
    background: var(--hover);
  }
  .pill :global(svg) {
    color: var(--text-2);
  }
  .divider {
    width: 1px;
    height: 20px;
    margin: 0 4px;
    background: var(--sep);
  }

  @media (max-width: 1180px) {
    .size,
    .pill .faint {
      display: none;
    }
    .search input {
      width: 160px;
    }
  }
</style>
