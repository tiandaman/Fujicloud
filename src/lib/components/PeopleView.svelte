<script lang="ts">
  import { api } from '../api';
  import { formatBytes, formatCount, plural } from '../format';
  import { store } from '../store.svelte';
  import type { Person, Subject } from '../types';
  import { marquee, touches, type Box } from '../marquee';
  import ActionBar, { type BarAction } from './ActionBar.svelte';
  import ContextMenu, { type MenuItem } from './ContextMenu.svelte';
  import Icon from './Icon.svelte';

  let { kind = 'person' }: { kind?: Subject } = $props();
  const pets = $derived(kind === 'pet');
  let regrouping = $state(false);

  async function regroup() {
    regrouping = true;
    await store.regroupPeople(kind);
    regrouping = false;
  }
  const everyone = $derived(pets ? store.pets : store.people);
  const named = $derived(everyone.filter((p) => p.name));
  // A group needs a couple of photos before it's worth asking about.
  const unnamed = $derived(everyone.filter((p) => !p.name && (p.photoIds.length > 1 || p.takenOut)));
  const status = $derived(pets ? store.petStatus : store.faces);
  const hidden = $derived((pets ? store.hiddenPets : store.hiddenPeople).filter((p) => p.photoIds.length > 0));
  const hideLabel = $derived(pets ? 'Hide: not my pet' : "Hide: I don't know them");
  let showHidden = $state(false);

  let drafts = $state<Record<number, string>>({});
  /** People picked for merging (Ctrl+click, or the tick on a face). */
  let picked = $state<number[]>([]);
  let renaming = $state<number | null>(null);
  let menu = $state<{ x: number; y: number; items: MenuItem[] } | null>(null);

  function open(p: Person, e?: MouseEvent) {
    if (e && (e.ctrlKey || e.metaKey || picked.length)) {
      toggle(p);
      return;
    }
    store.setCollection({ type: 'person', id: p.id });
  }

  function toggle(p: Person) {
    picked = picked.includes(p.id) ? picked.filter((id) => id !== p.id) : [...picked, p.id];
  }

  async function merge() {
    await store.mergePeople(picked);
    picked = [];
  }

  async function hide(ids: number[]) {
    picked = picked.filter((id) => !ids.includes(id));
    await store.hidePeople(ids);
  }

  async function save(p: Person) {
    const name = drafts[p.id]?.trim();
    if (!name) return;
    await store.namePerson(p.id, name);
    delete drafts[p.id];
    renaming = null;
  }

  function personMenu(e: MouseEvent, p: Person) {
    e.preventDefault();
    if (p.hidden) {
      menu = {
        x: e.clientX,
        y: e.clientY,
        items: [
          { label: 'Show again', icon: 'eye', action: () => store.hidePeople([p.id], false) },
          { label: 'Show photos', icon: 'photos', action: () => store.setCollection({ type: 'person', id: p.id }) },
        ],
      };
      return;
    }
    menu = {
      x: e.clientX,
      y: e.clientY,
      items: [
        { label: 'Show photos', icon: 'photos', action: () => store.setCollection({ type: 'person', id: p.id }) },
        { label: 'Merge with…', icon: 'people', action: () => (store.mergePicker = { x: e.clientX, y: e.clientY, id: p.id }) },
        { label: picked.includes(p.id) ? 'Unselect' : 'Select to merge', icon: 'check', action: () => toggle(p) },
        {
          label: p.name ? 'Rename…' : 'Add a name…',
          icon: 'edit',
          action: () => {
            drafts[p.id] = p.name ?? '';
            renaming = p.id;
          },
        },
        { separator: true },
        { label: hideLabel, icon: 'eye-off', action: () => hide([p.id]) },
        { label: 'Forget this name', icon: 'x', action: () => store.deletePerson(p.id), hidden: !p.name },
      ],
    };
  }

  // Drag a box over faces to select them.
  let canvas = $state<HTMLDivElement>();
  const boxSelect = {
    item: '.person, [data-person]',
    canvas: () => canvas,
    selected: () => picked,
    select: (ids: Set<number>) => (picked = [...ids]),
    hits: (b: Box) => {
      const c = canvas?.getBoundingClientRect();
      if (!canvas || !c) return [];
      return [...canvas.querySelectorAll<HTMLElement>('[data-person]')]
        .filter((el) => {
          const r = el.getBoundingClientRect();
          return touches(b, r.left - c.left, r.top - c.top, r.width, r.height);
        })
        .map((el) => Number(el.dataset.person));
    },
  };

  /** The toolbelt for faces picked: merge them, or hide them. */
  const one = $derived(picked.length === 1 ? store.personById.get(picked[0]) : undefined);
  const actions: BarAction[] = $derived([
    {
      icon: pets ? 'paw' : 'people',
      label: 'Merge',
      tone: 'primary',
      title: picked.length < 2 ? `Pick another to merge with, or use Merge with…` : `The same ${pets ? 'pet' : 'person'}: make them one`,
      disabled: picked.length < 2,
      run: merge,
    },
    {
      icon: 'similar',
      label: 'Merge with…',
      title: `Choose who ${one?.name ?? (pets ? 'this pet' : 'this person')} is the same as`,
      hidden: picked.length !== 1,
      run: (e: MouseEvent) => {
        const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
        const id = picked[0];
        picked = [];
        store.mergePicker = { x: r.left, y: Math.max(10, r.top - 348), id };
      },
    },
    {
      icon: 'edit',
      label: one?.name ? 'Rename' : 'Name',
      hidden: picked.length !== 1,
      run: () => {
        if (!one) return;
        drafts[one.id] = one.name ?? '';
        renaming = one.id;
        picked = [];
      },
    },
    { icon: 'eye-off', label: 'Hide', title: hideLabel, run: () => hide(picked) },
  ]);

  /** Renaming starts in the box; the "Add a name" boxes wait to be clicked. */
  function focus(el: HTMLInputElement, on: boolean) {
    if (!on) return;
    el.focus();
    el.select();
  }
