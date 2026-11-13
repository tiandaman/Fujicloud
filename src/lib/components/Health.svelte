<script lang="ts">
  import { api } from '../api';
  import { formatAgo, formatCount, plural } from '../format';
  import { store } from '../store.svelte';
  import type { LogEntry, Pending } from '../types';
  import Icon from './Icon.svelte';

  // Opening it reads the database back too.
  let deep = $state(false);
  $effect(() => {
    store.loadHealth(true).then(() => (deep = true));
  });

  const h = $derived(store.health);
  const b = $derived(store.backup ?? h?.backup ?? null);

  function close() {
    store.healthOpen = false;
  }

  async function retry() {
    await api.retryFailed();
    store.toast('Trying again: sidecars, thumbnails and analysis');
    setTimeout(() => store.loadHealth(), 1500);
  }

  /** Rows whose list of photos is open, by what they're about. */
  let open = $state<Record<string, boolean>>({});
  /** The log's warnings and problems, once asked for. */
  let problems = $state<LogEntry[] | null>(null);

  async function toggleProblems() {
    problems = problems ? null : await api.recentProblems().catch(() => []);
  }

  function show(id: number) {
    close();
    store.viewerId = id;
    store.focusId = id;
  }

  /** `2026-11-13 17:40:09` → "13 Nov, 17:40". */
  function logTime(at: string) {
    const d = new Date(at.replace(' ', 'T'));
    return isNaN(d.getTime()) ? at : `${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}, ${at.slice(11, 16)}`;
  }
  /** A path in a message is long: its file name is what's read. */
  const tidy = (text: string) => text.replace(/(?:(?<![A-Za-z])[A-Za-z]:[\\/]|\\\\)[^:\n]*[\\/]([^\\/:\n]+)/g, '$1');

  type Tone = 'ok' | 'warn' | 'bad' | 'busy' | 'off';
  const analysis = (name: string, a: Pending | null): { tone: Tone; text: string; detail: string; ids: number[] } => ({ ...describe(name, a), ids: a?.unreadableIds ?? [] });
  const describe = (name: string, a: Pending | null): { tone: Tone; text: string; detail: string } =>
    !a
      ? { tone: 'off', text: `${name} is off`, detail: 'Switch it on in Settings.' }
      : a.todo > 0
        ? { tone: 'busy', text: `${name}: ${formatCount(a.todo)} still to look at`, detail: 'Working through them in the background.' + (a.unreadable ? ` ${plural(a.unreadable, 'photo')} couldn't be read.` : '') }
        : a.unreadable > 0
          ? { tone: 'warn', text: `${name}: ${plural(a.unreadable, 'photo')} couldn't be read`, detail: 'Damaged, or a kind of file that can’t be opened. Try again once the library is reachable.' }
          : { tone: 'ok', text: `${name}: every photo looked at`, detail: '' };

  const name = (path: string) => path.split(/[\\/]/).pop() ?? path;
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
<div class="dialog" role="dialog" aria-modal="true" aria-labelledby="health-title">
  <h2 id="health-title"><Icon name="check" size={17} /> Library health</h2>
  {#if !h}
    <p class="faint">Looking…</p>
  {:else}
    <div class="rows">
      {#snippet row(tone: Tone, text: string, detail: string, action?: { label: string; run: () => void })}
        <div class="row">
          <span class="dot {tone}"></span>
          <div class="what">
            <strong>{text}</strong>
            {#if detail}<span class="faint">{detail}</span>{/if}
          </div>
          {#if action}<button class="btn subtle" onclick={action.run}>{action.label}</button>{/if}
        </div>
      {/snippet}

      {@render row(
        h.databaseProblem ? 'bad' : deep ? 'ok' : 'busy',
        h.databaseProblem ? 'The library’s index is damaged' : deep ? `Index sound · ${plural(h.photos, 'photo')}` : 'Reading the index back…',
        h.databaseProblem ? `${h.databaseProblem}. Your photos are fine: the index can be rebuilt from them. Open the log folder and send the log for help.` : '',
        h.databaseProblem ? { label: 'Open log folder', run: () => api.openLogFolder() } : undefined,
      )}

      {#if h.offline.length}
        {@render row('warn', `${h.offline.join(', ')} can’t be reached`, 'Its photos browse from the cache; changes are saved when it’s back.', { label: 'Look again', run: () => api.rescan() })}
      {:else}
        {@render row('ok', 'Every library folder is reachable', '')}
      {/if}

      {#if h.held}
        {@render row('warn', `${plural(store.reviewTotal || h.held, 'change')} from other PCs waiting`, 'Nothing is applied until you’ve looked.', {
          label: 'Review',
          run: () => {
            close();
            store.reviewOpen = true;
          },
        })}
      {/if}

      {#if !h.sidecarsOn}
        {@render row('off', 'Sidecars are off', 'Stars, albums and names stay on this PC only. Switch on “Write ratings to XMP sidecars” in Settings to share them.')}
      {:else if h.sidecarsOwed}
        {@render row(
          h.offline.length ? 'busy' : 'warn',
          `${plural(h.sidecarsOwed, 'photo')} with changes not saved to the library yet`,
          h.offline.length ? 'They’re saved when the library is reachable again.' : 'Other PCs don’t see these changes until they’re saved.',
          h.offline.length ? undefined : { label: 'Save now', run: retry },
        )}
      {:else}
        {@render row('ok', 'Every change is saved to the library', '')}
      {/if}

      {#snippet which(key: string, ids: number[], total: number)}
        {#if ids.length}
          <div class="which">
            <button class="link" onclick={() => (open[key] = !open[key])}>{open[key] ? 'Hide them' : `Show which ${total === 1 ? 'photo' : 'photos'}`}</button>
            {#if open[key]}
              <ul>
                {#each ids as id (id)}
                  {@const p = store.byId.get(id)}
                  <li>
                    {#if p}
                      <button class="link name" title="Open this photo" onclick={() => show(id)}>{p.name}</button>
                      <span class="faint truncate" title={p.folder}>{p.folder}</span>
                      <button class="link" onclick={() => api.reveal(id).catch((e) => store.toast(String(e), 'error'))}>Show in folder</button>
                    {:else}
                      <span class="faint">A photo that’s no longer in the library</span>
                    {/if}
                  </li>
                {/each}
                {#if total > ids.length}<li class="faint">and {formatCount(total - ids.length)} more</li>{/if}
              </ul>
            {/if}
          </div>
        {/if}
      {/snippet}

      {#if h.unshown}
        {@render row('warn', `${plural(h.unshown, 'photo')} couldn’t be shown`, 'Damaged, or a kind of file that can’t be opened.', { label: 'Try again', run: retry })}
        {@render which('unshown', h.unshownIds, h.unshown)}
      {/if}

      {#each [analysis('Smart search', h.smart), analysis('People', h.faces), analysis('Pets', h.pets), analysis('Text in photos', h.text)] as a (a.text)}
        {@render row(a.tone, a.text, a.detail, a.tone === 'warn' ? { label: 'Try again', run: retry } : undefined)}
        {@render which(a.text, a.ids, a.ids.length)}
      {/each}

      {#if !b?.target}
        {@render row('off', 'No backup', 'Choose a backup folder in Settings to keep a second copy on another drive.', {
          label: 'Settings',
          run: () => {
            close();
            store.settingsOpen = true;
          },
        })}
      {:else}
        {@render row(
          b.error ? 'warn' : b.running ? 'busy' : 'ok',
          b.running ? `Backing up${b.total ? ` · ${formatCount(b.done)} of ${formatCount(b.total)}` : '…'}` : b.lastBackedUp ? `Backed up ${formatAgo(b.lastBackedUp)}` : 'Not backed up yet',
          b.error ?? `To ${b.target}`,
          b.running ? undefined : { label: 'Back up now', run: () => api.backupNow() },
        )}
        {#if b.checking}
          <div class="row">
            <span class="dot busy"></span>
            <div class="what">
              <strong>Checking the backup · {formatCount(b.checkDone)} of {formatCount(b.checkTotal)}</strong>
              <span class="faint">Reading every photo and its copy back. You can keep working; it carries on where it stopped if the app is closed.</span>
              <div class="bar"><i style:width="{(b.checkDone / Math.max(1, b.checkTotal)) * 100}%"></i></div>
            </div>
          </div>
        {:else if b.damaged.length || b.unreadable.length}
          <div class="row">
            <span class="dot bad"></span>
            <div class="what">
              <strong>
                {#if b.damaged.length}{plural(b.damaged.length, 'photo')} differ{b.damaged.length === 1 ? 's' : ''} from {b.damaged.length === 1 ? 'its' : 'their'} backup copy{/if}{#if b.damaged.length && b.unreadable.length} · {/if}{#if b.unreadable.length}{plural(b.unreadable.length, 'file')} couldn’t be read{/if}
              </strong>
              <span class="faint">
                One of the two copies is damaged, which can mean a drive is failing. Where the check can tell which, it says so; otherwise open the photo here: if it looks right, the library’s copy is the good one.
                To put a damaged copy right, replace it with the good one in File Explorer, then check again.
              </span>
              <ul>
                {#each b.damaged.slice(0, 8) as path, i (path)}
                  {@const side = b.damagedSide?.[i]}
                  <li class="truncate" title={path}>
                    {name(path)}
                    <strong class="side" class:lib={side === 'library'}>
                      {side === 'backup' ? 'The backup’s copy is damaged; the library’s is good' : side === 'library' ? 'The library’s copy is damaged; the backup’s is good' : 'Can’t tell which copy is damaged'}
                    </strong>
                    <span class="faint">{path}</span>
                  </li>
                {/each}
                {#each b.unreadable.slice(0, Math.max(0, 8 - b.damaged.length)) as path (path)}
                  <li class="truncate" title={path}>{name(path)} <span class="faint">couldn’t be read · {path}</span></li>
                {/each}
                {#if b.damaged.length + b.unreadable.length > 8}<li class="faint">and {b.damaged.length + b.unreadable.length - 8} more (listed in the log)</li>{/if}
              </ul>
            </div>
            <button class="btn subtle" onclick={() => api.checkBackup()}>Check again</button>
          </div>
        {:else}
          {@render row(
            b.lastChecked ? 'ok' : 'off',
            b.lastChecked ? `Backup checked ${formatAgo(b.lastChecked)}: ${formatCount(b.checkSame)} photos the same in both places` : 'Backup not checked yet',
            (b.checkWaiting ? `${plural(b.checkWaiting, 'photo')} not backed up yet. ` : '') + 'Checked every month: every photo and its copy are read back and compared.',
            { label: 'Check now', run: () => api.checkBackup() },
          )}
        {/if}
      {/if}
    </div>
  {/if}
  {#if problems}
    <div class="problems">
      <h3>Recent problems <span class="faint">newest first</span></h3>
      {#if !problems.length}
        <p class="faint">Nothing has gone wrong lately.</p>
      {:else}
        <ul>
          {#each problems as e, i (i)}
            <li title={e.text}>
              <span class="dot {e.problem ? 'bad' : 'warn'}"></span>
              <span class="faint when">{logTime(e.at)}</span>
              <span class="said">{tidy(e.text)}</span>
            </li>
          {/each}
        </ul>
      {/if}
    </div>
  {/if}
  <footer>
    <span>
      <button class="btn subtle" onclick={toggleProblems}>{problems ? 'Hide recent problems' : 'Recent problems'}</button>
      <button class="btn subtle" onclick={() => api.openLogFolder()}>Open log folder</button>
    </span>
    <button class="btn primary" onclick={close}>Done</button>
  </footer>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 80;
    background: rgba(0, 0, 0, 0.45);
  }
  .dialog {
    position: fixed;
    z-index: 81;
    top: 50%;
    left: 50%;
    display: flex;
    flex-direction: column;
    width: min(640px, calc(100vw - 48px));
    max-height: calc(100vh - 80px);
    padding: 18px 18px 14px;
    transform: translate(-50%, -50%);
    border-radius: var(--radius-lg);
    background: #1e1e21;
    box-shadow: var(--shadow-lg);
  }
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 0 0 10px;
    font: 650 15px var(--font-display);
  }
  h2 :global(svg) {
    color: var(--accent);
  }
  .rows {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0 -6px;
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 11px;
    padding: 9px 6px;
    border-top: 1px solid var(--sep);
  }
  .row:first-child {
    border-top: 0;
  }
  .dot {
    flex: none;
    width: 9px;
    height: 9px;
    margin-top: 4px;
    border-radius: 50%;
    background: var(--text-3);
  }
  .dot.ok {
    background: #30d158;
  }
  .dot.warn {
    background: #ffb340;
  }
  .dot.bad {
    background: #ff453a;
  }
  .dot.busy {
    background: var(--accent);
    animation: pulse 1.4s ease-in-out infinite;
  }
  .dot.off {
    background: none;
    box-shadow: inset 0 0 0 1.5px var(--text-3);
  }
  @keyframes pulse {
    50% {
      opacity: 0.35;
    }
  }
  .what {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 12.5px;
    line-height: 1.4;
  }
  .what strong {
    font-weight: 600;
  }
  .what .faint {
    font-size: 11.5px;
  }
  ul {
    margin: 4px 0 0;
    padding: 0;
    list-style: none;
    font-size: 11.5px;
  }
  .side {
    margin: 0 4px;
    font-weight: 600;
    color: #ffb340;
  }
  .side.lib {
    color: #ff453a;
  }
  .which {
    padding: 0 6px 9px 26px;
    margin-top: -5px;
    font-size: 11.5px;
  }
  .which ul {
    max-height: 170px;
    overflow-y: auto;
  }
  .which li {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 2px 0;
  }
  .which li .truncate {
    flex: 1;
    min-width: 0;
  }
  .link {
    flex: none;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font: inherit;
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .link.name {
    color: var(--text);
    font-weight: 600;
  }
  .problems {
    flex: none;
    max-height: 34vh;
    overflow-y: auto;
    margin-top: 8px;
    padding-top: 8px;
    border-top: 1px solid var(--sep);
  }
  .problems h3 {
    margin: 0 0 4px;
    font: 600 12.5px var(--font-display);
  }
  .problems h3 .faint {
    margin-left: 6px;
    font-weight: 400;
    font-size: 11.5px;
  }
  .problems li {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 3px 0;
    font-size: 11.5px;
    line-height: 1.4;
  }
  .problems .dot {
    width: 7px;
    height: 7px;
    margin: 0;
    align-self: center;
  }
  .when {
    flex: none;
    width: 86px;
  }
  .said {
    flex: 1;
    min-width: 0;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .truncate {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .bar {
    height: 3px;
    margin-top: 5px;
    border-radius: 2px;
    background: rgba(255, 255, 255, 0.08);
    overflow: hidden;
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  footer {
    display: flex;
    justify-content: space-between;
    gap: 8px;
    margin-top: 12px;
  }
</style>
