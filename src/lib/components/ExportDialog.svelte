<script lang="ts">
  import { plural } from '../format';
  import { store } from '../store.svelte';
  import type { ExportPreset } from '../types';
  import Icon from './Icon.svelte';

  let { ids }: { ids: number[] } = $props();

  const PRESETS: { id: ExportPreset; title: string; detail: string }[] = [
    { id: 'web', title: 'Web', detail: 'JPEG, 2048 px on the long side. Sharp on any screen, quick to send.' },
    { id: 'instagram', title: 'Instagram', detail: 'JPEG, 1080 px wide (up to 1350 tall), so Instagram doesn’t shrink it again.' },
    { id: 'full', title: 'Full-size JPEG', detail: 'Every pixel, at the best quality. Camera JPEGs are copied as they are.' },
    { id: 'original', title: 'Original files', detail: 'The files as they are, with the RAW or Live Photo movie.' },
  ];

  // The last choice is remembered on this PC.
  const remembered = (() => {
    try {
      return JSON.parse(localStorage.getItem('export') ?? '{}') as { preset?: ExportPreset; location?: boolean };
    } catch {
      return {};
    }
  })();
  let preset = $state<ExportPreset>(remembered.preset ?? 'web');
  let location = $state(remembered.location ?? false);

  const movies = $derived(ids.filter((id) => store.byId.get(id)?.kind === 'video').length);

  function go() {
    try {
      localStorage.setItem('export', JSON.stringify({ preset, location }));
    } catch {
      // Private mode: nothing to remember.
    }
    store.exportPhotos(ids, preset, location);
  }

  function close() {
    store.exportDialog = null;
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape') close();
    else if (e.key === 'Enter') go();
    else return;
    e.preventDefault();
    e.stopImmediatePropagation();
  }}
/>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="export-title">
  <h2 id="export-title"><Icon name="export" size={17} /> Export {plural(ids.length, 'photo')}</h2>
  <div class="presets" role="radiogroup" aria-label="Size">
    {#each PRESETS as p (p.id)}
      <button class="preset" class:on={preset === p.id} role="radio" aria-checked={preset === p.id} onclick={() => (preset = p.id)}>
        <strong>{p.title}</strong>
        <span>{p.detail}</span>
      </button>
    {/each}
  </div>
  <label class="check" class:disabled={preset === 'original'}>
    <input type="checkbox" bind:checked={location} disabled={preset === 'original'} />
    <span>
      Include location
      <small>{preset === 'original' ? 'Original files keep everything they have.' : 'Off keeps where you live out of photos you post.'}</small>
    </span>
  </label>
  {#if movies && preset !== 'original'}
    <p class="note">{plural(movies, 'movie')} will be copied as {movies === 1 ? 'it is' : 'they are'}.</p>
  {/if}
  <footer>
    <button class="btn subtle" onclick={close}>Cancel</button>
    <button class="btn primary" onclick={go}>Choose folder…</button>
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 80;
    background: rgba(0, 0, 0, 0.45);
    animation: fade 160ms ease-out;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
  }
  .dialog {
    position: fixed;
    z-index: 81;
    top: 50%;
    left: 50%;
    width: min(460px, calc(100vw - 48px));
    padding: 18px 18px 14px;
    transform: translate(-50%, -50%);
    border-radius: var(--radius-lg);
    background: #1e1e21;
    box-shadow: var(--shadow-lg);
    animation: rise 220ms var(--ease-out);
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translate(-50%, -48%) scale(0.98);
    }
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 14px;
    font: 650 15px var(--font-display);
  }
  h2 :global(svg) {
    color: var(--accent);
  }
  .presets {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 8px;
  }
  .preset {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 10px 12px;
    border: 0;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.04);
    box-shadow: inset 0 0 0 1px var(--sep);
    color: var(--text-2);
    font: inherit;
    font-size: 11.5px;
    line-height: 1.4;
    text-align: left;
    cursor: default;
  }
  .preset strong {
    color: var(--text);
    font-size: 13px;
    font-weight: 600;
  }
  .preset:hover {
    background: rgba(255, 255, 255, 0.07);
  }
  .preset.on {
    background: color-mix(in srgb, var(--accent) 16%, transparent);
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .check {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    margin: 14px 2px 0;
    font-size: 13px;
  }
  .check span {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .check small {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .check.disabled {
    opacity: 0.55;
  }
  .note {
    margin: 10px 2px 0;
    color: var(--text-3);
    font-size: 11.5px;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
</style>