</script>

{#snippet face(p: Person, size: number)}
  <div class="face-wrap" class:picked={picked.includes(p.id)} class:hidden={p.hidden} data-person={p.hidden ? undefined : p.id}>
    <button
      class="face"
      style:width="{size}px"
      style:height="{size}px"
      title={picked.length ? 'Select to merge' : 'Show photos (Ctrl+click to select)'}
      onclick={(e) => open(p, e)}
    >
      <img src={api.faceUrl(p.cover)} alt="" draggable="false" />
    </button>
    {#if p.hidden}
      <button class="corner show" title="Show again" aria-label="Show again" onclick={() => store.hidePeople([p.id], false)}>
        <Icon name="eye" size={12} stroke={2} />
      </button>
    {:else}
      <button class="tick" data-no-marquee aria-label={picked.includes(p.id) ? 'Unselect' : 'Select to merge'} onclick={() => toggle(p)}>
        <Icon name="check" size={12} stroke={3} />
      </button>
      {#if !p.name}
        <button class="corner dismiss" data-no-marquee title={hideLabel} aria-label="Hide" onclick={() => hide([p.id])}>
          <Icon name="x" size={11} stroke={2.6} />
        </button>
      {/if}
    {/if}
  </div>
{/snippet}

{#snippet nameInput(p: Person)}
  <input
    class="name-input"
    placeholder="Add a name"
    list={pets ? 'pet-names' : 'people-names'}
    bind:value={drafts[p.id]}
    use:focus={renaming === p.id}
    onkeydown={(e) => {
      if (e.key === 'Enter') save(p);
      if (e.key === 'Escape') renaming = null;
      e.stopPropagation();
    }}
    onblur={() => (renaming = null)}
  />
{/snippet}

<datalist id={pets ? 'pet-names' : 'people-names'}>
  {#each pets ? store.petNames : store.names as n (n)}<option value={n}></option>{/each}
</datalist>

<div class="people" use:marquee={boxSelect}>
<div class="canvas" bind:this={canvas}>
  {#if !status?.enabled && pets}
    <div class="intro">
      <Icon name="paw" size={44} stroke={1.2} />
      <h2>Find your pets</h2>
      <p>
        Fujicloud can find the cats and dogs in your photos and tell your pets apart, so you can name each one once and see every photo
        they're in. It all happens on this PC: no photo ever leaves it. Two models are downloaded once ({status?.downloadMb ?? 355} MB).
      </p>
      <button class="btn primary" onclick={() => store.setPets(true)}>Turn on pets</button>
    </div>
  {:else if !status?.enabled}
    <div class="intro">
      <Icon name="people" size={44} stroke={1.2} />
      <h2>Find the people in your photos</h2>
      <p>
        Fujicloud can find faces and group the photos of each person, so you can name them once and see every photo they're in. It all
        happens on this PC: no photo or face ever leaves it. Two models are downloaded once ({status?.downloadMb ?? 183} MB).
      </p>
      <button class="btn primary" onclick={() => store.setFaces(true)}>Turn on face recognition</button>
    </div>
  {:else}
    {#if status.phase === 'downloading' || status.phase === 'indexing'}
      <div class="progress">
        <span class="spin"><Icon name="sync" size={14} /></span>
        <span>
          {#if status.phase === 'downloading'}Downloading {pets ? 'pet models' : 'face models'}… {status.total
              ? `${formatBytes(status.done)} of ${formatBytes(status.total)}`
              : ''}
          {:else}Looking for {pets ? 'pets' : 'faces'}… {formatCount(status.done)} of {formatCount(status.total)} photos{/if}
        </span>
        <div class="bar"><i style:width="{status.total ? (status.done / status.total) * 100 : 5}%"></i></div>
      </div>
    {:else if status.phase === 'error'}
      <p class="error">{status.error}</p>
    {/if}

    {#if named.length}
      <section>
        <h2>{pets ? 'Pets' : 'People'}</h2>
        <div class="grid">
          {#each named as p (p.id)}
            <div class="person" oncontextmenu={(e) => personMenu(e, p)} role="presentation">
              {@render face(p, 112)}
              {#if renaming === p.id}
                {@render nameInput(p)}
              {:else}
                <strong class="truncate">{p.name}</strong>
              {/if}
              <span class="faint tabular">{plural(p.photoIds.length, 'photo')}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if unnamed.length}
      <section>
        <div class="section-head">
          <h2>{pets ? 'Which pet is this?' : "Who's this?"}</h2>
          {#if named.length}
            <button
              class="btn subtle regroup"
              disabled={regrouping}
              title={`Group these again around the ${pets ? 'pets' : 'people'} you've named, and take out of each named one the photos that don't look like the ones you confirmed`}
              onclick={regroup}
            >
              <Icon name="sync" size={13} />
              {regrouping ? 'Grouping…' : `Regroup around named ${pets ? 'pets' : 'people'}`}
            </button>
          {/if}
        </div>
        <p class="hint">
          {#if pets}
            Name a pet and Fujicloud finds them in new photos too. Using a name that's already taken adds the photos to that pet. Not yours?
            The × hides them, and their new photos stay hidden too.
          {:else}
            Name a face and Fujicloud finds them in new photos too. Using a name that's already taken adds the photos to that person. Don't
            know them? The × on their face hides them, and their new photos stay hidden too.
          {/if}
        </p>
        <div class="grid">
          {#each unnamed as p (p.id)}
            <div class="person" oncontextmenu={(e) => personMenu(e, p)} role="presentation">
              {@render face(p, 96)}
              {@render nameInput(p)}
              <span class="faint tabular">{plural(p.photoIds.length, 'photo')}</span>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if hidden.length}
      <section class="hidden-people">
        <button class="link" onclick={() => (showHidden = !showHidden)}>
          <Icon name={showHidden ? 'down' : 'next'} size={12} />
          {showHidden
            ? 'Hidden'
            : pets
              ? `Show ${plural(hidden.length, 'hidden pet')}`
              : `Show ${hidden.length === 1 ? '1 hidden person' : `${hidden.length} hidden people`}`}
        </button>
        {#if showHidden}
          <p class="hint">
            {pets ? "Animals that aren't yours. They're left out of Pets" : "People you don't know. They're left out of People"}, the sidebar
            and photo info.
          </p>
          <div class="grid">
            {#each hidden as p (p.id)}
              <div class="person" oncontextmenu={(e) => personMenu(e, p)} role="presentation">
                {@render face(p, 80)}
                {#if p.name}<strong class="truncate">{p.name}</strong>{/if}
                <span class="faint tabular">{plural(p.photoIds.length, 'photo')}</span>
                <button class="show-again" onclick={() => store.hidePeople([p.id], false)}>Show again</button>
              </div>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    {#if !named.length && !unnamed.length && status.phase !== 'downloading' && status.phase !== 'indexing'}
      <div class="intro">
        <Icon name={pets ? 'paw' : 'people'} size={44} stroke={1.2} />
        <h2>No {pets ? 'pets' : 'people'} found yet</h2>
        <p>
          {#if status.scanned}
            Searched {plural(status.scanned, 'photo')}. {pets ? 'Pets show up here once one appears' : 'People show up here once someone appears'} in
            at least two photos.
          {:else}
            {pets ? 'Pets' : 'Faces'} are looked for in the background as thumbnails are built.
          {/if}
        </p>
      </div>
    {/if}
  {/if}
</div>
</div>

{#if picked.length}
  <ActionBar count={picked.length} noun={picked.length === 1 ? (pets ? 'pet' : 'person') : pets ? 'pets' : 'people'} {actions} oncancel={() => (picked = [])} />
{/if}

<svelte:window
  onkeydown={(e) => {
    if (e.key === 'Escape' && picked.length && !store.mergePicker && !store.confirmation) picked = [];
  }}
/>

{#if menu}
  <ContextMenu {...menu} onclose={() => (menu = null)} />
{/if}

<style>
  .canvas {
    position: relative;
    min-height: 100%;
  }
  .people {
    flex: 1;
    min-width: 0;
    overflow-y: auto;
    padding: 18px 26px 40px;
  }
  section + section {
    margin-top: 26px;
  }
  .section-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 12px;
  }
  .regroup {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: none;
  }
  h2 {
    margin: 0 0 14px;
    font: 600 17px/1.2 var(--font-display);
  }
  .hint {
    margin: -6px 0 16px;
    color: var(--text-3);
    font-size: 12px;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(132px, 1fr));
    gap: 22px 14px;
  }
  .person {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    min-width: 0;
    text-align: center;
  }
  .person strong {
    max-width: 100%;
    font-weight: 600;
    font-size: 13px;
  }
  .person .faint {
    font-size: 11.5px;
  }
  .face-wrap {
    position: relative;
    border-radius: 50%;
  }
  .face-wrap.picked .face {
    box-shadow: 0 0 0 3px var(--accent);
  }
  .tick {
    position: absolute;
    top: 2px;
    right: 2px;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.55);
    box-shadow: inset 0 0 0 1.5px rgba(255, 255, 255, 0.8);
    color: transparent;
    opacity: 0;
    cursor: default;
    transition: opacity 120ms;
  }
  .face-wrap:hover .tick,
  .face-wrap.picked .tick {
    opacity: 1;
  }
  .face-wrap.picked .tick {
    background: var(--accent);
    box-shadow: none;
    color: #fff;
  }
  .corner {
    position: absolute;
    top: 2px;
    left: 2px;
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 50%;
    background: rgba(0, 0, 0, 0.6);
    box-shadow: inset 0 0 0 1px rgba(255, 255, 255, 0.35);
    color: #fff;
    opacity: 0;
    cursor: default;
    transition: opacity 120ms, background 120ms;
  }
  .face-wrap:hover .corner {
    opacity: 1;
  }
  .corner.dismiss:hover {
    background: #c9443c;
  }
  .corner.show:hover {
    background: var(--accent);
  }
  .face-wrap.hidden .face img {
    opacity: 0.55;
    filter: grayscale(0.6);
  }
  .hidden-people {
    padding-top: 6px;
  }
  .hidden-people .grid {
    grid-template-columns: repeat(auto-fill, minmax(104px, 1fr));
  }
  .link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    margin-bottom: 12px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--text-2);
    font: inherit;
    font-size: 12.5px;
    cursor: default;
  }
  .link:hover {
    color: var(--text);
  }
  .show-again {
    padding: 2px 9px;
    border: 0;
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-2);
    font: inherit;
    font-size: 11.5px;
    cursor: default;
  }
  .show-again:hover {
    background: var(--accent);
    color: #fff;
  }
  .face {
    padding: 0;
    border: 0;
    border-radius: 50%;
    overflow: hidden;
    background: var(--bg-elev-2);
    box-shadow: 0 0 0 1px var(--border);
    cursor: default;
    transition: transform 160ms var(--ease-out), box-shadow 160ms;
  }
  .face:hover {
    transform: scale(1.04);
    box-shadow: 0 0 0 2px var(--accent);
  }
  .face img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .name-input {
    width: 120px;
    padding: 4px 8px;
    border: 0;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.07);
    box-shadow: inset 0 0 0 1px var(--border);
    color: var(--text);
    font: inherit;
    font-size: 12.5px;
    text-align: center;
  }
  .name-input:focus {
    outline: none;
    box-shadow: inset 0 0 0 1.5px var(--accent);
  }
  .intro {
    display: flex;
    flex-direction: column;
    align-items: center;
    max-width: 460px;
    margin: 12vh auto 0;
    text-align: center;
    color: var(--text-2);
  }
  .intro :global(svg) {
    color: var(--text-3);
  }
  .intro h2 {
    margin: 14px 0 8px;
    color: var(--text);
  }
  .intro p {
    margin: 0 0 18px;
    font-size: 13px;
    line-height: 1.55;
  }
  .progress {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    margin-bottom: 20px;
    color: var(--text-2);
    font-size: 12.5px;
  }
  .progress .bar {
    flex-basis: 100%;
    height: 3px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .progress .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
    transition: width 300ms;
  }
  .spin {
    display: inline-grid;
    animation: spin 1.2s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .error {
    color: #ff8a80;
    font-size: 12.5px;
  }
</style>
