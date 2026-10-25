<script lang="ts">
  import { untrack } from 'svelte';
  import { api } from '../api';
  import { labelColors } from '../film';
  import { formatDuration } from '../format';
  import { computeLayout, firstRowAfter, headerAt, verticalNeighbor, type Row } from '../layout';
  import { store } from '../store.svelte';
  import type { Photo } from '../types';
  import ContextMenu, { type MenuItem } from './ContextMenu.svelte';
  import Icon from './Icon.svelte';

  // Render this far beyond the viewport so fast flicks rarely show gaps.
  const OVERSCAN = 900;

  let scroller: HTMLDivElement;
  let width = $state(0);
  let height = $state(0);
  let scrollTop = $state(0);
  let scrolling = $state(false);
  let scrollIdle: ReturnType<typeof setTimeout>;
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  const layout = $derived(computeLayout(store.sections, width, store.mode, store.size, store.group !== 'none'));

  const visible = $derived.by(() => {
    const rows = layout.rows;
    const top = scrollTop - OVERSCAN;
    const bottom = scrollTop + height + OVERSCAN;
    const headers: Row[] = [];
    const tiles: number[] = [];
    for (let r = firstRowAfter(rows, top); r < rows.length && rows[r].y < bottom; r++) {
      const row = rows[r];
      if (row.section) headers.push(row);
      for (let i = row.start; i < row.end; i++) tiles.push(i);
    }
    return { headers, tiles };
  });

  // The section title pinned to the top once its header scrolls away.
  const sticky = $derived.by(() => {
    const h = headerAt(layout.headers, scrollTop + 1);
    return h && scrollTop > h.y + 8 ? h : null;
  });

  // ---- keep your place across relayouts --------------------------------------

  let anchor: { id: number; offset: number } | null = null;

  function captureAnchor() {
    const rows = layout.rows;
    for (let r = firstRowAfter(rows, scrollTop); r < rows.length; r++) {
      if (rows[r].end > rows[r].start) {
        const i = rows[r].start;
        anchor = { id: layout.flat[i].id, offset: layout.boxes[i * 4 + 1] - scrollTop };
        return;
      }
    }
    anchor = null;
  }

  // A new view starts at the top...
  $effect(() => {
    void [store.collection, store.sort, store.desc, store.group, store.query, store.kinds, store.minRating];
    untrack(() => {
      anchor = null;
      if (scroller) scroller.scrollTop = 0;
    });
  });

  // ...while resizing, zooming the grid or a library refresh keeps the photo
  // at the top of the window where it was.
  $effect(() => {
    void layout;
    untrack(() => {
      if (!anchor || !scroller) return;
      const i = layout.index.get(anchor.id);
      if (i === undefined) return;
      const target = layout.boxes[i * 4 + 1] - anchor.offset;
      if (Math.abs(scroller.scrollTop - target) > 1) scroller.scrollTop = target;
    });
  });

  function onScroll() {
    scrollTop = scroller.scrollTop;
    captureAnchor();
    scrolling = true;
    clearTimeout(scrollIdle);
    scrollIdle = setTimeout(() => (scrolling = false), 140);
  }

  // Ctrl + wheel (or trackpad pinch) zooms the grid, like Photos.
  function onWheel(e: WheelEvent) {
    if (!e.ctrlKey) return;
    e.preventDefault();
    store.size = Math.round(Math.min(480, Math.max(90, store.size * Math.exp(-e.deltaY * 0.006))));
  }

  // ---- handle for the viewer ----------------------------------------------------

  function reveal(id: number) {
    const i = layout.index.get(id);
    if (i === undefined || !scroller) return;
    const y = layout.boxes[i * 4 + 1];
    const h = layout.boxes[i * 4 + 3];
    const pad = sticky ? 64 : 12;
    if (y < scroller.scrollTop + pad) scroller.scrollTop = y - pad;
    else if (y + h > scroller.scrollTop + height - 12) scroller.scrollTop = y + h - height + 12;
    scrollTop = scroller.scrollTop;
  }

  $effect(() => {
    store.grid = {
      rectOf(id) {
        const i = layout.index.get(id);
        if (i === undefined || !scroller) return null;
        const r = scroller.getBoundingClientRect();
        const y = r.top + layout.boxes[i * 4 + 1] - scroller.scrollTop;
        const h = layout.boxes[i * 4 + 3];
        if (y + h < r.top || y > r.bottom) return null;
        return new DOMRect(r.left + layout.boxes[i * 4], y, layout.boxes[i * 4 + 2], h);
      },
      reveal,
    };
    return () => (store.grid = null);
  });

  // ---- selection ------------------------------------------------------------------

  function toggle(id: number) {
    const next = new Set(store.selection);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    store.selection = next;
    store.focusId = id;
    store.anchorId = id;
  }

  function selectRange(id: number) {
    const a = layout.index.get(store.anchorId ?? id) ?? 0;
    const b = layout.index.get(id) ?? 0;
    const next = new Set(store.selection);
    for (let i = Math.min(a, b); i <= Math.max(a, b); i++) next.add(layout.flat[i].id);
    store.selection = next;
    store.focusId = id;
  }

  // ---- drag out: photos dropped on Explorer, Photoshop or a browser ----------------

  let press: { x: number; y: number; id: number } | null = null;
  let dragged = false;

  function onTilePointerDown(e: PointerEvent, p: Photo) {
    dragged = false;
    // Touch drags scroll the grid; only a mouse or pen drags photos out.
    if (e.button === 0 && e.pointerType !== 'touch' && api.native) press = { x: e.clientX, y: e.clientY, id: p.id };
  }

  function onWindowPointerMove(e: PointerEvent) {
    if (!press) return;
    if (!(e.buttons & 1)) {
      press = null;
      return;
    }
    if (Math.hypot(e.clientX - press.x, e.clientY - press.y) < 8) return;
    const ids = store.selection.has(press.id) ? [...store.selection] : [press.id];
    press = null;
    dragged = true;
    api.dragOut(ids).catch((err) => store.toast(String(err), 'error'));
  }

  function onTileClick(e: MouseEvent, p: Photo) {
    // The click that ends a drag out isn't a click on the photo.
    if (dragged) {
      dragged = false;
      return;
    }
    if (e.shiftKey) selectRange(p.id);
    else if (e.ctrlKey || e.metaKey || store.selection.size) toggle(p.id);
    else store.open(p.id);
  }

  function onContextMenu(e: MouseEvent, p: Photo) {
    e.preventDefault();
    if (!store.selection.has(p.id)) {
      store.selection = new Set();
      store.focusId = p.id;
    }
    const ids = store.selection.size ? [...store.selection] : [p.id];
    const many = ids.length > 1;
    const pair = store.comparisonOf(ids);
    const compareLabel = many ? 'Compare side by side' : p.editOf != null ? 'Compare with original' : 'Compare with edit';
    menu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        { label: 'Open', icon: 'expand', hint: 'Space', action: () => store.open(p.id), hidden: many },
        { label: many ? `Edit ${ids.length} in Photoshop` : 'Edit in Photoshop', icon: 'edit', hint: 'Ctrl+E', action: () => store.edit(ids) },
        { label: compareLabel, icon: 'compare', hint: 'C', action: () => store.openCompare(pair), hidden: !pair },
        { label: 'Show in Explorer', icon: 'reveal', action: () => api.reveal(p.id), hidden: many },
        { label: 'Open with default app', icon: 'open', action: () => api.openDefault(p.id), hidden: many },
        { label: 'Find similar photos', icon: 'similar', action: () => store.findSimilar(p.id), hidden: many || !store.smartReady },
        { label: 'Show on map', icon: 'globe', action: () => store.showOnMap(p), hidden: many || p.lat == null },
        { separator: true },
        { stars: true, value: many ? 0 : p.rating, onrate: (r: number) => store.rate(r, ids) },
        {
          label: ids.every((id) => store.byId.get(id)?.favorite) ? 'Remove from Favourites' : 'Favourite',
          icon: 'heart',
          hint: '.',
          action: () => store.favorite(ids),
        },
        { label: 'Add to album…', icon: 'album', action: () => (store.albumPicker = { x: e.clientX, y: e.clientY, ids }) },
        {
          label: `Not ${store.collection.type === 'person' ? (store.personById.get(store.collection.id)?.name ?? 'this person') : ''}`,
          icon: 'people',
          action: () => store.collection.type === 'person' && store.removeFromPerson(store.collection.id, ids),
          hidden: store.collection.type !== 'person',
        },
        {
          label: `Remove from “${store.collection.type === 'album' ? store.albumById.get(store.collection.id)?.name : ''}”`,
          icon: 'x',
          action: () => store.collection.type === 'album' && store.removeFromAlbum(store.collection.id, ids),
          hidden: store.collection.type !== 'album',
        },
        { label: p.flag === 1 && !many ? 'Unpick' : 'Pick', icon: 'flag', hint: 'P', action: () => store.flag(p.flag === 1 && !many ? 0 : 1, ids) },
        { label: p.flag === -1 && !many ? 'Unreject' : 'Reject', icon: 'x', hint: 'X', action: () => store.flag(p.flag === -1 && !many ? 0 : -1, ids) },
        { labels: true, value: many ? null : p.label, onlabel: (l) => store.label(l, ids) },
        { separator: true },
        { label: many ? `Export ${ids.length} photos…` : 'Export…', icon: 'export', hint: 'Ctrl+Shift+E', action: () => (store.exportDialog = { ids }) },
        { label: many ? `Delete ${ids.length} photos…` : 'Delete…', icon: 'trash', hint: 'Del', action: () => store.deletePhotos(ids) },
      ],
    };
  }

  // ---- keyboard ------------------------------------------------------------------

  function onKey(e: KeyboardEvent) {
    if (store.viewerId != null || store.compare || store.settingsOpen || store.confirmation || store.exportDialog || menu) return;
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, select, [contenteditable]')) return;
    const flat = layout.flat;
    if (!flat.length) return;
    const cur = store.focusId != null ? (layout.index.get(store.focusId) ?? -1) : -1;
    let next = -1;
    switch (e.key) {
      case 'ArrowRight': next = Math.min(flat.length - 1, cur + 1); break;
      case 'ArrowLeft': next = Math.max(0, cur - 1); break;
      case 'ArrowDown': next = cur < 0 ? 0 : verticalNeighbor(layout, cur, 1); break;
      case 'ArrowUp': next = cur < 0 ? 0 : verticalNeighbor(layout, cur, -1); break;
      case 'Home': next = 0; break;
      case 'End': next = flat.length - 1; break;
      case ' ':
      case 'Enter':
        if (cur >= 0) {
          e.preventDefault();
          store.open(flat[cur].id);
        }
        return;
      case 'a':
        if (e.ctrlKey || e.metaKey) {
          e.preventDefault();
          store.selection = new Set(flat.map((p) => p.id));
        }
        return;
      case 'Escape':
        store.selection = new Set();
        return;
      default:
        return;
    }
    e.preventDefault();
    const id = flat[next].id;
    if (e.shiftKey) selectRange(id);
    else store.anchorId = id;
    store.focusId = id;
    reveal(id);
  }

  // Thumbnails that failed while the Shield was off load once it's back.
  $effect(() => {
    if (!store.reachEpoch || !scroller) return;
    for (const tile of scroller.querySelectorAll<HTMLElement>('.failed')) {
      tile.classList.remove('failed');
      const img = tile.querySelector('img');
      if (img) img.src = img.src;
    }
  });

  function fadeIn(img: HTMLImageElement) {
    if (img.complete && img.naturalWidth) img.classList.add('loaded');
    else img.addEventListener('load', () => img.classList.add('loaded'), { once: true });
  }
