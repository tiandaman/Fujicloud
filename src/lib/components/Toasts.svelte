<script lang="ts">
  import { store } from '../store.svelte';
</script>

<div class="toasts" class:raised={store.toolbelts > 0} aria-live="polite">
  {#each store.toasts as t (t.id)}
    <div class="toast" class:error={t.tone === 'error'}>
      {t.text}
      {#if t.action}
        <button
          class="action"
          onclick={() => {
            t.action?.run();
            store.toasts = store.toasts.filter((x) => x !== t);
          }}>{t.action.label}</button
        >
      {/if}
    </div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: 28px;
    transition: bottom 260ms var(--ease-out);
    z-index: 100;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    transform: translateX(-50%);
    pointer-events: none;
  }
  .toast {
    max-width: 560px;
    padding: 9px 16px;
    border-radius: 22px;
    background: rgba(44, 44, 48, 0.92);
    backdrop-filter: blur(20px) saturate(1.5);
    box-shadow: var(--shadow-md);
    font-weight: 500;
    animation: in 260ms var(--ease-out);
  }
  .action {
    margin-left: 10px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    font-weight: 600;
    pointer-events: auto;
    cursor: default;
  }
  .action:hover {
    text-decoration: underline;
  }
  .toast.error {
    background: rgba(90, 24, 22, 0.94);
    color: #ffd7d4;
  }
  @keyframes in {
    from {
      opacity: 0;
      transform: translateY(10px) scale(0.96);
    }
  }
  /* Above the selection toolbelt, not over it. */
  .toasts.raised {
    bottom: 92px;
  }
</style>
