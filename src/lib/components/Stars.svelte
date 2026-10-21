<script lang="ts">
  let { value, size = 15, onrate }: { value: number; size?: number; onrate: (rating: number) => void } = $props();
  let hover = $state(0);
  const shown = $derived(hover || value);
</script>

<div class="stars" role="radiogroup" aria-label="Rating" tabindex="-1" onpointerleave={() => (hover = 0)}>
  {#each [1, 2, 3, 4, 5] as n (n)}
    <button
      role="radio"
      aria-checked={value === n}
      aria-label="{n} star{n > 1 ? 's' : ''}"
      class:lit={n <= shown}
      class:preview={hover > 0}
      style:font-size="{size}px"
      onpointerenter={() => (hover = n)}
      onclick={() => onrate(value === n ? 0 : n)}
    >★</button>
  {/each}
</div>

<style>
  .stars {
    display: inline-flex;
    gap: 1px;
  }
  button {
    width: 1.35em;
    height: 1.5em;
    line-height: 1;
    color: rgba(255, 255, 255, 0.2);
    transition:
      color 100ms,
      transform 140ms var(--ease-out);
  }
  button.lit {
    color: var(--star);
  }
  button.lit.preview {
    color: #ffe066;
  }
  button:hover {
    transform: scale(1.18);
  }
</style>
