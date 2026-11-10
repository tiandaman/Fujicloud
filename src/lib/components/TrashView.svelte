<script lang="ts">
  import { api } from '../api';
  import { formatAgo, formatBytes, plural } from '../format';
  import { store } from '../store.svelte';
  import Icon from './Icon.svelte';

  // Listing reads the library drive, so it's done on opening.
  $effect(() => {
    store.loadTrash();
  });

  const trash = $derived(store.trash);
  let chosen = $state<Set<number>>(new Set());
  // A listing renumbers what's in it.
  $effect(() => {
    void trash;
    chosen = new Set();
  });

  function toggle(n: number) {
    const next = new Set(chosen);
    if (!next.delete(n)) next.add(n);
    chosen = next;
  }

  const all = $derived(!!trash?.items.length && chosen.size === trash.items.length);
  function toggleAll() {
    chosen = all ? new Set() : new Set(trash?.items.map((i) => i.n));
  }
</script>

<section class="trash">
  {#if !trash}
    <p class="empty faint">Looking in the trash…</p>
  {:else}
    <header>
      <p class="hint">
        Photos deleted from the library's drive wait here, in <code>.fujicloud-trash</code> on that drive, until the trash is emptied. Putting
        one back returns it to its folder with its stars, albums and names.
        {#if trash.recycleBin}Photos deleted from this PC's own drives are in the Windows Recycle Bin instead.{/if}
        {#if trash.offline}{plural(trash.offline, 'library folder')} can't be reached right now, so {trash.offline === 1 ? 'its' : 'their'} trash isn't shown.{/if}
      </p>
      {#if trash.items.length}
        <div class="actions">
          <label class="pick-all"><input type="checkbox" checked={all} onchange={toggleAll} /> All</label>
          <span class="faint">{chosen.size ? `${chosen.size} selected` : `${plural(trash.items.length, 'photo')} · ${formatBytes(trash.bytes)}`}</span>
          <span class="grow"></span>
          <button class="btn" disabled={!chosen.size} onclick={() => store.restoreTrash([...chosen])}><Icon name="back" size={13} /> Put back</button>
          <button class="btn danger" disabled={!chosen.size} onclick={() => store.emptyTrash([...chosen])}>Delete for good</button>
          <button class="btn danger" onclick={() => store.emptyTrash()}><Icon name="trash" size={13} /> Empty trash</button>
        </div>
      {/if}
    </header>
    {#if !trash.items.length}
      <div class="empty">
        <Icon name="trash" size={40} stroke={1.2} />
        <h3>The trash is empty</h3>
      </div>
    {:else}
      <div class="tiles">
        {#each trash.items as item (item.n)}
          <button class="tile" class:on={chosen.has(item.n)} aria-pressed={chosen.has(item.n)} onclick={() => toggle(item.n)} ondblclick={() => store.restoreTrash([item.n])} title="Double-click to put back">
            <img src={api.trashThumbUrl(item)} alt="" loading="lazy" draggable="false" />
            <span class="check"><Icon name="check" size={12} stroke={2.6} /></span>
            <strong class="truncate">{item.name}</strong>
            <span class="faint truncate">{item.folder || 'Library folder'}</span>
            <span class="faint truncate">{item.at ? `Deleted ${formatAgo(item.at)}${item.by ? ` by ${item.by}` : ''}` : formatBytes(item.size)}</span>
          </button>
        {/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .trash {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 14px 20px 90px;
  }
  header {
    position: sticky;
    top: -14px;
    z-index: 2;
    margin: -14px -20px 12px;
    padding: 14px 20px 10px;
    background: var(--bg, #161618);
    border-bottom: 1px solid var(--sep);
  }
  .hint {
    margin: 0 0 10px;
    max-width: 78ch;
    color: var(--text-2);
    font-size: 12.5px;
    line-height: 1.5;
  }
  code {
    font-size: 11.5px;
    color: var(--text);
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12.5px;
  }
  .pick-all {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .grow {
    flex: 1;
  }
  .btn.danger {
    color: #ff8a82;
  }
  .btn:disabled {
    opacity: 0.45;
  }
  .tiles {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 14px;
  }
  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 6px 6px 8px;
    border: 0;
    border-radius: 10px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 12px;
    text-align: left;
    cursor: default;
  }
  .tile:hover {
    background: rgba(255, 255, 255, 0.04);
  }
  .tile.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .tile img {
    width: 100%;
    aspect-ratio: 3 / 2;
    margin-bottom: 5px;
    border-radius: 7px;
    object-fit: cover;
    background: rgba(255, 255, 255, 0.06);
  }
  .tile strong {
    font-weight: 600;
  }
  .tile .faint {
    font-size: 11px;
  }
  .check {
    position: absolute;
    top: 12px;
    left: 12px;
    display: none;
    width: 20px;
    height: 20px;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
  }
  .tile.on .check {
    display: flex;
  }
  .truncate {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 10px;
    margin-top: 18vh;
    color: var(--text-3);
  }
  .empty h3 {
    margin: 0;
    font-weight: 600;
    color: var(--text-2);
  }
</style>
