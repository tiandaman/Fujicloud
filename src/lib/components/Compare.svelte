<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../api';
  import { kindLabel } from '../format';
  import { store } from '../store.svelte';
  import type { Photo } from '../types';
  import Icon from './Icon.svelte';

  const PAD = 18;
  const BAR = 56;
  const GAP = 6;

  const before = $derived(store.compare ? store.byId.get(store.compare.before) : undefined);
  const after = $derived(store.compare ? store.byId.get(store.compare.after) : undefined);
  const mode = $derived(store.compareMode);

  let vw = $state(innerWidth);
  let vh = $state(innerHeight);
  let root: HTMLDivElement;

  /** One pane per photo side by side, or one shared pane for the slider. */
  const pane = $derived({
    w: mode === 'split' ? (vw - PAD * 2 - GAP) / 2 : vw - PAD * 2,
    h: vh - BAR - PAD,
  });

  // ---- shared zoom & pan: both photos always show the same spot -------------
  // `tx, ty` move the photo's centre away from the pane's centre.

  let scale = $state(1);
  let tx = $state(0);
  let ty = $state(0);
  let smooth = $state(false);
  let split = $state(0.5);

  function fit(p: Photo | undefined) {
    const a = p && p.width && p.height ? p.width / p.height : 1.5;
    let w = pane.w;
    let h = w / a;
    if (h > pane.h) {
      h = pane.h;
      w = h * a;
    }
    return { w, h };
  }

  const fitBefore = $derived(fit(before));
  const fitAfter = $derived(fit(after));
  const oneToOne = $derived(before ? Math.max(1, before.width / (fitBefore.w * devicePixelRatio)) : 1);
  const wantFull = $derived(scale * Math.max(fitBefore.w, fitBefore.h) * devicePixelRatio > 2600);

  function clamp(s: number, x: number, y: number): [number, number] {
    const f = fitBefore;
    const maxX = Math.max(0, (f.w * s - pane.w) / 2);
    const maxY = Math.max(0, (f.h * s - pane.h) / 2);
    return [Math.max(-maxX, Math.min(maxX, x)), Math.max(-maxY, Math.min(maxY, y))];
  }

  /** Zoom keeping the point under (px, py), relative to the pane centre, still. */
  function zoomTo(s: number, px = 0, py = 0, animate = false) {
    s = Math.min(Math.max(4, oneToOne * 2), Math.max(1, s));
    const ux = (px - tx) / scale;
    const uy = (py - ty) / scale;
    [tx, ty] = clamp(s, px - s * ux, py - s * uy);
    scale = s;
    smooth = animate;
  }

  function paneOffset(e: MouseEvent, el: HTMLElement) {
    const r = el.getBoundingClientRect();
    return [e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2] as const;
  }

  function onWheel(e: WheelEvent) {
    e.preventDefault();
    const [px, py] = paneOffset(e, e.currentTarget as HTMLElement);
    zoomTo(scale * Math.exp(-e.deltaY * (e.ctrlKey ? 0.012 : 0.0022)), px, py);
  }

  function toggleZoom(e?: MouseEvent) {
    if (scale > 1.01) {
      smooth = true;
      scale = 1;
      tx = ty = 0;
    } else {
      const [px, py] = e ? paneOffset(e, e.currentTarget as HTMLElement) : [0, 0];
      zoomTo(Math.max(oneToOne, 2), px, py, true);
    }
  }

  // Dragging pans when zoomed; in slider mode it moves the divider otherwise
  // (or always, when grabbing the handle).
  let drag: { x: number; y: number; tx: number; ty: number; divider: boolean; left: number; width: number } | null = null;

  function onPointerDown(e: PointerEvent, divider = false) {
    if (e.button !== 0) return;
    const el = e.currentTarget as HTMLElement;
    const box = (divider ? el.parentElement! : el).getBoundingClientRect();
    el.setPointerCapture(e.pointerId);
    drag = { x: e.clientX, y: e.clientY, tx, ty, divider: divider || (mode === 'slider' && scale <= 1.001), left: box.left, width: box.width };
    smooth = false;
    if (drag.divider) moveDivider(e.clientX);
    e.stopPropagation();
  }

  function moveDivider(x: number) {
    if (!drag) return;
    split = Math.min(1, Math.max(0, (x - drag.left) / drag.width));
  }

  function onPointerMove(e: PointerEvent) {
    if (!drag) return;
    if (drag.divider) moveDivider(e.clientX);
    else if (scale > 1.001) [tx, ty] = clamp(scale, drag.tx + e.clientX - drag.x, drag.ty + e.clientY - drag.y);
  }

  function swap() {
    if (store.compare) store.compare = { before: store.compare.after, after: store.compare.before };
  }

  function close() {
    store.compare = null;
  }

  function onKey(e: KeyboardEvent) {
    const handled = () => {
      e.preventDefault();
      e.stopImmediatePropagation();
    };
    switch (e.key) {
      case 'Escape':
        handled();
        close();
        break;
      case 's':
      case 'S':
        handled();
        swap();
        break;
      case 'v':
      case 'V':
      case 'Tab':
        handled();
        store.compareMode = mode === 'split' ? 'slider' : 'split';
        break;
      case 'z':
      case 'Z':
        handled();
        toggleZoom();
        break;
      case '=':
      case '+':
        handled();
        zoomTo(scale * 1.5, 0, 0, true);
        break;
      case '-':
        handled();
        zoomTo(scale / 1.5, 0, 0, true);
        break;
      case 'ArrowLeft':
      case 'ArrowRight':
        if (mode === 'slider') {
          handled();
          split = Math.min(1, Math.max(0, split + (e.key === 'ArrowLeft' ? -0.05 : 0.05)));
        }
        break;
    }
  }

  onMount(() => {
    root.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 180, easing: 'ease-out' });
  });

  function describe(p: Photo | undefined) {
    if (!p) return '';
    if (p.editOf == null) {
      const film = p.filmSim ? ` · ${p.filmSim}` : '';
      return p.kind === 'raf' ? `Camera RAW${film}` : `Straight from the camera${film}`;
    }
    return p.kind === 'psd' ? 'Photoshop document' : p.kind === 'tiff' ? 'TIFF edit' : `Edited ${kindLabel[p.kind]}`;
  }

  function fadeIn(img: HTMLImageElement) {
    const done = () => img.classList.add('loaded');
    if (img.complete && img.naturalWidth) done();
    else img.addEventListener('load', done, { once: true });
  }

  const zoomLabel = $derived(scale <= 1.001 ? 'Fit' : `${Math.round((scale / oneToOne) * 100)}%`);
