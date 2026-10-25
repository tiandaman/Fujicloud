<script lang="ts">
  import { plural } from '../format';
  import { store } from '../store.svelte';
  import Icon from './Icon.svelte';

  let { x, y, ids }: { x: number; y: number; ids: number[] } = $props();

  let el: HTMLDivElement;
  let name = $state('');
  let pos = $state({ left: 0, top: 0 });

  // Keep it on screen.
  $effect(() => {
    const r = el.getBoundingClientRect();
    pos = { left: Math.min(x, innerWidth - r.width - 10), top: Math.min(y, innerHeight - r.height - 10) };
  });

  /** Whether every photo being added is already in the album. */
  const contains = (albumIds: number[]) => {
    const set = new Set(albumIds);
    return ids.every((id) => set.has(id));
  };

  function close() {
    store.albumPicker = null;
  }

  async function toggle(id: number, all: boolean) {
    close();
    if (all) await store.removeFromAlbum(id, ids);
    else await store.addToAlbum(id, ids);
  }

  async function create() {
    if (!name.trim()) return;
    close();
    await store.createAlbum(name, ids);
  }
</script>

<svelte:window
  onkeydown={(e) => e.key === 'Escape' && close()}
  onpointerdown={(e) => {
    if (!el.contains(e.target as Node)) close();
  }}
/>

<div class="picker" bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px" role="dialog" aria-label="Add to album">
  <header>Add {plural(ids.length, 'photo')} to</header>
  <div class="list">
    {#each store.albums as a (a.id)}
      {@const all = contains(a.ids)}
      <button class="row" class:on={all} onclick={() => toggle(a.id, all)} title={all ? 'Remove from this album' : 'Add to this album'}>
        <Icon name="album" size={15} />
        <span class="truncate">{a.name}</span>
        {#if all}<Icon name="check" size={14} stroke={2.4} />{/if}
      </button>
    {/each}
  </div>
  <form
    class="new"
    onsubmit={(e) => {
      e.preventDefault();
      create();
    }}
  >
    <Icon name="plus" size={15} />
    <!-- svelte-ignore a11y_autofocus -->
    <input placeholder="New album…" bind:value={name} autofocus onkeydown={(e) => e.stopPropagation()} />
  </form>
</div>

<style>
  .picker {
    position: fixed;
    z-index: 80;
    width: 250px;
    padding: 6px;
    border-radius: 12px;
    background: rgba(36, 38, 44, 0.96);
    backdrop-filter: blur(20px) saturate(1.4);
    box-shadow: var(--shadow-lg), inset 0 0 0 0.5px var(--border);
    animation: pop 140ms var(--ease-out);
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
  }
  header {
    padding: 6px 8px 8px;
    color: var(--text-3);
    font-size: 11.5px;
    font-weight: 600;
  }
  .list {
    max-height: 260px;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 30px;
    padding: 0 8px;
    border-radius: 7px;
    color: var(--text);
    text-align: left;
  }
  .row span {
    flex: 1;
  }
  .row:hover {
    background: var(--accent);
    color: white;
  }
  .row.on :global(svg:last-child) {
    color: var(--accent);
  }
  .row.on:hover :global(svg:last-child) {
    color: white;
  }
  .new {
    display: flex;
    align-items: center;
    gap: 9px;
    margin-top: 4px;
    padding: 6px 8px 2px;
    border-top: 1px solid var(--sep);
    color: var(--text-2);
  }
  .new input {
    flex: 1;
    min-width: 0;
    height: 28px;
    padding: 0 8px;
    border-radius: 6px;
    background: var(--bg-input);
    color: var(--text);
    font: inherit;
    outline: none;
  }
  .new input:focus {
    box-shadow: inset 0 0 0 1px var(--accent);
  }
</style>
