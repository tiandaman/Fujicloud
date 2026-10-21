<script lang="ts">
  import { onMount, tick, untrack } from 'svelte';
  import { api } from '../api';
  import { formatDateTime, formatCount } from '../format';
  import { store } from '../store.svelte';
  import Icon from './Icon.svelte';
  import InfoPanel from './InfoPanel.svelte';
  import Stars from './Stars.svelte';

  const INFO_W = 320;
  const PAD_TOP = 60;
  const PAD_BOTTOM = 92;
  const PAD_X = 28;
  const STRIP = 54;
  const STRIP_GAP = 6;
  const EASE = 'cubic-bezier(0.2, 0.9, 0.25, 1)';

  let vw = $state(innerWidth);
  let vh = $state(innerHeight);
  let frame = $state<HTMLDivElement>();
  let backdrop: HTMLDivElement;

  const id = $derived(store.viewerId!);
  const photo = $derived(store.byId.get(id)!);
  const order = $derived(store.navOrder);
  const idx = $derived(order.findIndex((p) => p.id === id));
  const pair = $derived(store.comparisonFor(photo));
  const stageW = $derived(vw - (store.showInfo ? INFO_W : 0));

  /** Where the photo sits at "fit" zoom, in window coordinates. */
  const fit = $derived.by(() => {
    const a = photo && photo.width && photo.height ? photo.width / photo.height : 1.5;
    const aw = stageW - PAD_X * 2;
    const ah = vh - PAD_TOP - PAD_BOTTOM;
    let w = aw;
    let h = aw / a;
    if (h > ah) {
      h = ah;
      w = ah * a;
    }
    return { x: (stageW - w) / 2, y: PAD_TOP + (ah - h) / 2, w, h };
  });

  // ---- zoom & pan ----------------------------------------------------------------
  // The frame is laid out at `fit` and transformed with origin 0 0, so a
  // window point (cx, cy) shows frame-local point ((cx - fit.x - tx) / scale, …).

  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let smooth = $state(false);
  let zoomPill = $state(false);
  let pillTimer: ReturnType<typeof setTimeout>;
  let wantOriginal = $state(false);

  const oneToOne = $derived(photo ? Math.max(1, photo.width / (fit.w * devicePixelRatio)) : 1);
  const maxScale = $derived(Math.max(4, oneToOne * 2));
  const zoomLabel = $derived(scale <= 1.001 ? 'Fit' : `${Math.round((scale / oneToOne) * 100)}%`);

  function clampPan(s: number, x: number, y: number): [number, number] {
    const dw = fit.w * s;
    const dh = fit.h * s;
    x = dw <= stageW ? (fit.w - dw) / 2 : Math.min(-fit.x, Math.max(stageW - dw - fit.x, x));
    y = dh <= vh ? (fit.h - dh) / 2 : Math.min(-fit.y, Math.max(vh - dh - fit.y, y));
    return [x, y];
  }

  function zoomTo(s: number, cx: number, cy: number, animate = false) {
    s = Math.min(maxScale, Math.max(1, s));
    const u = (cx - fit.x - tx) / scale;
    const v = (cy - fit.y - ty) / scale;
    [tx, ty] = clampPan(s, cx - fit.x - s * u, cy - fit.y - s * v);
    scale = s;
    smooth = animate;
    // Past what the 2560px preview can show sharply, fetch the full file.
    if (s * Math.max(fit.w, fit.h) * devicePixelRatio > 2600) wantOriginal = true;
    zoomPill = true;
    clearTimeout(pillTimer);
    pillTimer = setTimeout(() => (zoomPill = false), 1100);
  }

  function resetZoom(animate = true) {
    smooth = animate;
    scale = 1;
    tx = 0;
    ty = 0;
  }

  function toggleZoom(cx = fit.x + fit.w / 2, cy = fit.y + fit.h / 2) {
    if (scale > 1.01) resetZoom();
    else zoomTo(Math.max(oneToOne, 2), cx, cy, true);
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    // Pinch gestures arrive as ctrl+wheel with small deltas.
    zoomTo(scale * Math.exp(-e.deltaY * (e.ctrlKey ? 0.012 : 0.0022)), e.clientX, e.clientY);
  }

  // ---- drag: pan when zoomed, swipe to navigate when not ---------------------------

  let drag = $state<{ x: number; y: number; tx: number; ty: number; moved: boolean } | null>(null);
  let swipe = $state(0);

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    drag = { x: e.clientX, y: e.clientY, tx, ty, moved: false };
    smooth = false;
  }

  function onPointerMove(e: PointerEvent) {
    showChrome();
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true;
    if (scale > 1.001) [tx, ty] = clampPan(scale, drag.tx + dx, drag.ty + dy);
    else swipe = dx;
  }

  function onPointerUp() {
    if (!drag) return;
    const wasSwipe = scale <= 1.001 && Math.abs(swipe) > 90;
    if (wasSwipe) go(swipe < 0 ? 1 : -1);
    else if (swipe) smooth = true;
    swipe = 0;
    drag = null;
  }

  // ---- navigation ------------------------------------------------------------------

  function go(delta: number) {
    const next = order[idx + delta];
    if (!next) return;
    store.viewerId = next.id;
    store.focusId = next.id;
  }

  // Each photo starts at fit, and we warm up the neighbours' previews.
  // Only `id` is tracked: re-rating the photo replaces the photo list, and
  // that must not throw away the current zoom.
  $effect(() => {
    void id;
    untrack(() => {
      resetZoom(false);
      wantOriginal = false;
      for (const d of [1, -1, 2]) {
        const p = order[idx + d];
        if (p) new Image().src = api.previewUrl(p);
      }
    });
  });

  // If the photo disappears (e.g. filtered out by a rating change), close.
  $effect(() => {
    if (!photo) store.viewerId = null;
  });

  // ---- open / close animations -----------------------------------------------------

  let closing = false;

  /** Keyframe that makes the frame look exactly like the grid tile. */
  function tileKeyframe(t: DOMRect): Keyframe {
    const s = Math.max(t.width / fit.w, t.height / fit.h);
    const x = t.x + t.width / 2 - fit.x - (s * fit.w) / 2;
    const y = t.y + t.height / 2 - fit.y - (s * fit.h) / 2;
    const ix = (fit.w - t.width / s) / 2;
    const iy = (fit.h - t.height / s) / 2;
    return { transform: `translate(${x}px, ${y}px) scale(${s})`, clipPath: `inset(${iy}px ${ix}px round ${3 / s}px)` };
  }

  const fitKeyframe = (): Keyframe => ({
    transform: `translate(${tx}px, ${ty}px) scale(${scale})`,
    clipPath: 'inset(0px 0px round 0px)',
  });

  onMount(() => {
    const rect = store.grid?.rectOf(id);
    store.hiddenId = rect ? id : null;
    backdrop.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 240, easing: 'ease-out' });
    const anim = rect
      ? frame!.animate([tileKeyframe(rect), fitKeyframe()], { duration: 360, easing: EASE })
      : frame!.animate([{ opacity: 0, transform: 'scale(0.96)' }, { opacity: 1, transform: 'none' }], { duration: 220, easing: EASE });
    anim.finished.then(() => (store.hiddenId = null)).catch(() => {});
    showChrome();
  });

  async function close() {
    if (closing) return;
    closing = true;
    chrome = false;
    if (await api.isFullscreen()) await api.setFullscreen(false);
    store.grid?.reveal(id);
    await tick();
    const rect = store.grid?.rectOf(id);
    store.hiddenId = rect ? id : null;
    const from = fitKeyframe();
    backdrop.animate([{ opacity: 1 }, { opacity: 0 }], { duration: 260, easing: 'ease-in', fill: 'forwards' });
    if (!frame) {
      store.viewerId = null;
      store.viewerList = null;
      return;
    }
    const anim = rect
      ? frame.animate([from, tileKeyframe(rect)], { duration: 300, easing: EASE, fill: 'forwards' })
      : frame.animate([{ opacity: 1 }, { opacity: 0, transform: 'scale(0.96)' }], { duration: 180, fill: 'forwards' });
    await anim.finished.catch(() => {});
    store.hiddenId = null;
    store.viewerId = null;
    store.viewerList = null;
  }

  // ---- auto-hiding chrome ---------------------------------------------------------

  let chrome = $state(true);
  let chromeTimer: ReturnType<typeof setTimeout>;
  let overChrome = false;

  function showChrome() {
    if (closing) return;
    chrome = true;
    clearTimeout(chromeTimer);
    chromeTimer = setTimeout(() => {
      if (!overChrome) chrome = false;
    }, 2400);
  }

  // ---- keyboard ---------------------------------------------------------------------

  async function onKey(e: KeyboardEvent) {
    if (store.settingsOpen || store.compare || (e.target as HTMLElement).closest('input, textarea')) return;
    switch (e.key) {
      case 'ArrowRight':
      case 'ArrowDown':
        go(1);
        break;
      case 'ArrowLeft':
      case 'ArrowUp':
        go(-1);
        break;
      case 'Home':
        if (order[0]) store.viewerId = order[0].id;
        break;
      case 'End':
        if (order.length) store.viewerId = order[order.length - 1].id;
        break;
      case 'Escape':
      case ' ':
      case 'Backspace':
        close();
        break;
      case 'z':
      case 'Z':
        toggleZoom();
        break;
      case '=':
      case '+':
        zoomTo(scale * 1.5, fit.x + fit.w / 2, fit.y + fit.h / 2, true);
        break;
      case '-':
        zoomTo(scale / 1.5, fit.x + fit.w / 2, fit.y + fit.h / 2, true);
        break;
      case 'f':
      case 'F':
        if (e.ctrlKey || e.metaKey) return;
        await api.setFullscreen(!(await api.isFullscreen()));
        break;
      default:
        return;
    }
    e.preventDefault();
    showChrome();
  }

  function fadeIn(img: HTMLImageElement) {
    const done = () => img.classList.add('loaded');
    if (img.complete && img.naturalWidth) done();
    else img.addEventListener('load', done, { once: true });
  }

  const strip = $derived.by(() => {
    if (idx < 0) return [];
    const reach = Math.ceil(stageW / (STRIP + STRIP_GAP) / 2) + 2;
    const start = Math.max(0, idx - reach);
    return order.slice(start, idx + reach + 1).map((p, k) => ({ p, offset: start + k - idx }));
  });
