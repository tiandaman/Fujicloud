import type { LayoutMode, Photo, Section } from './types';

/** A horizontal band of the scroll area: a section header or a row of tiles. */
export interface Row {
  y: number;
  h: number;
  /** Set for header rows. */
  section?: Section;
  /** Tiles in this row, as a `[start, end)` range of the flat order. */
  start: number;
  end: number;
}

export interface Layout {
  width: number;
  height: number;
  rows: Row[];
  /** `x, y, w, h` for every tile, in flat order. */
  boxes: Float64Array;
  flat: Photo[];
  /** Photo id → flat index. */
  index: Map<number, number>;
  /** Flat index → row index, for keyboard navigation. */
  rowOf: Int32Array;
  headers: Row[];
}

export const GAP = 3;
export const HEADER_H = 58;
export const PAD_X = 20;
export const PAD_TOP = 8;
export const PAD_BOTTOM = 40;

function aspect(p: Photo): number {
  const a = p.width && p.height ? p.width / p.height : 1.5;
  // Keep panoramas and slivers from wrecking a row.
  return Math.min(3.2, Math.max(0.45, a));
}

export function computeLayout(sections: Section[], viewportWidth: number, mode: LayoutMode, size: number, showHeaders: boolean): Layout {
  const width = Math.max(200, viewportWidth - PAD_X * 2);
  const total = sections.reduce((n, s) => n + s.items.length, 0);
  const boxes = new Float64Array(total * 4);
  const flat: Photo[] = new Array(total);
  const index = new Map<number, number>();
  const rowOf = new Int32Array(total);
  const rows: Row[] = [];
  const headers: Row[] = [];
  let y = PAD_TOP;
  let i = 0;

  const place = (p: Photo, x: number, rowY: number, w: number, h: number) => {
    flat[i] = p;
    index.set(p.id, i);
    rowOf[i] = rows.length;
    boxes[i * 4] = PAD_X + x;
    boxes[i * 4 + 1] = rowY;
    boxes[i * 4 + 2] = w;
    boxes[i * 4 + 3] = h;
    i++;
  };

  for (const section of sections) {
    if (!section.items.length) continue;
    if (showHeaders) {
      const header: Row = { y, h: HEADER_H, section, start: i, end: i };
      rows.push(header);
      headers.push(header);
      y += HEADER_H;
    }

    if (mode === 'grid') {
      const cols = Math.max(1, Math.round((width + GAP) / (size + GAP)));
      const tile = (width - GAP * (cols - 1)) / cols;
      for (let s = 0; s < section.items.length; s += cols) {
        const start = i;
        const slice = section.items.slice(s, s + cols);
        slice.forEach((p, c) => place(p, c * (tile + GAP), y, tile, tile));
        rows.push({ y, h: tile, start, end: i });
        y += tile + GAP;
      }
    } else {
      const items = section.items;
      let s = 0;
      while (s < items.length) {
        // Grow the row until it is wider than the viewport at the target
        // height, then keep whichever of "with" / "without" the last photo
        // lands closer to the target height.
        let sum = 0;
        let e = s;
        while (e < items.length) {
          const next = sum + aspect(items[e]);
          if (next * size + GAP * (e - s) >= width) {
            const hWith = (width - GAP * (e - s)) / next;
            const hWithout = e > s ? (width - GAP * (e - s - 1)) / sum : Infinity;
            if (Math.abs(Math.log(hWith / size)) <= Math.abs(Math.log(hWithout / size))) {
              sum = next;
              e++;
            }
            break;
          }
          sum = next;
          e++;
        }
        const last = e >= items.length;
        const fitted = (width - GAP * (e - s - 1)) / sum;
        // The final row of a section keeps the target height, left-aligned.
        const h = last ? Math.min(size, fitted) : fitted;
        const start = i;
        let x = 0;
        for (let k = s; k < e; k++) {
          const w = aspect(items[k]) * h;
          place(items[k], x, y, w, h);
          x += w + GAP;
        }
        rows.push({ y, h, start, end: i });
        y += h + GAP;
        s = e;
      }
    }
    y += showHeaders ? 10 : 0;
  }

  return { width: viewportWidth, height: y + PAD_BOTTOM, rows, boxes, flat, index, rowOf, headers };
}

/** Index of the first row that ends below `top`. */
export function firstRowAfter(rows: Row[], top: number): number {
  let lo = 0;
  let hi = rows.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (rows[mid].y + rows[mid].h < top) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

/** The header whose section is under `top`, for the sticky title. */
export function headerAt(headers: Row[], top: number): Row | null {
  let lo = 0;
  let hi = headers.length - 1;
  let found: Row | null = null;
  while (lo <= hi) {
    const mid = (lo + hi) >> 1;
    if (headers[mid].y <= top) {
      found = headers[mid];
      lo = mid + 1;
    } else hi = mid - 1;
  }
  return found;
}

/** Flat index of the tile in the row above/below closest to `from` horizontally. */
export function verticalNeighbor(layout: Layout, from: number, dir: 1 | -1): number {
  const cx = layout.boxes[from * 4] + layout.boxes[from * 4 + 2] / 2;
  let r = layout.rowOf[from] + dir;
  while (r >= 0 && r < layout.rows.length && layout.rows[r].start === layout.rows[r].end) r += dir;
  if (r < 0 || r >= layout.rows.length) return from;
  const row = layout.rows[r];
  let best = row.start;
  let bestD = Infinity;
  for (let k = row.start; k < row.end; k++) {
    const d = Math.abs(layout.boxes[k * 4] + layout.boxes[k * 4 + 2] / 2 - cx);
    if (d < bestD) {
      bestD = d;
      best = k;
    }
  }
  return best;
}
