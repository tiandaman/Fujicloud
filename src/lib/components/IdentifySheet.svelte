<script lang="ts">
  // "Which pet is this?" in a small sheet over the page: the group's photos
  // to scroll through, a name to give (or one of the names already there to
  // tap), and on to the next group. Opens out of the face that was clicked
  // and goes back into it.
  import { onMount, tick } from 'svelte';
  import { api } from '../api';
  import { plural } from '../format';
  import { store } from '../store.svelte';
  import type { FaceInPhoto, Person, Subject } from '../types';
  import Icon from './Icon.svelte';

  let { id = $bindable(), kind, queue, onclose }: { id: number; kind: Subject; queue: Person[]; onclose: () => void } = $props();
  const pets = $derived(kind === 'pet');
  const group = $derived(store.personById.get(id));
  const at = $derived(queue.findIndex((p) => p.id === id));
  const named = $derived((pets ? store.pets : store.people).filter((p) => p.name && !p.hidden).sort((a, b) => b.photoIds.length - a.photoIds.length));
  const photos = $derived((group?.photoIds ?? []).map((pid) => store.byId.get(pid)).filter((p) => !!p));

  let name = $state('');
  let picked = $state<number[]>([]);
  let panel: HTMLDivElement;
  let scrim: HTMLDivElement;
  let input = $state<HTMLInputElement>();
  let closing = false;
  const EASE = 'cubic-bezier(0.32, 0.72, 0, 1)';

  /** Where each photo's animal is, to mark it when there are several in the picture. */
  let boxes = $state<Record<number, FaceInPhoto[]>>({});
  $effect(() => {
    const mine = id;
    for (const p of photos.slice(0, 60)) {
      if (boxes[p.id]) continue;
      api.facesIn(p.id).then((faces) => {
        if (mine === id) boxes[p.id] = faces.filter((f) => f.kind === kind);
      }).catch(() => {});
    }
  });

  /** The face on the page this sheet opens out of and closes into. */
  const origin = () => document.querySelector<HTMLElement>(`[data-person="${id}"] .face`)?.getBoundingClientRect();
  function from(): Keyframe {
    const o = origin();
    const r = panel.getBoundingClientRect();
    if (!o) return { transform: 'translateY(24px) scale(0.94)', opacity: 0 };
    const s = Math.max(0.08, o.width / r.width);
    return { transform: `translate(${o.left + o.width / 2 - (r.left + r.width / 2)}px, ${o.top + o.height / 2 - (r.top + r.height / 2)}px) scale(${s})`, opacity: 0.2 };
  }

  onMount(() => {
    panel.animate([from(), { transform: 'none', opacity: 1 }], { duration: 380, easing: EASE });
    scrim.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 260, easing: 'ease-out' });
    input?.focus();
  });

  async function close() {
    if (closing) return;
    closing = true;
    scrim.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 240, easing: 'ease-in', fill: 'forwards' });
    await panel.animate([{ transform: 'none', opacity: 1 }, from()], { duration: 280, easing: EASE, fill: 'forwards' }).finished.catch(() => {});
    onclose();
  }

  /** On to the next group still to name, or done. */
  async function next(step = 1) {
    const rest = queue.filter((p) => p.id !== id || step === 0);
    const to = queue[at + step] ?? rest.find((p) => p.id !== id);
    name = '';
    picked = [];
    if (!to || to.id === id) return close();
    await panel.animate([{ opacity: 1, transform: 'none' }, { opacity: 0, transform: `translateX(${-18 * step}px)` }], { duration: 120, easing: 'ease-in' }).finished.catch(() => {});
    id = to.id;
    await tick();
    panel.animate([{ opacity: 0, transform: `translateX(${18 * step}px)` }, { opacity: 1, transform: 'none' }], { duration: 200, easing: EASE });
    input?.focus();
  }

  async function give(as: string) {
    const to = as.trim();
    if (!to || !group) return;
    const after = queue[at + 1] ?? queue[at - 1];
    await store.namePerson(group.id, to);
    if (after) {
      name = '';
      picked = [];
      id = after.id;
      await tick();
      panel.animate([{ opacity: 0, transform: 'translateX(18px)' }, { opacity: 1, transform: 'none' }], { duration: 220, easing: EASE });
      input?.focus();
    } else close();
  }

  async function hide() {
    if (!group) return;
    const after = queue[at + 1] ?? queue[at - 1];
    await store.hidePeople([group.id]);
    if (after) id = after.id;
    else close();
  }

  async function notThese() {
    if (!group || !picked.length) return;
    const all = picked.length === photos.length;
    await store.removeFromPerson(group.id, picked);
    picked = [];
    if (all) next();
  }

  function seeAll() {
    const to = id;
    onclose();
    store.setCollection({ type: 'person', id: to });
  }

  // The group went (named elsewhere, merged): move on.
  $effect(() => {
    if (!group && !closing) {
      const to = queue.find((p) => store.personById.has(p.id));
      if (to) id = to.id;
      else onclose();
    }
  });
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key !== 'Escape' || store.confirmation) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    close();
  }}
