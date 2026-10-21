<script lang="ts" module>
  import type { Label } from '../types';
  import type { IconName } from './Icon.svelte';

  export type MenuItem =
    | { label: string; icon?: IconName; hint?: string; action: () => void; hidden?: boolean; checked?: boolean }
    | { separator: true }
    | { stars: true; value: number; onrate: (rating: number) => void }
    | { labels: true; value: Label | null; onlabel: (label: Label) => void };
</script>

<script lang="ts">
  import { labelColors } from '../film';
  import Icon from './Icon.svelte';
  import Stars from './Stars.svelte';

  let { x, y, items, onclose }: { x: number; y: number; items: MenuItem[]; onclose: () => void } = $props();

  let el: HTMLDivElement;
  let pos = $state({ left: 0, top: 0 });

  // Keep the menu on screen.
  $effect(() => {
    const r = el.getBoundingClientRect();
    pos = {
      left: Math.min(x, innerWidth - r.width - 8),
      top: y + r.height > innerHeight - 8 ? Math.max(8, y - r.height) : y,
    };
  });

  function run(fn: () => void) {
    onclose();
    fn();
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onclose();
    }
  }}
  onblur={onclose}
/>

<div class="scrim" role="presentation" onpointerdown={onclose} oncontextmenu={(e) => { e.preventDefault(); onclose(); }}></div>
<div class="menu" role="menu" tabindex="-1" bind:this={el} style:left="{pos.left}px" style:top="{pos.top}px">
  {#each items as item, i (i)}
    {#if 'separator' in item}
      <hr />
    {:else if 'stars' in item}
      <div class="row custom"><Stars value={item.value} size={16} onrate={(r) => run(() => item.onrate(r))} /></div>
    {:else if 'labels' in item}
      <div class="row custom labels">
        {#each Object.entries(labelColors) as [name, color] (name)}
          <button
            class="dot"
            class:on={item.value === name}
            style:background={color}
            aria-label={name}
            title={name}
            onclick={() => run(() => item.onlabel(name as Label))}
          ></button>
        {/each}
      </div>
    {:else if !item.hidden}
      <button class="row" role="menuitem" onclick={() => run(item.action)}>
        <span class="icon">
          {#if item.checked}<Icon name="check" size={14} stroke={2.2} />{:else if item.icon}<Icon name={item.icon} size={15} />{/if}
        </span>
        <span class="text">{item.label}</span>
        {#if item.hint}<span class="hint">{item.hint}</span>{/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
  }
  .menu {
    position: fixed;
    z-index: 91;
    min-width: 220px;
    padding: 5px;
    border-radius: 10px;
    background: rgba(40, 40, 44, 0.92);
    backdrop-filter: blur(30px) saturate(1.6);
    box-shadow: var(--shadow-md);
    animation: pop 120ms var(--ease-out);
    transform-origin: top left;
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: scale(0.97);
    }
  }
  .row {
    display: flex;
    align-items: center;
    gap: 9px;
    width: 100%;
    height: 30px;
    padding: 0 9px;
    border-radius: 6px;
    text-align: left;
  }
  button.row:hover {
    background: var(--accent);
    color: white;
  }
  button.row:hover .hint,
  button.row:hover .icon {
    color: rgba(255, 255, 255, 0.85);
  }
  .icon {
    width: 16px;
    display: grid;
    place-items: center;
    color: var(--text-2);
  }
  .text {
    flex: 1;
  }
  .hint {
    color: var(--text-3);
    font-size: 11.5px;
  }
  .custom {
    padding-left: 34px;
  }
  .labels {
    gap: 8px;
  }
  .dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    transition: transform 120ms var(--ease-out);
  }
  .dot:hover {
    transform: scale(1.2);
  }
  .dot.on {
    box-shadow:
      0 0 0 2px rgba(40, 40, 44, 1),
      0 0 0 3.5px white;
  }
  hr {
    border: 0;
    height: 1px;
    margin: 4px 6px;
    background: var(--sep);
  }
</style>
