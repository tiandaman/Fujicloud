<script lang="ts">
  import { onMount } from 'svelte';
  import AlbumPicker from './lib/components/AlbumPicker.svelte';
  import Compare from './lib/components/Compare.svelte';
  import Confirm from './lib/components/Confirm.svelte';
  import ExportDialog from './lib/components/ExportDialog.svelte';
  import Grid from './lib/components/Grid.svelte';
  import InfoPanel from './lib/components/InfoPanel.svelte';
  import MapView from './lib/components/MapView.svelte';
  import PeopleView from './lib/components/PeopleView.svelte';
  import Settings from './lib/components/Settings.svelte';
  import Sidebar from './lib/components/Sidebar.svelte';
  import Toasts from './lib/components/Toasts.svelte';
  import Toolbar from './lib/components/Toolbar.svelte';
  import Viewer from './lib/components/Viewer.svelte';
  import Welcome from './lib/components/Welcome.svelte';
  import { store } from './lib/store.svelte';
  import type { Label } from './lib/types';

  let toolbar: Toolbar;
  let error = $state<string | null>(null);

  onMount(() => {
    store.init().catch((e) => (error = String(e)));
  });

  $effect(() => {
    document.documentElement.classList.toggle('mica', store.effects);
  });

  const LABEL_KEYS: Record<string, Label> = { '6': 'Red', '7': 'Yellow', '8': 'Green', '9': 'Blue' };

  // Shortcuts that work in both the grid and the viewer.
  function onKey(e: KeyboardEvent) {
    if (store.settingsOpen || store.compare || store.confirmation || store.exportDialog || (e.target as HTMLElement).closest('input, textarea, select'))
      return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl) {
      if (e.key === 'z' && !e.shiftKey) store.undo();
      else if ((e.key === 'z' || e.key === 'Z') && e.shiftKey) store.redo();
      else if (e.key === 'y') store.redo();
      else if (e.key === 'f') toolbar.focusSearch();
      else if (e.key.toLowerCase() === 'e' && e.shiftKey) store.targets.length && (store.exportDialog = { ids: store.targets });
      else if (e.key === 'e') store.edit();
      else if (e.key === ',') store.settingsOpen = true;
      else if (e.key === '=' || e.key === '+') store.size = Math.min(480, Math.round(store.size * 1.15));
      else if (e.key === '-') store.size = Math.max(90, Math.round(store.size / 1.15));
      else return;
      e.preventDefault();
      return;
    }
    if (e.altKey) return;
    const key = e.key.toLowerCase();
    if (/^[0-5]$/.test(e.key)) store.rate(Number(e.key));
    else if (LABEL_KEYS[e.key]) store.label(LABEL_KEYS[e.key]);
    else if (key === 'p') store.flag(1);
    else if (key === 'x') store.flag(-1);
    else if (key === 'u') store.flag(0);
    else if (key === 'i') store.showInfo = !store.showInfo;
    else if (e.key === '.') store.favorite();
    else if (key === 'c') store.openCompare(store.comparisonOf(store.viewerId != null ? [store.viewerId] : store.targets));
    else if (key === '/') toolbar.focusSearch();
    else if (e.key === 'Delete') store.deletePhotos();
    else return;
    e.preventDefault();
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="app">
  {#if store.sidebar && store.ready}
    <Sidebar />
  {/if}
  <main>
    <Toolbar bind:this={toolbar} />
    <div class="content">
      {#if error}
        <div class="fatal">
          <h2>Fujicloud couldn't start</h2>
          <p class="muted">{error}</p>
        </div>
      {:else if store.ready && !store.sources.length && !store.photos.length}
        <Welcome />
      {:else if store.collection.type === 'map'}
        <MapView />
      {:else if store.collection.type === 'people'}
        <PeopleView />
      {:else}
        <Grid />
        {#if store.showInfo}
          <aside class="info"><InfoPanel ids={store.targets} /></aside>
        {/if}
      {/if}
    </div>
  </main>
</div>

{#if store.viewerId != null}
  <Viewer />
{/if}
{#if store.compare}
  <Compare />
{/if}
{#if store.albumPicker}
  <AlbumPicker {...store.albumPicker} />
{/if}
{#if store.settingsOpen && store.settings}
  <Settings />
{/if}
{#if store.exportDialog}
  <ExportDialog ids={store.exportDialog.ids} />
{/if}
{#if store.confirmation}
  <Confirm />
{/if}
<Toasts />

<style>
  .app {
    display: flex;
    height: 100%;
  }
  main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
  }
  .content {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .info {
    width: 300px;
    flex: none;
    border-left: 1px solid var(--sep);
    background: var(--bg);
    animation: slide 220ms var(--ease-out);
  }
  @keyframes slide {
    from {
      opacity: 0;
      transform: translateX(16px);
    }
  }
  .fatal {
    margin: auto;
    text-align: center;
  }
</style>
