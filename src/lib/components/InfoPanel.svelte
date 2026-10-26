<script lang="ts">
  import { api } from '../api';
  import { filmGradient, labelColors } from '../film';
  import {
    formatAperture,
    formatBytes,
    formatDuration,
    formatEv,
    formatFocal,
    formatLongDate,
    formatMegapixels,
    formatShutter,
    formatTime,
    kindLabel,
    plural,
  } from '../format';
  import { store } from '../store.svelte';
  import type { Detail, FaceInPhoto, Label } from '../types';
  import { placeName } from '../recipe';
  import Histogram from './Histogram.svelte';
  import RecipeCard from './RecipeCard.svelte';
  import Icon from './Icon.svelte';
  import Stars from './Stars.svelte';

  let { ids, dark = false }: { ids: number[]; dark?: boolean } = $props();

  const photo = $derived(ids.length === 1 ? store.byId.get(ids[0]) : undefined);
  /** Who made the camera, from its model name. */
  function maker(camera: string): string {
    if (/^(iphone|ipad)/i.test(camera)) return 'Apple';
    if (/^pixel/i.test(camera)) return 'Google';
    if (/^(x-|x\d|xf\d|x100|gfx|finepix)/i.test(camera)) return 'FUJIFILM';
    return 'Camera';
  }

  const twin = $derived(photo?.pairId != null ? store.byId.get(photo.pairId) : undefined);
  const many = $derived(ids.length > 1 ? ids.map((id) => store.byId.get(id)).filter((p) => !!p) : []);
  const pair = $derived(store.comparisonOf(ids));
  // Who's in the photo; refreshed when people are named or regrouped.
  let faces = $state<FaceInPhoto[]>([]);
  let naming = $state<Record<number, string>>({});
  $effect(() => {
    const id = photo?.id;
    void store.people;
    if (id == null || !(store.faces?.enabled || store.petStatus?.enabled)) {
      faces = [];
      return;
    }
    let live = true;
    api
      .facesIn(id)
      .then((f) => live && (faces = f))
      .catch(() => {});
    return () => {
      live = false;
    };
  });

  async function hideFace(face: FaceInPhoto) {
    faces = faces.filter((f) => f !== face);
    await store.hideFace(face.id);
  }

  async function nameFace(face: FaceInPhoto) {
    const name = naming[face.id]?.trim();
    if (!name) return;
    await store.nameFace(face.id, name);
    delete naming[face.id];
  }

  const inAlbums = $derived(photo ? store.albums.filter((a) => a.ids.includes(photo.id)) : []);
  const edits = $derived(photo ? (store.editsOf.get(photo.id) ?? (twin ? store.editsOf.get(twin.id) : undefined) ?? []) : []);

  let detail = $state<Detail | null>(null);
  let showExif = $state(false);

  // Debounced, so holding an arrow key doesn't queue a request per photo.
  $effect(() => {
    const id = photo?.id;
    detail = null;
    if (id == null) return;
    const t = setTimeout(async () => {
      try {
        const d = await api.photoDetail(id);
        if (photo?.id === id) detail = d;
      } catch {
        // The panel still shows what the index knows.
      }
    }, 90);
    return () => clearTimeout(t);
  });

  const exposure = $derived(
    photo
      ? [
          ['Focal', formatFocal(photo.focalLength)],
          ['Aperture', formatAperture(photo.aperture)],
          ['Shutter', formatShutter(photo.shutter)],
          ['ISO', photo.iso ? String(photo.iso) : null],
        ]
      : [],
  );
</script>

