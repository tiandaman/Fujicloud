<script lang="ts">
  import { store } from '../store.svelte';
</script>

<div class="toasts" aria-live="polite">
  {#each store.toasts as t (t.id)}
    <div class="toast" class:error={t.tone === 'error'}>{t.text}</div>
  {/each}
</div>

<style>
  .toasts {
    position: fixed;
    left: 50%;
    bottom: 28px;
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
</style>
