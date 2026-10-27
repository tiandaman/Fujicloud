<script lang="ts" module>
  import type { IconName } from './Icon.svelte';

  export type BarAction = {
    icon: IconName;
    label: string;
    run: (e: MouseEvent) => void;
    tone?: 'primary' | 'danger';
    hidden?: boolean;
    disabled?: boolean;
    /** Tooltip: what it does, and its shortcut. */
    title?: string;
  };
</script>

<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import { store } from '../store.svelte';
  import { cubicOut } from 'svelte/easing';
  import Icon from './Icon.svelte';

  /** What's selected, and the actions for it, floating over the bottom of the page. */
  let {
    count,
    noun,
    actions,
    oncancel,
    extra,
  }: { count: number; noun: string; actions: BarAction[]; oncancel: () => void; extra?: Snippet } = $props();

  const shown = $derived(actions.filter((a) => !a.hidden));

  // Toasts rise above it while it's here.
  onMount(() => {
    store.toolbelts++;
    return () => store.toolbelts--;
  });

  /** Rises into place (and sinks away), keeping its centring. */
  function rise(_: Element, { out = false } = {}) {
    return {
      duration: out ? 160 : 340,
      easing: out ? cubicOut : (t: number) => 1 - Math.pow(1 - t, 4),
      css: (t: number) => {
        const y = (1 - t) * (out ? 12 : 22);
        const s = 0.94 + 0.06 * t;
        return `opacity: ${Math.min(1, t * 1.6)}; transform: translateX(-50%) translateY(${y}px) scale(${s});`;
      },
    };
  }
</script>

<div class="toolbelt" role="toolbar" aria-label="{count} {noun} selected" in:rise out:rise={{ out: true }}>
  <div class="count">
    {#key count}<span class="num tabular">{count}</span>{/key}
    <span class="noun">{noun}</span>
  </div>
  {#if extra}
    <span class="sep"></span>
    <div class="extra">{@render extra()}</div>
  {/if}
  <span class="sep"></span>
  {#each shown as a, i (a.label)}
    <button
      class="act"
      class:primary={a.tone === 'primary'}
      class:danger={a.tone === 'danger'}
      style:--i={i}
      title={a.title ?? a.label}
      disabled={a.disabled}
      onclick={(e) => a.run(e)}
    >
      <Icon name={a.icon} size={16} stroke={1.8} />
      <span class="label">{a.label}</span>
    </button>
  {/each}
  <span class="sep"></span>
  <button class="close" title="Clear selection (Esc)" aria-label="Clear selection" onclick={oncancel}>
    <Icon name="x" size={14} stroke={2.2} />
  </button>
</div>

<style>
  .toolbelt {
    position: absolute;
    left: 50%;
    bottom: 22px;
    z-index: 40;
    display: flex;
    align-items: center;
    gap: 2px;
    max-width: calc(100% - 32px);
    padding: 6px;
    transform: translateX(-50%);
    border-radius: 16px;
    background: linear-gradient(180deg, rgba(48, 48, 54, 0.86), rgba(34, 34, 38, 0.9));
    backdrop-filter: blur(24px) saturate(1.7);
    -webkit-backdrop-filter: blur(24px) saturate(1.7);
    box-shadow:
      0 18px 50px rgba(0, 0, 0, 0.5),
      0 2px 8px rgba(0, 0, 0, 0.3),
      inset 0 0.5px 0 rgba(255, 255, 255, 0.14),
      0 0 0 0.5px rgba(255, 255, 255, 0.1);
    font-size: 12.5px;
    white-space: nowrap;
  }
  .count {
    display: flex;
    align-items: baseline;
    gap: 6px;
    padding: 0 10px 0 8px;
    color: var(--text-2);
  }
  .num {
    display: inline-grid;
    place-items: center;
    min-width: 26px;
    height: 24px;
    padding: 0 8px;
    border-radius: 12px;
    background: var(--accent);
    color: #fff;
    font-weight: 600;
    font-size: 12.5px;
    animation: bump 280ms var(--ease-out);
  }
  .sep {
    width: 1px;
    height: 22px;
    margin: 0 4px;
    background: rgba(255, 255, 255, 0.1);
    flex: none;
  }
  .extra {
    display: flex;
    align-items: center;
    padding: 0 4px;
  }
  .act {
    display: flex;
    align-items: center;
    gap: 7px;
    height: 34px;
    padding: 0 11px;
    border-radius: 10px;
    color: var(--text);
    opacity: 0;
    animation: pop 300ms var(--ease-out) forwards;
    animation-delay: calc(60ms + var(--i) * 22ms);
    transition:
      background 120ms,
      transform 120ms var(--ease-out);
  }
  .act:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.09);
  }
  .act:active:not(:disabled) {
    transform: scale(0.95);
  }
  .act:disabled {
    color: var(--text-3);
  }
  .act.primary {
    background: var(--accent);
    color: #fff;
  }
  .act.primary:hover:not(:disabled) {
    background: var(--accent-hover);
  }
  .act.primary:disabled {
    background: rgba(10, 132, 255, 0.35);
    color: rgba(255, 255, 255, 0.6);
  }
  .act.danger:hover:not(:disabled) {
    background: rgba(255, 69, 58, 0.16);
    color: var(--danger);
  }
  .close {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    margin-left: 2px;
    border-radius: 50%;
    color: var(--text-2);
    transition: background 120ms;
  }
  .close:hover {
    background: rgba(255, 255, 255, 0.1);
    color: var(--text);
  }
  /* Narrow windows: icons, with the words in their tooltips. */
  @media (max-width: 1180px) {
    .act:not(.primary) .label {
      display: none;
    }
    .act:not(.primary) {
      padding: 0 9px;
    }
  }
  @keyframes pop {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
    to {
      opacity: 1;
      transform: none;
    }
  }
  @keyframes bump {
    from {
      transform: scale(1.18);
    }
    to {
      transform: none;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .act,
    .num {
      animation: none;
      opacity: 1;
    }
  }
</style>
