import type { Photo, Recipe, RecipeRow } from './types';

const signed = (n: number) => (n > 0 ? `+${n}` : n < 0 ? `−${Math.abs(n)}` : '0');
const signedHalf = (n: number) => signed(Math.abs(n % 1) > 0 ? Number(n.toFixed(1)) : n);

export function wbShiftText(shift: [number, number] | null): string | null {
  if (!shift) return null;
  const [r, b] = shift;
  if (!r && !b) return null;
  return `${signed(r)} R, ${signed(b)} B`;
}

/** Every setting of a recipe as `[label, value]`, in the order Fujifilm lists them. */
export function recipeLines(r: Recipe): [string, string][] {
  const lines: [string, string | null | undefined][] = [
    ['Film simulation', r.filmSim],
    ['Grain', r.grain],
    ['Color Chrome', r.colorChrome],
    ['Color Chrome FX Blue', r.colorChromeBlue],
    ['White balance', r.whiteBalance && [r.whiteBalance, wbShiftText(r.wbShift)].filter(Boolean).join(' · ')],
    ['Dynamic range', r.dynamicRange],
    ['Highlight', r.highlight != null ? signedHalf(r.highlight) : null],
    ['Shadow', r.shadow != null ? signedHalf(r.shadow) : null],
    ['Color', r.color != null ? signed(r.color) : null],
    ['Monochrome toning', r.monoWarmCool != null ? `${signed(r.monoWarmCool)} warm/cool, ${signed(r.monoMagentaGreen ?? 0)} M/G` : null],
    ['Sharpness', r.sharpness != null ? signed(r.sharpness) : null],
    ['Noise reduction', r.noiseReduction != null ? signed(r.noiseReduction) : null],
    ['Clarity', r.clarity != null ? signed(r.clarity) : null],
  ];
  return lines.filter((l): l is [string, string] => !!l[1]);
}

/** Plain text for sharing a recipe (e.g. pasting into a forum post). */
export function recipeText(r: Recipe, name?: string | null): string {
  const lines = recipeLines(r).map(([k, v]) => `${k}: ${v}`);
  return [name ?? r.filmSim ?? 'Fujifilm recipe', ...lines].join('\n');
}

/** A short distinguishing summary, e.g. "H −1 · S +1 · Grain Weak". */
export function recipeSummary(r: Recipe): string {
  const bits = [
    r.highlight != null && r.highlight !== 0 ? `H ${signedHalf(r.highlight)}` : null,
    r.shadow != null && r.shadow !== 0 ? `S ${signedHalf(r.shadow)}` : null,
    r.color != null && r.color !== 0 ? `Color ${signed(r.color)}` : null,
    r.grain && r.grain !== 'Off' ? `Grain ${r.grain.split(',')[0]}` : null,
    wbShiftText(r.wbShift),
    r.dynamicRange && r.dynamicRange !== 'DR100' ? r.dynamicRange.replace('D-Range Priority', 'DR-P') : null,
  ].filter(Boolean);
  return bits.length ? bits.join(' · ') : 'Default settings';
}

export function recipeTitle(row: RecipeRow): string {
  return row.name ?? row.recipe.filmSim ?? 'Recipe';
}

// ---- places ------------------------------------------------------------------

const regionNames = (() => {
  try {
    return new Intl.DisplayNames(undefined, { type: 'region' });
  } catch {
    return null;
  }
})();

export function countryName(code: string | null): string {
  if (!code) return 'Unknown';
  try {
    return regionNames?.of(code) ?? code;
  } catch {
    return code;
  }
}

/** "Kyoto, Japan"; regions are added where towns are commonly ambiguous. */
export function placeName(p: Pick<Photo, 'town' | 'region' | 'country'>): string | null {
  if (!p.country) return null;
  const country = countryName(p.country);
  const region = p.region && ['US', 'CA', 'AU'].includes(p.country) ? p.region : null;
  if (!p.town) return [p.region, country].filter(Boolean).join(', ');
  return [p.town, region, country].filter(Boolean).join(', ');
}
