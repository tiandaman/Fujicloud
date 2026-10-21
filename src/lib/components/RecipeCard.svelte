<script lang="ts">
  import { filmGradient } from '../film';
  import { recipeLines, recipeText } from '../recipe';
  import { store } from '../store.svelte';
  import type { Recipe } from '../types';
  import Icon from './Icon.svelte';

  let { recipe, recipeKey }: { recipe: Recipe; recipeKey: string | null } = $props();

  const row = $derived(recipeKey ? store.recipeByKey.get(recipeKey) : undefined);
  // The film simulation is the card's title, so it's not repeated below.
  const lines = $derived(recipeLines(recipe).filter(([k]) => k !== 'Film simulation'));
  let naming = $state(false);
  let draft = $state('');

  async function copy() {
    try {
      await navigator.clipboard.writeText(recipeText(recipe, row?.name));
      store.toast('Recipe copied');
    } catch {
      store.toast("Couldn't copy to the clipboard", 'error');
    }
  }

  function startNaming() {
    draft = row?.name ?? '';
    naming = true;
  }

  async function saveName() {
    naming = false;
    if (recipeKey) await store.nameRecipe(recipeKey, draft.trim() || null);
  }
</script>

<div class="card">
  <header style:background={filmGradient(recipe.filmSim)}>
    <div class="titles">
      {#if naming}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="name-input"
          bind:value={draft}
          placeholder="Name this recipe"
          autofocus
          onkeydown={(e) => {
            if (e.key === 'Enter') saveName();
            if (e.key === 'Escape') naming = false;
            e.stopPropagation();
          }}
          onblur={saveName}
        />
      {:else}
        <strong>{row?.name ?? recipe.filmSim ?? 'Recipe'}</strong>
        <span>{row?.name ? recipe.filmSim : 'Film recipe'}{row ? ` · used on ${row.count} photo${row.count === 1 ? '' : 's'}` : ''}</span>
      {/if}
    </div>
    <div class="tools">
      {#if recipeKey}
        <button title="Name this recipe" aria-label="Name this recipe" onclick={startNaming}><Icon name="edit" size={14} /></button>
      {/if}
      <button title="Copy recipe as text" aria-label="Copy recipe" onclick={copy}><Icon name="copy" size={14} /></button>
    </div>
  </header>
  {#if lines.length}
    <dl>
      {#each lines as [k, v] (k)}
        <dt>{k}</dt>
        <dd class="tabular">{v}</dd>
      {/each}
    </dl>
  {/if}
  {#if recipeKey && row && row.count > 1}
    <button class="all" onclick={() => store.setCollection({ type: 'recipe', key: recipeKey })}>
      Show all {row.count} photos with this recipe <Icon name="next" size={12} stroke={2.2} />
    </button>
  {/if}
</div>

<style>
  .card {
    border-radius: 10px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.035);
    box-shadow: inset 0 0 0 0.5px var(--sep);
  }
  header {
    position: relative;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    padding: 12px 10px 12px 13px;
    color: white;
  }
  header::before {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(to right, rgba(0, 0, 0, 0.45), rgba(0, 0, 0, 0.1));
  }
  .titles,
  .tools {
    position: relative;
  }
  .titles {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    line-height: 1.3;
  }
  .titles strong {
    font: 650 14px var(--font-display);
    text-shadow: 0 1px 3px rgba(0, 0, 0, 0.4);
  }
  .titles span {
    font-size: 11.5px;
    opacity: 0.85;
  }
  .name-input {
    width: 100%;
    height: 26px;
    padding: 0 8px;
    border: 0;
    border-radius: 6px;
    background: rgba(0, 0, 0, 0.45);
    color: white;
    font: 600 13px var(--font);
  }
  .tools {
    display: flex;
    gap: 2px;
  }
  .tools button {
    display: grid;
    place-items: center;
    width: 26px;
    height: 26px;
    border-radius: 6px;
    color: rgba(255, 255, 255, 0.85);
    background: rgba(0, 0, 0, 0.2);
  }
  .tools button:hover {
    background: rgba(0, 0, 0, 0.4);
    color: white;
  }
  dl {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 5px 12px;
    margin: 0;
    padding: 11px 13px 12px;
    font-size: 12px;
  }
  dt {
    color: var(--text-3);
    white-space: nowrap;
  }
  dd {
    margin: 0;
    text-align: right;
    font-weight: 550;
  }
  .all {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    width: 100%;
    padding: 8px;
    border-top: 1px solid var(--sep);
    color: var(--accent);
    font-size: 12px;
    font-weight: 550;
  }
  .all:hover {
    background: var(--hover);
  }
</style>