</script>

<svelte:window bind:innerWidth={vw} bind:innerHeight={vh} onkeydown={onKey} />

<div class="viewer" class:cursor-hidden={!chrome && scale <= 1.001} role="dialog" aria-modal="true" aria-label="Photo viewer">
  <div class="backdrop" bind:this={backdrop}></div>

  <div
    class="stage"
    style:width="{stageW}px"
    role="presentation"
    onwheel={onWheel}
    onpointerdown={onPointerDown}
    onpointermove={onPointerMove}
    onpointerup={onPointerUp}
    onpointercancel={onPointerUp}
    ondblclick={(e) => toggleZoom(e.clientX, e.clientY)}
    class:grabbing={drag && scale > 1.001}
    class:zoomed={scale > 1.001}
  >
    {#if photo}
      <div
        class="frame"
        class:smooth
        bind:this={frame}
        style:left="{fit.x}px"
        style:top="{fit.y}px"
        style:width="{fit.w}px"
        style:height="{fit.h}px"
        style:transform="translate({tx + swipe}px, {ty}px) scale({scale})"
        ontransitionend={() => (smooth = false)}
      >
        {#key id}
          <img class="layer base" src={api.thumbUrl(photo)} alt="" draggable="false" />
          <img class="layer" src={api.previewUrl(photo)} alt={photo.name} draggable="false" decoding="async" use:fadeIn />
          {#if wantOriginal && (photo.kind === 'jpeg' || photo.kind === 'raf')}
            <img class="layer" src={api.originalUrl(photo)} alt="" draggable="false" decoding="async" use:fadeIn />
          {/if}
        {/key}
      </div>
    {/if}
  </div>

  {#if photo}
    <header
      class="bar"
      class:show={chrome}
      role="toolbar"
      tabindex="-1"
      style:width="{stageW}px"
      onpointerenter={() => (overChrome = true)}
      onpointerleave={() => (overChrome = false)}
    >
      <button class="back" onclick={close}>
        <Icon name="back" size={18} stroke={2} />
        <span>{store.collectionTitle}</span>
      </button>
      <div class="title">
        <strong class="truncate">{photo.name}</strong>
        <span class="muted tabular">
          {photo.takenAt ? formatDateTime(photo.takenAt) : ''}{idx >= 0 ? ` · ${formatCount(idx + 1)} of ${formatCount(order.length)}` : ''}
        </span>
      </div>
      <div class="actions">
        <Stars value={photo.rating} size={17} onrate={(r) => store.rate(r, [photo.id])} />
        <span class="sep"></span>
        <button class="icon-btn" class:pick={photo.flag === 1} title="Pick (P)" onclick={() => store.flag(photo.flag === 1 ? 0 : 1, [photo.id])}>
          <Icon name="flag" size={17} fill={photo.flag === 1} />
        </button>
        <button class="icon-btn" class:reject={photo.flag === -1} title="Reject (X)" onclick={() => store.flag(photo.flag === -1 ? 0 : -1, [photo.id])}>
          <Icon name="x" size={17} />
        </button>
        <span class="sep"></span>
        <button class="icon-btn" title="Zoom (Z)" onclick={() => toggleZoom()}><Icon name="search" size={17} /></button>
        <button class="icon-btn" class:on={store.showInfo} title="Info (I)" onclick={() => (store.showInfo = !store.showInfo)}>
          <Icon name="info" size={17} />
        </button>
        {#if pair}
          <button class="btn compare" title="Compare before and after (C)" onclick={() => store.openCompare(pair)}>
            <Icon name="compare" size={15} /> Before / After
          </button>
        {/if}
        <button class="btn edit" title="Edit in Photoshop (Ctrl+E)" onclick={() => store.edit([photo.id])}>
          <Icon name="edit" size={15} /> Edit
        </button>
      </div>
    </header>

    <div class="zoom-pill tabular" class:show={zoomPill}>{zoomLabel}</div>

    <div
      class="strip"
      class:show={chrome && scale <= 1.001}
      role="listbox"
      tabindex="-1"
      aria-label="Filmstrip"
      style:width="{stageW}px"
      onpointerenter={() => (overChrome = true)}
      onpointerleave={() => (overChrome = false)}
    >
      {#each strip as { p, offset } (p.id)}
        <button
          class="thumb"
          class:current={offset === 0}
          role="option"
          aria-selected={offset === 0}
          aria-label={p.name}
          style:transform="translateX({offset * (STRIP + STRIP_GAP) - STRIP / 2}px)"
          onclick={() => (store.viewerId = store.focusId = p.id)}
        >
          <img src={api.thumbUrl(p)} alt="" draggable="false" />
        </button>
      {/each}
    </div>

    {#if idx > 0}
      <button class="nav prev" class:show={chrome} aria-label="Previous" onclick={() => go(-1)}><Icon name="back" size={22} stroke={2} /></button>
    {/if}
    {#if idx < order.length - 1}
      <button class="nav next" class:show={chrome} aria-label="Next" style:left="{stageW - 60}px" onclick={() => go(1)}><Icon name="next" size={22} stroke={2} /></button>
    {/if}
  {/if}

  {#if store.showInfo && photo}
    <aside class="info"><InfoPanel ids={[photo.id]} dark /></aside>
  {/if}
</div>

<style>
  .viewer {
    position: fixed;
    inset: 0;
    z-index: 50;
    overflow: hidden;
  }
  .viewer.cursor-hidden {
    cursor: none;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    background: #0b0b0c;
  }
  .stage {
    position: absolute;
    inset: 0 auto 0 0;
    touch-action: none;
  }
  .stage.zoomed {
    cursor: grab;
  }
  .stage.grabbing {
    cursor: grabbing;
  }
  /* No will-change here: it would freeze the layer's raster scale, and a
     zoomed-in photo would be an upscaled bitmap instead of sharp pixels. */
  .frame {
    position: absolute;
    transform-origin: 0 0;
  }
  .frame.smooth {
    transition: transform 280ms var(--ease-out);
  }
  .layer {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    user-select: none;
    -webkit-user-drag: none;
  }
  .layer:not(.base) {
    opacity: 0;
    transition: opacity 160ms ease;
  }
  .layer:global(.loaded) {
    opacity: 1;
  }

  .bar {
    position: absolute;
    top: 0;
    left: 0;
    display: grid;
    grid-template-columns: 1fr minmax(0, auto) 1fr;
    align-items: center;
    gap: 16px;
    height: 56px;
    padding: 0 14px 0 8px;
    background: linear-gradient(to bottom, rgba(11, 11, 12, 0.85), rgba(11, 11, 12, 0));
    opacity: 0;
    transform: translateY(-6px);
    transition:
      opacity 200ms,
      transform 260ms var(--ease-out);
    pointer-events: none;
  }
  .bar.show,
  .strip.show,
  .nav.show {
    opacity: 1;
    transform: none;
    pointer-events: auto;
  }
  .back {
    justify-self: start;
    display: flex;
    align-items: center;
    gap: 4px;
    height: 32px;
    padding: 0 10px 0 4px;
    border-radius: 8px;
    color: var(--accent);
    font-weight: 500;
    flex: none;
  }
  .back:hover {
    background: var(--hover);
  }
  .title {
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    line-height: 1.25;
  }
  .title strong {
    font-weight: 600;
    max-width: 100%;
  }
  .title span {
    font-size: 11.5px;
  }
  .actions {
    justify-self: end;
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .sep {
    width: 1px;
    height: 18px;
    margin: 0 6px;
    background: var(--sep);
  }
  .pick {
    color: var(--pick) !important;
  }
  .reject {
    color: var(--danger) !important;
  }
  .edit,
  .compare {
    margin-left: 8px;
    height: 30px;
  }
  .compare + .edit {
    margin-left: 6px;
  }

  .zoom-pill {
    position: absolute;
    left: 50%;
    bottom: 104px;
    padding: 5px 12px;
    border-radius: 20px;
    background: rgba(40, 40, 44, 0.85);
    backdrop-filter: blur(12px);
    font-weight: 600;
    font-size: 12px;
    transform: translateX(-50%) translateY(4px);
    opacity: 0;
    transition:
      opacity 180ms,
      transform 220ms var(--ease-out);
    pointer-events: none;
  }
  .zoom-pill.show {
    opacity: 1;
    transform: translateX(-50%);
  }

  .strip {
    position: absolute;
    left: 0;
    bottom: 18px;
    height: 58px;
    opacity: 0;
    transform: translateY(8px);
    transition:
      opacity 200ms,
      transform 260ms var(--ease-out);
    pointer-events: none;
    -webkit-mask-image: linear-gradient(to right, transparent, black 12%, black 88%, transparent);
    mask-image: linear-gradient(to right, transparent, black 12%, black 88%, transparent);
  }
  .thumb {
    position: absolute;
    left: 50%;
    top: 2px;
    width: 54px;
    height: 54px;
    border-radius: 6px;
    overflow: hidden;
    background: var(--tile);
    opacity: 0.55;
    transition:
      transform 260ms var(--ease-out),
      opacity 160ms;
  }
  .thumb:hover {
    opacity: 0.9;
  }
  .thumb.current {
    opacity: 1;
    box-shadow:
      0 0 0 2px #0b0b0c,
      0 0 0 4px white;
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }

  .nav {
    position: absolute;
    top: 50%;
    left: 16px;
    width: 44px;
    height: 44px;
    margin-top: -22px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    background: rgba(40, 40, 44, 0.6);
    backdrop-filter: blur(12px);
    color: white;
    opacity: 0;
    pointer-events: none;
    transition:
      opacity 200ms,
      background 120ms;
  }
  .nav:hover {
    background: rgba(60, 60, 66, 0.85);
  }

  .info {
    position: absolute;
    top: 0;
    right: 0;
    bottom: 0;
    width: 320px;
    background: #161618;
    border-left: 1px solid var(--sep);
    animation: slide 240ms var(--ease-out);
  }
  @keyframes slide {
    from {
      transform: translateX(24px);
      opacity: 0;
    }
  }
</style>
