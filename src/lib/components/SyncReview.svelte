<script lang="ts">
  import { api } from '../api';
  import { formatAgo, formatDay, plural } from '../format';
  import { store } from '../store.svelte';
  import type { Marks, SyncChange } from '../types';
  import Icon from './Icon.svelte';

  // A folder moved or tidied on another PC can mean thousands of changes:
  // they're shown, and decided, a page at a time.
  const PAGE = 300;
  const changes = $derived(store.review.slice(0, PAGE));
  const total = $derived(Math.max(store.reviewTotal, changes.length));
  const more = $derived(total - changes.length);
  const deleted = $derived(changes.filter((c) => c.removed));
  const changed = $derived(changes.filter((c) => !c.removed));

  // Each photo's decision: take the other PC's change, or keep it as it is
  // here. Everything starts as "take", as it would without reviewing.
  let keep = $state<Record<number, boolean>>({});
  const kept = (c: SyncChange) => keep[c.id] ?? false;
  const keptCount = $derived(changes.filter(kept).length);

  function setAll(value: boolean) {
    const next: Record<number, boolean> = {};
    for (const c of changes) next[c.id] = value && (!c.removed || c.restorable);
    keep = next;
  }

  function done() {
    const apply: number[] = [];
    const reject: number[] = [];
    for (const c of changes) (kept(c) ? reject : apply).push(c.id);
    store.resolveReview(apply, reject);
  }

  async function takeEverything() {
    const ok = await store.ask(
      `Take all ${total.toLocaleString()} changes?`,
      'Every change from the other PC is applied here, the ones not shown too: photos deleted there are removed from this library as well.',
      'Take all',
    );
    if (ok) store.resolveReview([], [], true);
  }

  function close() {
    store.reviewOpen = false;
  }

  const FLAG: Record<number, string> = { 1: 'Pick', [-1]: 'Reject', 0: 'No flag' };
  const stars = (n: number) => (n ? '★'.repeat(n) : 'No stars');

  /** What would change, as short phrases. */
  function differences(before: Marks, after: Marks): { text: string; tone?: 'add' | 'remove' }[] {
    const out: { text: string; tone?: 'add' | 'remove' }[] = [];
    if (before.rating !== after.rating) out.push({ text: `${stars(before.rating)} → ${stars(after.rating)}` });
    if (before.flag !== after.flag) out.push({ text: `${FLAG[before.flag]} → ${FLAG[after.flag]}` });
    if ((before.label ?? null) !== (after.label ?? null)) out.push({ text: `${before.label ?? 'No label'} → ${after.label ?? 'No label'}` });
    if (before.favorite !== after.favorite) out.push({ text: after.favorite ? 'Favourite' : 'Not a favourite', tone: after.favorite ? 'add' : 'remove' });
    const has = (list: string[], a: string) => list.some((b) => b.toLowerCase() === a.toLowerCase());
    for (const a of after.albums) if (!has(before.albums, a)) out.push({ text: `Added to ${a}`, tone: 'add' });
    for (const a of before.albums) if (!has(after.albums, a)) out.push({ text: `Taken out of ${a}`, tone: 'remove' });
    return out;
  }

  /** "XPS-LAPTOP · 3 hours ago", where the change says. */
  const who = (c: SyncChange) => [c.by, c.at ? formatAgo(c.at) : null].filter(Boolean).join(' · ');
  const where = (c: SyncChange) => [c.takenAt ? formatDay(c.takenAt) : null, c.folder.split(/[\\/]/).filter(Boolean).slice(-2).join(' › ')].filter(Boolean).join(' · ');
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key !== 'Escape') return;
    close();
    e.preventDefault();
    e.stopImmediatePropagation();
  }}
/>