/>

<div class="scrim" bind:this={scrim} role="presentation" onclick={close}></div>
<div class="sheet" bind:this={panel} role="dialog" aria-modal="true" aria-label={pets ? 'Which pet is this?' : "Who's this?"}>
  {#if group}
    <header>
      <img class="avatar" src={api.faceUrl(group.cover)} alt="" draggable="false" />
      <div class="title">
        <strong>{group.name ?? (pets ? 'Which pet is this?' : "Who's this?")}</strong>
        <span class="faint">{plural(photos.length, 'photo')}{queue.length > 1 && at >= 0 ? ` · ${at + 1} of ${queue.length} to name` : ''}</span>
      </div>
      {#if queue.length > 1}
        <button class="round" title="Previous" aria-label="Previous" disabled={at <= 0} onclick={() => next(-1)}><Icon name="back" size={14} /></button>
        <button class="round" title="Skip to the next" aria-label="Next" disabled={at >= queue.length - 1} onclick={() => next(1)}><Icon name="next" size={14} /></button>
      {/if}
      <button class="round" title="Close (Esc)" aria-label="Close" onclick={close}><Icon name="x" size={14} /></button>
    </header>

    <div class="name">
      <input
        bind:this={input}
        bind:value={name}
        placeholder={pets ? 'Name this pet' : 'Name this person'}
        onkeydown={(e) => {
          if (e.key === 'Enter') give(name);
          if (e.key !== 'Escape') e.stopPropagation();
        }}
      />
      <button class="btn primary" disabled={!name.trim()} onclick={() => give(name)}>Save</button>
    </div>
    {#if named.length}
      <div class="chips">
        {#each named.filter((p) => !name.trim() || p.name!.toLowerCase().includes(name.trim().toLowerCase())).slice(0, 14) as p (p.id)}
          <button class="chip" title="It's {p.name}" onclick={() => give(p.name!)}>
            <img src={api.faceUrl(p.cover)} alt="" draggable="false" />
            {p.name}
          </button>
        {/each}
      </div>
    {/if}

    <div class="photos">
      {#each photos as p (p.id)}
        {@const on = picked.includes(p.id)}
        {@const mine = (boxes[p.id] ?? []).filter((f) => f.personId === group.id)}
        {@const others = (boxes[p.id] ?? []).length - mine.length}
        <button
          class="photo"
          class:on
          style:aspect-ratio="{p.width || 3} / {p.height || 2}"
          title={on ? 'Unselect' : `Select if this isn't ${pets ? 'the same pet' : 'the same person'}`}
          onclick={() => (picked = on ? picked.filter((x) => x !== p.id) : [...picked, p.id])}
          ondblclick={() => store.open(p.id, photos.map((x) => x.id))}
        >
          <img src={api.thumbUrl(p)} alt={p.name} loading="lazy" draggable="false" />
          <!-- Marked only when there's more than one animal to tell apart. -->
          {#if others > 0}
            {#each mine as f (f.id)}
              <i class="box" style:left="{f.bbox[0] * 100}%" style:top="{f.bbox[1] * 100}%" style:width="{f.bbox[2] * 100}%" style:height="{f.bbox[3] * 100}%"></i>
            {/each}
          {/if}
          {#if on}<span class="tick"><Icon name="check" size={11} stroke={3} /></span>{/if}
        </button>
      {/each}
    </div>

    <footer>
      {#if picked.length}
        <button class="btn" onclick={notThese}>{picked.length === 1 ? 'This one isn’t' : `These ${picked.length} aren’t`} {pets ? 'the same pet' : 'the same person'}</button>
        <button class="btn subtle" onclick={() => (picked = [])}>Unselect</button>
      {:else}
        <button class="btn subtle" title={pets ? 'An animal that isn’t yours: hide it, and its new photos' : 'Someone you don’t know: hide them, and their new photos'} onclick={hide}>
          {pets ? 'Not my pet' : 'I don’t know them'}
        </button>
        <button class="btn subtle" onclick={seeAll}>Open all photos</button>
      {/if}
      <span class="faint hint">Click a photo that doesn’t belong · double-click to look closer</span>
    </footer>
  {/if}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(0, 0, 0, 0.42);
  }
  .sheet {
    position: fixed;
    z-index: 61;
    left: 50%;
    top: 50%;
    display: flex;
    flex-direction: column;
    width: min(560px, calc(100vw - 48px));
    max-height: min(640px, calc(100vh - 80px));
    margin: 0;
    translate: -50% -50%;
    padding: 16px 16px 12px;
    border-radius: 18px;
    background: #1f1f23;
    box-shadow:
      0 24px 70px rgba(0, 0, 0, 0.55),
      0 0 0 1px rgba(255, 255, 255, 0.07);
    transform-origin: center;
    will-change: transform, opacity;
  }
  header {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .avatar {
    width: 46px;
    height: 46px;
    border-radius: 50%;
    object-fit: cover;
    background: rgba(255, 255, 255, 0.06);
  }
  .title {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .title strong {
    font: 650 15px var(--font-display);
  }
  .title .faint {
    font-size: 11.5px;
  }
  .round {
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border: 0;
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-2);
    cursor: pointer;
  }
  .round:hover:not(:disabled) {
    background: rgba(255, 255, 255, 0.13);
    color: var(--text);
  }
  .round:disabled {
    opacity: 0.35;
    cursor: default;
  }
  .name {
    display: flex;
    gap: 8px;
    margin-top: 12px;
  }
  .name input {
    flex: 1;
    min-width: 0;
    height: 34px;
    padding: 0 12px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 10px;
    background: rgba(255, 255, 255, 0.05);
    color: var(--text);
    font-size: 13.5px;
    outline: none;
  }
  .name input:focus {
    border-color: var(--accent);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 9px;
  }
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 11px 0 3px;
    border: 0;
    border-radius: 14px;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text);
    font-size: 12.5px;
    cursor: pointer;
    transition: background 0.12s;
  }
  .chip:hover {
    background: var(--accent);
  }
  .chip img {
    width: 22px;
    height: 22px;
    border-radius: 50%;
    object-fit: cover;
  }
  .photos {
    flex: 1;
    min-height: 120px;
    display: flex;
    flex-wrap: wrap;
    align-content: flex-start;
    gap: 5px;
    margin: 12px -4px 0;
    padding: 0 4px 4px;
    overflow-y: auto;
    overscroll-behavior: contain;
  }
  .photo {
    position: relative;
    height: 108px;
    padding: 0;
    border: 0;
    border-radius: 7px;
    overflow: hidden;
    background: rgba(255, 255, 255, 0.05);
    cursor: pointer;
  }
  .photo img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .photo.on {
    outline: 2.5px solid #ff9f0a;
    outline-offset: -2.5px;
  }
  .photo.on img {
    opacity: 0.55;
  }
  .box {
    position: absolute;
    border: 2px solid #fff;
    border-radius: 6px;
    box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.5);
    pointer-events: none;
  }
  .tick {
    position: absolute;
    top: 5px;
    left: 5px;
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    border-radius: 50%;
    background: #ff9f0a;
    color: #000;
  }
  footer {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 10px;
  }
  .hint {
    margin-left: auto;
    font-size: 11px;
    text-align: right;
  }
</style>
