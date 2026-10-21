/**
 * A colour identity per film simulation, used for sidebar swatches and info
 * chips. Loosely after each stock's character: Velvia saturated, Classic
 * Chrome muted teal, Acros neutral, and so on.
 */
const swatches: Record<string, [string, string]> = {
  Provia: ['#4f8cff', '#7fd1ff'],
  Velvia: ['#ff3d7f', '#ffb000'],
  Astia: ['#ff9a8b', '#ffd3a5'],
  'Classic Chrome': ['#3f6f73', '#c7a46a'],
  'Classic Neg.': ['#2e5e4e', '#e0a96d'],
  'Nostalgic Neg.': ['#c07a3d', '#f2d0a4'],
  'Reala Ace': ['#3aa17e', '#ffd166'],
  'Pro Neg. Hi': ['#8a7cff', '#ffc4d6'],
  'Pro Neg. Std': ['#9a8fbf', '#e8dcf0'],
  Eterna: ['#3b5a6f', '#9fb4a5'],
  'Eterna Bleach Bypass': ['#55606a', '#c9c3b5'],
  'Studio Portrait': ['#d98a6a', '#f6d5c3'],
  Sepia: ['#7a5230', '#e2c095'],
};

export function filmSwatch(name: string | null): [string, string] {
  if (!name) return ['#5a5a60', '#8a8a92'];
  if (name.startsWith('Acros')) return ['#1d1d1f', '#d9d9de'];
  if (name.startsWith('Monochrome')) return ['#3a3a3e', '#bdbdc2'];
  return swatches[name] ?? ['#5a5a60', '#8a8a92'];
}

export function filmGradient(name: string | null): string {
  const [a, b] = filmSwatch(name);
  return `linear-gradient(135deg, ${a}, ${b})`;
}

export const labelColors: Record<string, string> = {
  Red: '#ff453a',
  Yellow: '#ffd60a',
  Green: '#30d158',
  Blue: '#0a84ff',
  Purple: '#bf5af2',
};