</script>

<svelte:window onkeydown={onKey} onpointermove={onWindowPointerMove} onpointerup={() => (press = null)} />

<div class="wrap">
  <div
    class="scroller"
    class:scrolling
    bind:this={scroller}
    bind:clientWidth={width}
    bind:clientHeight={height}
    onscroll={onScroll}
    onwheel={onWheel}
  >
    <div class="canvas" style:height="{layout.height}px">
      {#each visible.headers as row (row.section!.key)}
        <header class="section" style:transform="translateY({row.y}px)">
          <h2>{row.section!.title}</h2>
          <span>{row.section!.subtitle}</span>
          {#if row.section!.action}
            <button class="section-action" onclick={row.section!.action.run}>{row.section!.action.label}</button>
          {/if}
        </header>
      {/each}

      {#each visible.tiles as i (layout.flat[i].id)}
        {@const p = layout.flat[i]}
        {@const selected = store.selection.has(p.id)}
        <div
          class="tile"
          class:grid={store.mode === 'grid'}
          class:selected
          class:selecting={store.selection.size > 0}
          class:focused={store.focusId === p.id}
          class:rejected={p.flag === -1}
          class:hidden={store.hiddenId === p.id}
          style:transform="translate({layout.boxes[i * 4]}px, {layout.boxes[i * 4 + 1]}px)"
          style:width="{layout.boxes[i * 4 + 2]}px"
          style:height="{layout.boxes[i * 4 + 3]}px"
          role="button"
          tabindex="-1"
          aria-label={p.name}
          onpointerdown={(e) => onTilePointerDown(e, p)}
          onclick={(e) => onTileClick(e, p)}
          oncontextmenu={(e) => onContextMenu(e, p)}
          onkeydown={() => {}}
        >
          <img
            src={api.thumbUrl(p)}
            alt=""
            decoding="async"
            draggable="false"
            use:fadeIn
            onerror={(e) => (e.currentTarget as HTMLElement).parentElement?.classList.add('failed')}
          />
          <div class="placeholder">
            <span class="badge">{p.kind === 'heif' ? 'HEIF' : p.kind.toUpperCase()}</span>
            <span class="name">{p.name}</span>
          </div>

          <div class="overlay">
            <div class="top">
              <button
                class="check"
                aria-label={selected ? 'Deselect' : 'Select'}
                onclick={(e) => {
                  e.stopPropagation();
                  if (e.shiftKey) selectRange(p.id);
                  else toggle(p.id);
                }}
              >
                <Icon name="check" size={12} stroke={3} />
              </button>
              <span class="marks">
                {#if p.label}<i class="label" style:background={labelColors[p.label]}></i>{/if}
                {#if p.flag === 1}<span class="pick"><Icon name="flag" size={12} fill stroke={2} /></span>{/if}
              </span>
            </div>
            <div class="bottom">
              {#if p.favorite}
                <span class="fav"><Icon name="heart" size={13} fill stroke={1.6} /></span>
              {/if}
              {#if store.collection.type === 'duplicates' && store.keepers.has(p.id)}
                <span class="badge keep" title="The one to keep: best rated, then most pixels">KEEP</span>
              {/if}
              {#if p.rating}
                <span class="stars">{'★'.repeat(p.rating)}</span>
              {/if}
              <span class="kinds">
                {#if p.editOf == null && store.editsOf.has(p.id)}
                  <span class="badge" title="Edited · press C to compare"><Icon name="compare" size={11} stroke={2} /></span>
                {/if}
                {#if p.kind === 'video'}
                  <span class="badge movie"><Icon name="play" size={9} fill stroke={0} />{p.duration ? formatDuration(p.duration) : ''}</span>
                {:else if store.stackBadge(p)}<span class="badge">{store.stackBadge(p)}</span>{/if}
              </span>
              {#if p.editOf != null}
                <span class="badge edit" title="Edit · press C to compare with the original">{p.kind === 'psd' ? 'PSD' : p.kind === 'tiff' ? 'TIFF' : 'EDIT'}</span>
              {/if}
            </div>
          </div>
        </div>
      {/each}
    </div>
  </div>

  {#if sticky}
    <div class="sticky">
      <h2>{sticky.section!.title}</h2>
      <span>{sticky.section!.subtitle}</span>
    </div>
  {/if}

  {#if layout.flat.length === 0 && store.ready}
    <div class="empty">
      <Icon name={store.collection.type === 'duplicates' ? 'similar' : 'photos'} size={40} stroke={1.2} />
      <h3>
        {store.collection.type === 'duplicates' ? (store.findingDuplicates ? 'Looking for duplicates…' : 'No duplicates') : 'No photos here'}
      </h3>
      <p>
        {#if store.collection.type === 'duplicates'}
          {store.findingDuplicates
            ? 'Comparing files of the same size, and shots taken moments apart.'
            : store.smartReady
              ? 'No copies of the same file, and no near-identical shots.'
              : 'No copies of the same file. Turn on smart search to find near-identical shots too.'}
        {:else if store.query}Nothing matches “{store.query}”.
        {:else if store.minRating || store.kindsFiltered}Try loosening the filters in the toolbar.
        {:else}Photos you add will show up here.{/if}
      </p>
    </div>
  {/if}
</div>

{#if menu}
  <ContextMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .wrap {
    position: relative;
    flex: 1;
    min-width: 0;
    height: 100%;
  }
  .scroller {
    position: absolute;
    inset: 0;
    overflow-y: auto;
    overflow-x: hidden;
    overscroll-behavior: contain;
  }
  .canvas {
    position: relative;
    contain: strict;
    width: 100%;
  }
  /* Hover effects are wasted work mid-scroll. */
  .scrolling .tile {
    pointer-events: none;
  }

  .section,
  .sticky {
    position: absolute;
    left: 0;
    right: 0;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 22px 22px 0;
    height: 58px;
  }
  .section h2,
  .sticky h2 {
    margin: 0;
    font: 600 17px/1.2 var(--font-display);
    letter-spacing: -0.01em;
  }
  .section span,
  .sticky span {
    color: var(--text-3);
    font-size: 12px;
  }
  .sticky {
    top: 0;
    height: 48px;
    padding-top: 14px;
    background: rgba(20, 20, 22, 0.86);
    backdrop-filter: blur(20px) saturate(1.5);
    box-shadow: 0 1px 0 var(--sep);
    pointer-events: none;
    z-index: 2;
    animation: drop 160ms var(--ease-out);
  }
  @keyframes drop {
    from {
      opacity: 0;
      transform: translateY(-6px);
    }
  }

  .tile {
    position: absolute;
    top: 0;
    left: 0;
    overflow: hidden;
    background: var(--tile);
    border-radius: 3px;
    contain: strict;
  }
  .tile.grid {
    border-radius: 2px;
  }
  .tile.hidden {
    visibility: hidden;
  }
  .tile img {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: cover;
    opacity: 0;
    transition:
      opacity 180ms ease,
      transform 220ms var(--ease-out),
      border-radius 220ms var(--ease-out);
  }
  .tile img:global(.loaded) {
    opacity: 1;
  }
  .tile.rejected img {
    opacity: 0.35;
    filter: grayscale(0.6);
  }
  .tile.selected {
    background: var(--accent-soft);
  }
  .tile.selected img {
    transform: scale(0.88);
    border-radius: 6px;
  }
  .tile.focused::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: inherit;
    box-shadow: inset 0 0 0 2px var(--accent);
    pointer-events: none;
  }

  .placeholder {
    position: absolute;
    inset: 0;
    display: none;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-3);
    font-size: 11px;
    padding: 8px;
    text-align: center;
  }
  .tile:global(.failed) img {
    display: none;
  }
  .tile:global(.failed) .placeholder {
    display: flex;
  }
  .section-action {
    margin-left: auto;
    align-self: center;
    padding: 2px 10px;
    border: 0;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-2);
    font: inherit;
    font-size: 11.5px;
    cursor: default;
    pointer-events: auto;
  }
  .section-action:hover {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text);
  }
  .badge.keep {
    background: var(--pick);
    color: #04210d;
    font-weight: 700;
  }
  .placeholder .name {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    padding: 7px;
    pointer-events: none;
  }
  .overlay::before {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(to bottom, rgba(0, 0, 0, 0.35), transparent 30%, transparent 70%, rgba(0, 0, 0, 0.45));
    opacity: 0;
    transition: opacity 140ms;
  }
  .tile:hover .overlay::before {
    opacity: 1;
  }
  .top,
  .bottom {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 6px;
    min-height: 20px;
  }
  .check {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    border-radius: 50%;
    color: transparent;
    box-shadow:
      inset 0 0 0 1.5px rgba(255, 255, 255, 0.85),
      0 0 4px rgba(0, 0, 0, 0.4);
    opacity: 0;
    transform: scale(0.85);
    transition:
      opacity 120ms,
      transform 160ms var(--ease-out),
      background 120ms;
    pointer-events: auto;
  }
  .tile:hover .check,
  .tile.selecting .check {
    opacity: 1;
    transform: none;
  }
  .check:hover {
    background: rgba(255, 255, 255, 0.25);
  }
  .tile.selected .check {
    background: var(--accent);
    color: white;
    box-shadow:
      inset 0 0 0 1.5px white,
      0 1px 4px rgba(0, 0, 0, 0.35);
  }
  .marks {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .label {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    box-shadow:
      0 0 0 1.5px rgba(0, 0, 0, 0.35),
      0 1px 3px rgba(0, 0, 0, 0.4);
  }
  .pick {
    color: var(--pick);
    filter: drop-shadow(0 1px 2px rgba(0, 0, 0, 0.6));
  }
  .fav {
    color: #fff;
    filter: drop-shadow(0 1px 2px rgba(0, 0, 0, 0.6));
  }
  .stars {
    margin-right: auto;
    color: var(--star);
    font-size: 11px;
    letter-spacing: 1px;
    text-shadow: 0 1px 3px rgba(0, 0, 0, 0.7);
  }
  .kinds {
    margin-left: auto;
    opacity: 0;
    transition: opacity 140ms;
  }
  .tile:hover .kinds {
    opacity: 1;
  }
  .kinds {
    display: flex;
    gap: 3px;
  }
  /* A movie's length is always shown, like in Photos. */
  .kinds:has(.movie) {
    opacity: 1;
  }
  .badge.movie {
    display: inline-flex;
    align-items: center;
    gap: 3px;
  }
  .kinds .badge,
  .badge.edit {
    background: rgba(0, 0, 0, 0.55);
    color: rgba(255, 255, 255, 0.9);
    backdrop-filter: blur(6px);
  }
  .badge.edit {
    color: #fff;
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.35);
  }

  .empty {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-3);
    pointer-events: none;
  }
  .empty h3 {
    margin: 10px 0 0;
    color: var(--text-2);
    font: 600 16px var(--font-display);
  }
  .empty p {
    margin: 0;
  }
</style>
