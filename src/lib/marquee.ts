/**
 * Drag a box to select: press on the page (not on a control), move a few
 * pixels, and everything the box touches is selected; Ctrl or Shift adds to
 * what was selected already. Near the top or bottom edge the page scrolls.
 * A press that doesn't move stays an ordinary click.
 *
 *   <div class="scroller" use:marquee={{ canvas, hits, select, from }}>
 */

export type Box = { left: number; top: number; right: number; bottom: number };

export type MarqueeOptions = {
  /** The scrolled content; the box is drawn in it, and `hits` gets boxes in its coordinates. */
  canvas: () => HTMLElement | undefined;
  /** What the box touches. */
  hits: (box: Box) => number[];
  /** What's selected now, to add to with Ctrl or Shift. */
  selected: () => Iterable<number>;
  select: (ids: Set<number>) => void;
  /** Whether a press here may start a box (e.g. not on a selected photo, which drags it out). */
  from?: (e: PointerEvent) => boolean;
};

const START = 6;
const EDGE = 48;

export function marquee(scroller: HTMLElement, options: MarqueeOptions) {
  let opts = options;
  let start: { x: number; y: number; id: number; additive: boolean; base: Set<number> } | null = null;
  let active = false;
  let last = { x: 0, y: 0 };
  let frame = 0;
  let box: HTMLDivElement | null = null;

  const local = (x: number, y: number) => {
    const r = opts.canvas()?.getBoundingClientRect();
    return r ? { x: x - r.left, y: y - r.top } : { x, y };
  };

  function down(e: PointerEvent) {
    if (e.button !== 0 || e.pointerType === 'touch') return;
    const t = e.target as HTMLElement;
    if (t.closest('input, textarea, select, a, [data-no-marquee]')) return;
    if (opts.from && !opts.from(e)) return;
    const p = local(e.clientX, e.clientY);
    const additive = e.ctrlKey || e.metaKey || e.shiftKey;
    start = { ...p, id: e.pointerId, additive, base: new Set(additive ? opts.selected() : []) };
    last = { x: e.clientX, y: e.clientY };
  }

  function move(e: PointerEvent) {
    if (!start || e.pointerId !== start.id) return;
    last = { x: e.clientX, y: e.clientY };
    if (!active) {
      const p = local(e.clientX, e.clientY);
      if (Math.hypot(p.x - start.x, p.y - start.y) < START) return;
      active = true;
      scroller.setPointerCapture(e.pointerId);
      scroller.classList.add('marqueeing');
      getSelection()?.removeAllRanges();
      box = document.createElement('div');
      box.className = 'marquee-box';
      opts.canvas()?.appendChild(box);
    }
    if (!frame) frame = requestAnimationFrame(tick);
  }

  /** Redraws the box and the selection, and scrolls near the edges. */
  function tick() {
    frame = 0;
    if (!start || !active) return;
    const view = scroller.getBoundingClientRect();
    const speed =
      last.y < view.top + EDGE ? -Math.ceil((view.top + EDGE - last.y) / 3) : last.y > view.bottom - EDGE ? Math.ceil((last.y - view.bottom + EDGE) / 3) : 0;
    if (speed) scroller.scrollTop += Math.max(-40, Math.min(40, speed));
    const p = local(last.x, last.y);
    const b: Box = {
      left: Math.min(start.x, p.x),
      top: Math.min(start.y, p.y),
      right: Math.max(start.x, p.x),
      bottom: Math.max(start.y, p.y),
    };
    if (box) {
      box.style.transform = `translate(${b.left}px, ${b.top}px)`;
      box.style.width = `${b.right - b.left}px`;
      box.style.height = `${b.bottom - b.top}px`;
    }
    const next = new Set(start.base);
    for (const id of opts.hits(b)) next.add(id);
    opts.select(next);
    // Keep scrolling while held at the edge, even without moving.
    if (speed) frame = requestAnimationFrame(tick);
  }

  function up(e: PointerEvent) {
    if (!start || e.pointerId !== start.id) return;
    if (active) {
      // The press ended a box, not a click on whatever is under it.
      const swallow = (c: MouseEvent) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener('click', swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener('click', swallow, { capture: true }), 0);
      if (scroller.hasPointerCapture(e.pointerId)) scroller.releasePointerCapture(e.pointerId);
    }
    end();
  }

  function end() {
    if (frame) cancelAnimationFrame(frame);
    frame = 0;
    box?.remove();
    box = null;
    scroller.classList.remove('marqueeing');
    start = null;
    active = false;
  }

  scroller.addEventListener('pointerdown', down);
  scroller.addEventListener('pointermove', move);
  window.addEventListener('pointermove', move);
  window.addEventListener('pointerup', up);
  window.addEventListener('pointercancel', end);
  return {
    update(next: MarqueeOptions) {
      opts = next;
    },
    destroy() {
      end();
      scroller.removeEventListener('pointerdown', down);
      scroller.removeEventListener('pointermove', move);
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', up);
      window.removeEventListener('pointercancel', end);
    },
  };
}

/** Whether two boxes overlap. */
export function touches(a: Box, left: number, top: number, width: number, height: number): boolean {
  return left < a.right && left + width > a.left && top < a.bottom && top + height > a.top;
}