<div class="scrim" role="presentation" onclick={close}></div>
<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="review-title">
  <header>
    <h2 id="review-title"><Icon name="sync" size={17} /> Changes from other PCs</h2>
    <p class="hint">
      Another PC (or Lightroom) changed {plural(total, 'photo')} in the library. Nothing here has changed yet: choose what to take.
      Kept photos stay as they are on this PC.
    </p>
    <div class="all">
      <button class="btn subtle" onclick={() => setAll(false)}>Take all</button>
      <button class="btn subtle" onclick={() => setAll(true)}>Keep all as they are</button>
      {#if more > 0}
        <span class="faint paging">Showing the first {changes.length} of {total.toLocaleString()}. Done decides these; the rest come next.</span>
        <button class="btn subtle" onclick={takeEverything}>Take all {total.toLocaleString()}…</button>
      {/if}
    </div>
  </header>

  <div class="list">
    {#if deleted.length}
      <h3>Deleted on another PC · {deleted.length}</h3>
      {#each deleted as c (c.id)}
        <div class="row" class:kept={kept(c)}>
          <img src={api.thumbUrlById(c.id)} alt="" loading="lazy" />
          <div class="what">
            <strong class="truncate">{c.name}</strong>
            <span class="faint truncate">{where(c)}{who(c) ? ` · by ${who(c)}` : ''}</span>
            <span class="chips">
              <span class="chip remove">Deleted</span>
              {#if !c.restorable}<span class="chip note" title="It isn't in the library folder's .fujicloud-trash, so it was deleted for good">Can't be put back</span>{/if}
            </span>
          </div>
          <div class="seg" role="radiogroup" aria-label="Deletion of {c.name}">
            <button role="radio" aria-checked={!kept(c)} class:on={!kept(c)} onclick={() => (keep[c.id] = false)}>Delete here too</button>
            <button
              role="radio"
              aria-checked={kept(c)}
              class:on={kept(c)}
              disabled={!c.restorable}
              title={c.restorable ? 'Put the file back from the library folder’s trash' : 'The file isn’t in the library folder’s trash'}
              onclick={() => (keep[c.id] = true)}>Put it back</button
            >
          </div>
        </div>
      {/each}
    {/if}

    {#if changed.length}
      <h3>Changed on another PC · {changed.length}</h3>
      {#each changed as c (c.id)}
        <div class="row" class:kept={kept(c)}>
          <img src={api.thumbUrlById(c.id)} alt="" loading="lazy" />
          <div class="what">
            <strong class="truncate">{c.name}{c.twins.length ? (c.kind === 'raf' || c.kind === 'dng' ? ' + JPEG' : ' + RAW') : ''}</strong>
            <span class="faint truncate">{where(c)}{who(c) ? ` · by ${who(c)}` : ''}</span>
            <span class="chips">
              {#each c.after ? differences(c.before, c.after) : [] as d, i (i)}
                <span class="chip {d.tone ?? ''}">{d.text}</span>
              {/each}
            </span>
          </div>
          <div class="seg" role="radiogroup" aria-label="Change to {c.name}">
            <button role="radio" aria-checked={!kept(c)} class:on={!kept(c)} onclick={() => (keep[c.id] = false)}>Take</button>
            <button role="radio" aria-checked={kept(c)} class:on={kept(c)} onclick={() => (keep[c.id] = true)}>Keep mine</button>
          </div>
        </div>
      {/each}
    {/if}
  </div>

  <footer>
    <span class="faint">{changes.length - keptCount} to take · {keptCount} to keep</span>
    <button class="btn subtle" onclick={close}>Later</button>
    <button class="btn primary" onclick={done}>Done</button>
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
    display: flex;
    flex-direction: column;
    width: min(720px, calc(100vw - 48px));
    max-height: calc(100vh - 80px);
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
  header {
    padding: 18px 18px 10px;
    border-bottom: 1px solid var(--sep);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 6px;
    font: 650 15px var(--font-display);
  }
  h2 :global(svg) {
    color: var(--accent);
  }
  .hint {
    margin: 0 0 10px;
    color: var(--text-2);
    font-size: 12px;
    line-height: 1.45;
  }
  .all {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
  }
  .paging {
    margin-left: auto;
    font-size: 11.5px;
  }
  .list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 18px 10px;
  }
  h3 {
    margin: 12px 0 6px;
    color: var(--text-3);
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 7px 8px;
    border-radius: 10px;
  }
  .row:hover {
    background: rgba(255, 255, 255, 0.035);
  }
  .row img {
    flex: none;
    width: 64px;
    height: 48px;
    border-radius: 6px;
    object-fit: cover;
    background: rgba(255, 255, 255, 0.06);
  }
  .row.kept img {
    opacity: 0.55;
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12.5px;
  }
  .what strong {
    font-weight: 600;
  }
  .what .faint {
    font-size: 11px;
  }
  .truncate {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 2px;
  }
  .chip {
    padding: 1px 7px;
    border-radius: 99px;
    background: rgba(255, 255, 255, 0.07);
    color: var(--text-2);
    font-size: 11px;
    white-space: nowrap;
  }
  .chip.add {
    background: color-mix(in srgb, #30d158 18%, transparent);
    color: #7ee29b;
  }
  .chip.remove {
    background: color-mix(in srgb, #ff453a 18%, transparent);
    color: #ff8a82;
  }
  .chip.note {
    background: none;
    color: var(--text-3);
    box-shadow: inset 0 0 0 1px var(--sep);
  }
  .row.kept .chip {
    text-decoration: line-through;
    opacity: 0.6;
  }
  .seg {
    flex: none;
    display: flex;
    padding: 2px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.05);
  }
  .seg button {
    padding: 4px 10px;
    border: 0;
    border-radius: 6px;
    background: none;
    color: var(--text-2);
    font: inherit;
    font-size: 11.5px;
    cursor: default;
  }
  .seg button.on {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text);
  }
  .seg button:disabled {
    opacity: 0.4;
  }
  footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
    padding: 12px 18px 14px;
    border-top: 1px solid var(--sep);
  }
  footer .faint {
    margin-right: auto;
    font-size: 11.5px;
  }
</style>
