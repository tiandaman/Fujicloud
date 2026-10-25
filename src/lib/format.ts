import type { Kind, Photo } from './types';

// Capture times are camera-clock wall time, so always format in UTC.
const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric', timeZone: 'UTC' });
const monthFmt = new Intl.DateTimeFormat(undefined, { month: 'long', year: 'numeric', timeZone: 'UTC' });
const dateTimeFmt = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' });
const longDateFmt = new Intl.DateTimeFormat(undefined, { weekday: 'long', month: 'long', day: 'numeric', year: 'numeric', timeZone: 'UTC' });
const timeFmt = new Intl.DateTimeFormat(undefined, { timeStyle: 'short', timeZone: 'UTC' });
const num = new Intl.NumberFormat();

export const formatDay = (s: number) => dayFmt.format(s * 1000);
export const formatMonth = (s: number) => monthFmt.format(s * 1000);
export const formatDateTime = (s: number) => dateTimeFmt.format(s * 1000);
export const formatLongDate = (s: number) => longDateFmt.format(s * 1000);
export const formatTime = (s: number) => timeFmt.format(s * 1000);
export const formatCount = (n: number) => num.format(n);

export function plural(n: number, word: string) {
  return `${formatCount(n)} ${word}${n === 1 ? '' : 's'}`;
}

export function formatShutter(s: number | null): string | null {
  if (!s) return null;
  if (s >= 1) return `${+s.toFixed(1)}″`;
  return `1/${Math.round(1 / s)}`;
}

export function formatAperture(f: number | null): string | null {
  return f ? `ƒ/${+f.toFixed(1)}` : null;
}

export function formatFocal(mm: number | null): string | null {
  return mm ? `${+mm.toFixed(1)} mm` : null;
}

export function formatIso(iso: number | null): string | null {
  return iso ? `ISO ${iso}` : null;
}

export function formatEv(ev: number | null): string | null {
  if (ev == null || Math.abs(ev) < 0.01) return null;
  return `${ev > 0 ? '+' : '−'}${Math.abs(ev).toFixed(1)} EV`;
}

/** How long ago a moment was, roughly: "just now", "5 min ago", "yesterday". */
export function formatAgo(unixSeconds: number, now = Date.now() / 1000): string {
  const s = Math.max(0, now - unixSeconds);
  if (s < 90) return 'just now';
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  const days = Math.floor(s / 86400);
  if (days === 1) return 'yesterday';
  if (days < 30) return `${days} days ago`;
  return `on ${new Date(unixSeconds * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })}`;
}

/** 75 → "1:15", 3725 → "1:02:05". */
export function formatDuration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  const [h, m, sec] = [Math.floor(s / 3600), Math.floor((s % 3600) / 60), s % 60];
  const pad = (n: number) => String(n).padStart(2, '0');
  return h ? `${h}:${pad(m)}:${pad(sec)}` : `${m}:${pad(sec)}`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB', 'TB'];
  let v = bytes / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

export function formatMegapixels(p: Photo): string | null {
  if (!p.width || !p.height) return null;
  return `${p.width} × ${p.height} · ${((p.width * p.height) / 1e6).toFixed(1)} MP`;
}

export function exposureLine(p: Photo): string[] {
  return [formatFocal(p.focalLength), formatAperture(p.aperture), formatShutter(p.shutter), formatIso(p.iso)].filter(
    (x): x is string => !!x,
  );
}

export const kindLabel: Record<Kind, string> = {
  jpeg: 'JPEG',
  raf: 'RAF',
  heif: 'HEIF',
  tiff: 'TIFF',
  psd: 'PSD',
  png: 'PNG',
  dng: 'DNG',
  video: 'Video',
};