<div class="panel" class:dark>
  {#if photo}
    <section class="head">
      <div class="name">
        <h3 class="truncate" title={photo.name}>{photo.name}</h3>
        <span class="badge">{twin ? 'RAW+JPEG' : photo.kind.toUpperCase()}</span>
      </div>
      {#if photo.takenAt}
        <p class="muted">{formatLongDate(photo.takenAt)} · {formatTime(photo.takenAt)}</p>
      {/if}
    </section>

    <section>
      <Histogram {photo} />
      <div class="exposure">
        {#each exposure as [label, value] (label)}
          <div>
            <strong class="tabular">{value ?? '—'}</strong>
            <span>{label}</span>
          </div>
        {/each}
      </div>
      {#if detail && formatEv(detail.exposureComp)}
        <p class="ev muted">Exposure compensation {formatEv(detail.exposureComp)}</p>
      {/if}
    </section>

    {#if pair}
      {@const b = store.byId.get(pair.before)}
      {@const a = store.byId.get(pair.after)}
      <section class="edits">
        <h4>{photo.editOf != null ? 'Before & after' : edits.length > 1 ? `Before & after · ${plural(edits.length, 'edit')}` : 'Before & after'}</h4>
        <button class="ba" title="Compare before and after (C)" onclick={() => store.openCompare(pair)}>
          {#each [b, a] as p, i (i)}
            {#if p}
              <span class="side">
                <img src={api.thumbUrl(p)} alt="" draggable="false" />
                <em>{i === 0 ? 'Before' : 'After'}</em>
              </span>
            {/if}
          {/each}
          <span class="go"><Icon name="compare" size={15} /> Compare</span>
        </button>
        {#if photo.editOf != null && b}
          <button class="link truncate" title="Go to the original" onclick={() => (store.viewerId != null ? (store.viewerId = store.focusId = b.id) : (store.focusId = b.id))}>
            Original: {b.name}
          </button>
        {:else if edits.length > 1}
          <div class="others">
            {#each edits as e (e.id)}
              <button class="chip" title="Compare with {e.name}" onclick={() => store.openCompare({ before: pair.before, after: e.id })}>
                {e.kind === 'psd' ? 'PSD' : e.kind === 'tiff' ? 'TIFF' : 'JPEG'} · {e.name}
              </button>
            {/each}
          </div>
        {/if}
      </section>
    {/if}

    {#if detail?.recipe?.filmSim}
      <section><RecipeCard recipe={detail.recipe} recipeKey={photo.recipeKey} /></section>
    {/if}

    <section class="rows">
      {#if photo.filmSim && !detail?.recipe?.filmSim}
        <div class="row">
          <span class="swatch" style:background={filmGradient(photo.filmSim)}></span>
          <div><strong>{photo.filmSim}</strong><span class="muted">Film simulation</span></div>
        </div>
      {/if}
      {#if photo.camera}
        <div class="row">
          <span class="ico"><Icon name="camera" size={16} /></span>
          <div><strong>{photo.camera}</strong><span class="muted">{maker(photo.camera)}</span></div>
        </div>
      {/if}
      <!-- A phone's "lens" is the phone itself: already said above. -->
      {#if photo.lens && photo.lens !== photo.camera}
        <div class="row">
          <span class="ico"><Icon name="lens" size={16} /></span>
          <div><strong class="truncate">{photo.lens}</strong><span class="muted">Lens</span></div>
        </div>
      {/if}
      {#if photo.lat != null && photo.lon != null}
        <div class="row">
          <span class="ico"><Icon name="pin" size={16} /></span>
          <div class="grow">
            <strong class="truncate">{placeName(photo) ?? 'Somewhere on Earth'}</strong>
            <span class="muted tabular">
              {photo.gpsEstimated ? 'Estimated from photos taken around the same time' : `${photo.lat.toFixed(4)}, ${photo.lon.toFixed(4)}${detail?.altitude != null ? ` · ${Math.round(detail.altitude)} m` : ''}`}
            </span>
          </div>
          <button class="icon-btn" title="Show on map" aria-label="Show on map" onclick={() => store.showOnMap(photo)}><Icon name="globe" size={16} /></button>
        </div>
      {/if}
    </section>

    <section class="cull">
      <div class="cull-row">
        <Stars value={photo.rating} size={18} onrate={(r) => store.rate(r, [photo.id])} />
        <div class="flags">
          <button class="icon-btn" class:pick={photo.flag === 1} title="Pick (P)" onclick={() => store.flag(photo.flag === 1 ? 0 : 1, [photo.id])}>
            <Icon name="flag" size={16} fill={photo.flag === 1} />
          </button>
          <button class="icon-btn" class:reject={photo.flag === -1} title="Reject (X)" onclick={() => store.flag(photo.flag === -1 ? 0 : -1, [photo.id])}>
            <Icon name="x" size={16} />
          </button>
          <button class="icon-btn" class:fav={photo.favorite} title="Favourite (.)" onclick={() => store.favorite([photo.id])}>
            <Icon name="heart" size={16} fill={photo.favorite} />
          </button>
        </div>
      </div>
      <div class="labels">
        {#each Object.entries(labelColors) as [name, color], i (name)}
          <button
            class="dot"
            class:on={photo.label === name}
            style:background={color}
            title="{name}{i < 4 ? ` (${i + 6})` : ''}"
            aria-label="{name} label"
            onclick={() => store.label(name as Label, [photo.id])}
          ></button>
        {/each}
      </div>
      <div class="albums">
        {#each inAlbums as a (a.id)}
          <span class="album-chip">
            <button class="truncate" title="Show this album" onclick={() => store.setCollection({ type: 'album', id: a.id })}>{a.name}</button>
            <button class="remove" title="Remove from {a.name}" aria-label="Remove from {a.name}" onclick={() => store.removeFromAlbum(a.id, [photo.id])}>
              <Icon name="x" size={11} stroke={2.2} />
            </button>
          </span>
        {/each}
        <button class="add-album" onclick={(e) => (store.albumPicker = { x: e.clientX - 200, y: e.clientY + 12, ids: [photo.id] })}>
          <Icon name="plus" size={12} stroke={2.2} /> Album
        </button>
      </div>
      {#if faces.length}
        <div class="faces">
          {#each faces as f (f.id)}
            <div class="face-chip">
              <img src={api.faceUrl(f.id)} alt="" draggable="false" />
              {#if f.name && f.personId != null}
                <button class="truncate" title="Show photos of {f.name}" onclick={() => store.setCollection({ type: 'person', id: f.personId! })}>{f.name}</button>
              {:else}
                <input
                  placeholder={f.kind === 'pet' ? 'Which pet?' : "Who's this?"}
                  list={f.kind === 'pet' ? 'pet-face-names' : 'face-names'}
                  bind:value={naming[f.id]}
                  onkeydown={(e) => {
                    if (e.key === 'Enter') nameFace(f);
                    e.stopPropagation();
                  }}
                />
                <button class="dismiss" title={f.kind === 'pet' ? 'Hide: not my pet' : "Hide: I don't know them"} aria-label="Hide" onclick={() => hideFace(f)}>
                  <Icon name="x" size={10} stroke={2.4} />
                </button>
              {/if}
            </div>
          {/each}
          <datalist id="face-names">
            {#each store.names as n (n)}<option value={n}></option>{/each}
          </datalist>
          <datalist id="pet-face-names">
            {#each store.petNames as n (n)}<option value={n}></option>{/each}
          </datalist>
        </div>
      {/if}
    </section>

    <section class="file">
      <dl>
        {#if formatMegapixels(photo)}<dt>Size</dt><dd class="tabular">{formatMegapixels(photo)}</dd>{/if}
        {#if photo.duration}<dt>Length</dt><dd class="tabular">{formatDuration(photo.duration)}</dd>{/if}
        <dt>File</dt>
        <dd class="tabular">
          {formatBytes(photo.size)}{twin ? ` + ${formatBytes(twin.size)} ${twin.kind === 'video' ? 'Live movie' : kindLabel[twin.kind]}` : ''}
        </dd>
        <dt>Folder</dt>
        <dd class="path" title={photo.folder}>{photo.folder}</dd>
      </dl>
      <div class="actions">
        <button class="btn primary" onclick={() => store.edit([photo.id])}>
          <Icon name="edit" size={15} /> Edit in Photoshop
        </button>
        {#if store.smartReady}
          <button class="icon-btn" title="Find similar photos" onclick={() => store.findSimilar(photo.id)}><Icon name="similar" size={16} /></button>
        {/if}
        <button class="icon-btn" title="Show in Explorer" onclick={() => api.reveal(photo.id)}><Icon name="reveal" size={17} /></button>
        <button class="icon-btn" title="Open with default app" onclick={() => api.openDefault(photo.id)}><Icon name="open" size={16} /></button>
      </div>
    </section>

    {#if detail?.exif.length}
      <section>
        <button class="disclosure" onclick={() => (showExif = !showExif)}>
          <span class:open={showExif}><Icon name="next" size={12} stroke={2.2} /></span>
          All metadata
          <span class="faint">{detail.exif.length}</span>
        </button>
        {#if showExif}
          <dl class="exif">
            {#each detail.exif as [k, v] (k)}
              <dt>{k}</dt>
              <dd>{v}</dd>
            {/each}
          </dl>
        {/if}
      </section>
    {/if}
  {:else if many.length}
    <section class="multi">
      <div class="stack">
        {#each many.slice(0, 3) as p, i (p.id)}
          <img src={api.thumbUrl(p)} alt="" style:transform="rotate({(i - 1) * 6}deg) translateY({i === 1 ? -4 : 0}px)" />
        {/each}
      </div>
      <h3>{plural(many.length, 'photo')} selected</h3>
      <p class="muted">{formatBytes(many.reduce((n, p) => n + p.size, 0))}</p>
      <Stars value={many.every((p) => p.rating === many[0].rating) ? many[0].rating : 0} size={20} onrate={(r) => store.rate(r, ids)} />
      <div class="multi-actions">
        <button class="btn primary" onclick={() => store.edit(ids)}><Icon name="edit" size={15} /> Edit in Photoshop</button>
        {#if pair}
          <button class="btn" onclick={() => store.openCompare(pair)}><Icon name="compare" size={15} /> Compare side by side</button>
        {/if}
        <button class="btn" onclick={() => store.flag(1, ids)}><Icon name="flag" size={15} /> Pick all</button>
        <button class="btn" onclick={(e) => (store.albumPicker = { x: e.clientX - 120, y: e.clientY + 12, ids })}><Icon name="album" size={15} /> Add to album</button>
        <button class="btn subtle" onclick={() => (store.selection = new Set())}>Clear selection</button>
      </div>
    </section>
  {:else}
    <div class="none">
      <Icon name="info" size={28} stroke={1.3} />
      <p>Select a photo to see its details.</p>
      <p class="faint hint">Arrow keys move · <span class="kbd">Space</span> opens</p>
    </div>
  {/if}
</div>

<style>
  .panel {
    height: 100%;
    overflow-y: auto;
    padding: 4px 0 24px;
  }
  section {
    padding: 14px 18px;
    border-bottom: 1px solid var(--sep);
  }
  section:last-child {
    border-bottom: 0;
  }
  .head {
    padding-top: 16px;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  h3 {
    margin: 0;
    font: 600 15px/1.3 var(--font-display);
    min-width: 0;
  }
  .head p {
    margin: 3px 0 0;
    font-size: 12px;
  }
  .exposure {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    margin-top: 12px;
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.035);
    box-shadow: inset 0 0 0 0.5px var(--sep);
  }
  .exposure div {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 8px 2px 7px;
    gap: 1px;
  }
  .exposure div + div {
    border-left: 1px solid var(--sep);
  }
  .exposure strong {
    font-size: 12.5px;
    font-weight: 600;
    white-space: nowrap;
  }
  .exposure span {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-3);
  }
  .ev {
    margin: 8px 0 0;
    font-size: 11.5px;
  }
  .rows {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .row {
    display: flex;
    align-items: center;
    gap: 11px;
    min-width: 0;
  }
  .row > div {
    display: flex;
    flex-direction: column;
    min-width: 0;
    line-height: 1.3;
  }
  .row strong {
    font-weight: 550;
  }
  .row .grow {
    flex: 1;
  }
  .row span {
    font-size: 11.5px;
  }
  .swatch,
  .ico {
    flex: none;
    width: 30px;
    height: 30px;
    border-radius: 8px;
  }
  .swatch {
    box-shadow: inset 0 0 0 0.5px rgba(255, 255, 255, 0.2);
  }
  .ico {
    display: grid;
    place-items: center;
    background: rgba(255, 255, 255, 0.06);
    color: var(--text-2);
  }
  .cull-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .flags {
    display: flex;
    gap: 2px;
  }
  .pick {
    color: var(--pick) !important;
  }
  .reject {
    color: var(--danger) !important;
  }
  .labels {
    display: flex;
    gap: 9px;
    margin-top: 10px;
  }
  .dot {
    width: 15px;
    height: 15px;
    border-radius: 50%;
    opacity: 0.85;
    transition: transform 140ms var(--ease-out);
  }
  .dot:hover {
    transform: scale(1.15);
    opacity: 1;
  }
  .dot.on {
    opacity: 1;
    box-shadow:
      0 0 0 2px var(--bg),
      0 0 0 3.5px white;
  }
  .dark .dot.on {
    box-shadow:
      0 0 0 2px #161618,
      0 0 0 3.5px white;
  }
  dl {
    display: grid;
    grid-template-columns: 56px 1fr;
    gap: 5px 10px;
    margin: 0;
    font-size: 12px;
  }
  dt {
    color: var(--text-3);
  }
  dd {
    margin: 0;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  .path {
    font-family: var(--mono);
    font-size: 11px;
    color: var(--text-2);
    user-select: text;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 4px;
    margin-top: 14px;
  }
  .actions .btn {
    flex: 1;
  }
  .disclosure {
    display: flex;
    align-items: center;
    gap: 6px;
    font-weight: 550;
    color: var(--text-2);
  }
  .disclosure span:first-child {
    transition: transform 160ms var(--ease-out);
  }
  .disclosure span.open {
    transform: rotate(90deg);
  }
  .exif {
    grid-template-columns: 120px 1fr;
    margin-top: 10px;
    font-size: 11.5px;
    user-select: text;
  }
  .edits h4 {
    margin: 0 0 8px;
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .ba {
    position: relative;
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 3px;
    width: 100%;
    padding: 0;
    border-radius: 9px;
    overflow: hidden;
    background: var(--tile);
  }
  .ba .side {
    position: relative;
    aspect-ratio: 4 / 3;
  }
  .ba img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
    transition: transform 300ms var(--ease-out);
  }
  .ba:hover img {
    transform: scale(1.04);
  }
  .ba em {
    position: absolute;
    left: 6px;
    bottom: 6px;
    padding: 2px 6px;
    border-radius: 5px;
    background: rgba(0, 0, 0, 0.55);
    color: #fff;
    font-size: 10px;
    font-style: normal;
    font-weight: 600;
    letter-spacing: 0.03em;
    text-transform: uppercase;
  }
  .ba .go {
    position: absolute;
    left: 50%;
    top: 50%;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 11px;
    border-radius: 999px;
    transform: translate(-50%, -50%);
    background: rgba(20, 20, 22, 0.78);
    backdrop-filter: blur(10px);
    color: #fff;
    font-size: 12px;
    font-weight: 600;
    opacity: 0;
    transition: opacity 160ms ease;
  }
  .ba:hover .go,
  .ba:focus-visible .go {
    opacity: 1;
  }
  .link {
    display: block;
    max-width: 100%;
    margin-top: 8px;
    color: var(--accent);
    font-size: 12px;
  }
  .link:hover {
    text-decoration: underline;
  }
  .others {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 8px;
  }
  .chip {
    padding: 4px 8px;
    border-radius: 6px;
    background: var(--bg-input);
    color: var(--text-2);
    font-size: 11.5px;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chip:hover {
    background: var(--hover);
    color: var(--text);
  }
  .fav {
    color: #ff5a7a;
  }
  .albums {
    display: flex;
    flex-wrap: wrap;
    gap: 5px;
    margin-top: 12px;
  }
  .album-chip {
    display: inline-flex;
    align-items: center;
    max-width: 100%;
    height: 24px;
    padding: 0 3px 0 9px;
    border-radius: 12px;
    background: var(--bg-input);
    font-size: 12px;
  }
  .album-chip button.truncate {
    min-width: 0;
    color: var(--text);
  }
  .album-chip .remove {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    margin-left: 3px;
    border-radius: 50%;
    color: var(--text-3);
  }
  .album-chip .remove:hover {
    background: var(--hover);
    color: var(--text);
  }
  .faces {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 10px;
  }
  .face-chip {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    max-width: 100%;
    height: 28px;
    padding: 0 10px 0 2px;
    border-radius: 14px;
    background: var(--bg-input);
    font-size: 12px;
  }
  .face-chip img {
    width: 24px;
    height: 24px;
    flex: none;
    border-radius: 50%;
    object-fit: cover;
  }
  .face-chip button {
    color: var(--text);
  }
  .face-chip input {
    width: 92px;
    padding: 0;
    border: 0;
    background: none;
    color: var(--text);
    font: inherit;
  }
  .face-chip input:focus {
    outline: none;
  }
  .face-chip:has(.dismiss) {
    padding-right: 3px;
  }
  .face-chip .dismiss {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    flex: none;
    border-radius: 50%;
    color: var(--text-3);
  }
  .face-chip .dismiss:hover {
    background: var(--hover);
    color: var(--text);
  }
  .add-album {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    height: 24px;
    padding: 0 9px;
    border-radius: 12px;
    color: var(--text-2);
    font-size: 12px;
    box-shadow: inset 0 0 0 1px var(--sep);
  }
  .add-album:hover {
    color: var(--text);
    background: var(--hover);
  }
  .multi {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 6px;
    padding-top: 30px;
    border: 0;
  }
  .stack {
    position: relative;
    width: 120px;
    height: 100px;
    margin-bottom: 10px;
  }
  .stack img {
    position: absolute;
    inset: 10px 15px;
    width: 90px;
    height: 80px;
    object-fit: cover;
    border-radius: 6px;
    box-shadow: 0 6px 18px rgba(0, 0, 0, 0.5);
    outline: 2px solid rgba(255, 255, 255, 0.85);
  }
  .multi p {
    margin: 0 0 6px;
  }
  .multi-actions {
    display: flex;
    flex-direction: column;
    gap: 6px;
    width: 100%;
    margin-top: 14px;
  }
  .none {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 6px;
    color: var(--text-3);
    text-align: center;
    padding: 24px;
  }
  .none p {
    margin: 4px 0 0;
    color: var(--text-2);
  }
  .none .hint {
    color: var(--text-3);
    font-size: 12px;
  }
</style>