</script>

<svelte:window bind:innerWidth={vw} bind:innerHeight={vh} onkeydown={onKey} />

{#snippet frame(p: Photo, size: { w: number; h: number }, clip: string | null)}
  <div
    class="frame"
    class:smooth
    style:width="{size.w}px"
    style:height="{size.h}px"
    style:transform="translate(calc(-50% + {tx}px), calc(-50% + {ty}px)) scale({scale})"
    style:clip-path={clip}
    ontransitionend={() => (smooth = false)}
  >
    {#key p.id}
      <img class="layer base" src={api.thumbUrl(p)} alt="" draggable="false" />
      <img class="layer" src={api.previewUrl(p)} alt={p.name} draggable="false" decoding="async" use:fadeIn />
      {#if wantFull && (p.kind === 'jpeg' || p.kind === 'raf')}
        <img class="layer" src={api.originalUrl(p)} alt="" draggable="false" decoding="async" use:fadeIn />
      {/if}
    {/key}
  </div>
{/snippet}

{#snippet label(text: string, p: Photo | undefined, side: 'left' | 'right')}
  <div class="label {side}">
    <strong>{text}</strong>
    <span class="truncate">{p?.name} · {describe(p)}</span>
  </div>
{/snippet}

<div class="compare" bind:this={root} role="dialog" aria-modal="true" aria-label="Compare before and after">
  <header class="bar">
    <button class="back" onclick={close}><Icon name="back" size={18} stroke={2} /><span>Done</span></button>

    <div class="seg" role="radiogroup" aria-label="Compare mode">
      <button role="radio" aria-checked={mode === 'split'} class:on={mode === 'split'} onclick={() => (store.compareMode = 'split')}>
        <Icon name="compare" size={15} /> Side by side
      </button>
      <button role="radio" aria-checked={mode === 'slider'} class:on={mode === 'slider'} onclick={() => (store.compareMode = 'slider')}>
        <Icon name="expand" size={15} /> Slider
      </button>
    </div>

    <div class="tools">
      <span class="zoom tabular" title="Double-click or Z to zoom">{zoomLabel}</span>
      <button class="icon-btn" title="Swap sides (S)" onclick={swap}><Icon name="swap" size={16} /></button>
      {#if after}
        <button class="btn" title="Open the edit in Photoshop" onclick={() => store.edit([after.id])}><Icon name="edit" size={15} /> Edit</button>
      {/if}
    </div>
  </header>

  {#if before && after}
    {#if mode === 'split'}
      <div class="panes" style:top="{BAR}px">
        {#each [before, after] as p, i (i)}
          <div
            class="pane"
            class:zoomed={scale > 1.001}
            style:width="{pane.w}px"
            style:height="{pane.h}px"
            role="presentation"
            onwheel={onWheel}
            ondblclick={toggleZoom}
            onpointerdown={(e) => onPointerDown(e)}
            onpointermove={onPointerMove}
            onpointerup={() => (drag = null)}
          >
            {@render frame(p, i === 0 ? fitBefore : fitAfter, null)}
            {@render label(i === 0 ? 'Before' : 'After', p, i === 0 ? 'left' : 'right')}
          </div>
        {/each}
      </div>
    {:else}
      <div class="panes" style:top="{BAR}px">
        <div
          class="pane slider"
          class:zoomed={scale > 1.001}
          style:width="{pane.w}px"
          style:height="{pane.h}px"
          role="presentation"
          onwheel={onWheel}
          ondblclick={toggleZoom}
          onpointerdown={(e) => onPointerDown(e)}
          onpointermove={onPointerMove}
          onpointerup={() => (drag = null)}
        >
          {@render frame(before, fitBefore, null)}
          <!-- The edit shows to the right of the divider. Clip in pane space: -->
          <div class="clip" style:clip-path="inset(0 0 0 {split * 100}%)">
            {@render frame(after, fitAfter, null)}
          </div>
          <div
            class="divider"
            style:left="{split * 100}%"
            role="slider"
            tabindex="0"
            aria-label="Before/after divider"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(split * 100)}
            onpointerdown={(e) => onPointerDown(e, true)}
            onpointermove={onPointerMove}
            onpointerup={() => (drag = null)}
          >
            <span class="knob"><Icon name="compare" size={16} /></span>
          </div>
          {@render label('Before', before, 'left')}
          {@render label('After', after, 'right')}
        </div>
      </div>
    {/if}
  {/if}

  <p class="hint faint">
    Scroll to zoom · drag to pan · <span class="kbd">V</span> switch view · <span class="kbd">S</span> swap · <span class="kbd">Esc</span> done
  </p>
</div>

<style>
  .compare {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: #0b0b0c;
  }
  .bar {
    position: absolute;
    inset: 0 0 auto 0;
    height: 56px;
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
    padding: 0 14px 0 8px;
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
  }
  .back:hover {
    background: var(--hover);
  }
  .seg {
    display: flex;
    padding: 2px;
    border-radius: 9px;
    background: var(--bg-input);
    box-shadow: inset 0 0 0 0.5px var(--border);
  }
  .seg button {
    display: flex;
    align-items: center;
    gap: 6px;
    height: 28px;
    padding: 0 12px;
    border-radius: 7px;
    color: var(--text-2);
    font-weight: 500;
  }
  .seg button.on {
    background: rgba(255, 255, 255, 0.14);
    color: var(--text);
    box-shadow: 0 1px 2px rgba(0, 0, 0, 0.3);
  }
  .tools {
    justify-self: end;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .zoom {
    min-width: 46px;
    text-align: center;
    color: var(--text-2);
    font-size: 12px;
    font-weight: 600;
  }
  .tools .btn {
    height: 30px;
  }

  .panes {
    position: absolute;
    left: 18px;
    right: 18px;
    bottom: 18px;
    display: flex;
    gap: 6px;
  }
  .pane {
    position: relative;
    overflow: hidden;
    border-radius: 10px;
    background: #111113;
    touch-action: none;
  }
  .pane.zoomed {
    cursor: grab;
  }
  .pane.slider {
    cursor: ew-resize;
  }
  .pane.slider.zoomed {
    cursor: grab;
  }
  .frame {
    position: absolute;
    top: 50%;
    left: 50%;
    transform-origin: center;
  }
  .frame.smooth {
    transition: transform 260ms var(--ease-out);
  }
  .clip {
    position: absolute;
    inset: 0;
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

  .divider {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 2px;
    margin-left: -1px;
    background: rgba(255, 255, 255, 0.9);
    box-shadow: 0 0 8px rgba(0, 0, 0, 0.6);
    cursor: ew-resize;
    z-index: 2;
  }
  .divider::before {
    /* A wider invisible grab area. */
    content: '';
    position: absolute;
    inset: 0 -12px;
  }
  .knob {
    position: absolute;
    top: 50%;
    left: 50%;
    display: grid;
    place-items: center;
    width: 38px;
    height: 38px;
    border-radius: 50%;
    transform: translate(-50%, -50%);
    background: rgba(255, 255, 255, 0.95);
    color: #111;
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.5);
  }

  .label {
    position: absolute;
    top: 12px;
    display: flex;
    flex-direction: column;
    max-width: 45%;
    padding: 6px 11px;
    border-radius: 9px;
    background: rgba(20, 20, 22, 0.72);
    backdrop-filter: blur(12px);
    line-height: 1.3;
    pointer-events: none;
    z-index: 3;
  }
  .label.left {
    left: 12px;
  }
  .label.right {
    right: 12px;
    text-align: right;
  }
  .label strong {
    font-size: 12px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }
  .label span {
    font-size: 11.5px;
    color: var(--text-2);
  }
  .hint {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 2px;
    margin: 0;
    text-align: center;
    font-size: 11px;
    pointer-events: none;
  }
</style>
