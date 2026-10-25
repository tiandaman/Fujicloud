<script lang="ts">
  import { store } from '../store.svelte';

  const c = $derived(store.confirmation!);

  function focus(el: HTMLElement) {
    el.focus();
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape') store.answer(false);
    else if (e.key === 'Enter') store.answer(true);
    else return;
    e.preventDefault();
    e.stopImmediatePropagation();
  }}
/>

<div class="scrim" role="presentation" onclick={() => store.answer(false)}></div>
<div class="dialog" role="alertdialog" aria-modal="true" aria-labelledby="confirm-title" aria-describedby="confirm-body">
  <h2 id="confirm-title">{c.title}</h2>
  <p id="confirm-body">{c.body}</p>
  <footer>
    <button class="btn subtle" onclick={() => store.answer(false)}>Cancel</button>
    <button class="btn danger" use:focus onclick={() => store.answer(true)}>{c.action}</button>
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 90;
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
    z-index: 91;
    top: 50%;
    left: 50%;
    width: min(420px, calc(100vw - 48px));
    padding: 20px 20px 16px;
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
    margin: 0 0 8px;
    font: 650 15px var(--font-display);
  }
  p {
    margin: 0 0 18px;
    color: var(--text-2);
    font-size: 12.5px;
    line-height: 1.5;
  }
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
