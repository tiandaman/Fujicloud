<script lang="ts">
  import { api } from '../api';
  import { plural } from '../format';
  import { store } from '../store.svelte';
  import Icon from './Icon.svelte';

  let { x, y, id }: { x: number; y: number; id: number } = $props();

  let el: HTMLDivElement;
  let query = $state('');
  let pos = $state({ left: 0, top: 0 });
  const me = $derived(store.personById.get(id));
  const others = $derived(
    store.people.filter(
      (p) => p.id !== id && p.photoIds.length > 0 && (!query.trim() || (p.name ?? '').toLowerCase().includes(query.trim().toLowerCase())),
    ),
  );

  // Keep it on screen.
  $effect(() => {
    const r = el.getBoundingClientRect();
    pos = { left: Math.max(10, Math.min(x, innerWidth - r.width - 10)), top: Math.max(10, Math.min(y, innerHeight - r.height - 10)) };
  });

  function close() {
    store.mergePicker = null;
  }

  function pick(into: number) {
    // Closing unmounts this picker and its props with it: read them first.
    const from = id;
    close();
    store.mergePeople([from, into], into);
  }

  function focus(el: HTMLInputElement) {
    el.focus();
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      close();
      e.stopImmediatePropagation();
    }
  }}
/>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="picker" bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px" role="dialog" aria-label="Merge with">
  <header>
    <strong>Merge {me?.name ?? 'this person'} with…</strong>
    <span class="faint">Same person found twice? Pick the other one.</span>
  </header>
  <input
    placeholder="Search names"
    bind:value={query}
    use:focus
    onkeydown={(e) => {
      if (e.key === 'Enter' && others.length === 1) pick(others[0].id);
      e.stopPropagation();
    }}
  />
  <div class="list">
    {#each others as p (p.id)}
      <button class="row" onclick={() => pick(p.id)}>
        <img src={api.faceUrl(p.cover)} alt="" draggable="false" />
        <span class="grow truncate">{p.name ?? 'Unnamed'}</span>
        <span class="faint tabular">{plural(p.photoIds.length, 'photo')}</span>
      </button>
    {:else}
      <p class="faint empty">No one {query.trim() ? `called “${query.trim()}”` : 'else yet'}.</p>
    {/each}
  </div>
  <footer class="faint"><Icon name="people" size={12} /> Their photos join up; a name is kept.</footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 74;
  }
  .picker {
    position: fixed;
    z-index: 75;
    width: 290px;
    max-height: 420px;
    display: flex;
    flex-direction: column;
    border-radius: 12px;
    background: rgba(40, 40, 44, 0.96);
    backdrop-filter: blur(24px) saturate(1.6);
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
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: 12px 12px 8px;
  }
  header strong {
    font-size: 13px;
  }
  header span {
    font-size: 11.5px;
  }
  input {
    margin: 0 10px 6px;
    padding: 6px 9px;
    border: 0;
    border-radius: 7px;
    background: rgba(255, 255, 255, 0.07);
    box-shadow: inset 0 0 0 1px var(--border);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
  }
  input:focus {
    outline: none;
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .list {
    overflow-y: auto;
    padding: 0 6px 6px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    padding: 5px 7px;
    border: 0;
    border-radius: 7px;
    background: none;
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    text-align: left;
    cursor: default;
  }
  .row:hover {
    background: var(--accent);
    color: #fff;
  }
  .row:hover .faint {
    color: rgba(255, 255, 255, 0.8);
  }
  .row img {
    width: 26px;
    height: 26px;
    flex: none;
    border-radius: 50%;
    object-fit: cover;
  }
  .grow {
    flex: 1;
    min-width: 0;
  }
  .row .faint {
    font-size: 11px;
  }
  .empty {
    margin: 8px;
    font-size: 12px;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 5px;
    padding: 7px 12px 10px;
    border-top: 1px solid var(--sep);
    font-size: 11px;
  }
</style>
