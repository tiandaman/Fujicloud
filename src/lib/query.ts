// One search box for everything: "Mochi at the beach in 2024, 4 stars or
// more" is a pet, a year, a rating, and a description of the picture.
//
// Words that name something the library knows (a person or pet, a place, an
// album, a year or month, stars, picks, a kind of file) become filters; what's
// left is the description, matched against names and, with smart search on,
// against what's in the picture. Anything in "quotes" is always description.

import type { Photo } from './types';

/** What the library knows, for recognising words. */
export interface Known {
  /** Named people and pets, with the photos each is in. */
  people: { name: string; photoIds: number[] }[];
  albums: { name: string; ids: number[] }[];
  /** Places photos were taken: each name as shown, and the photo field it is. */
  places: { name: string; field: 'town' | 'region' | 'country'; value: string }[];
  /** Today, as Unix seconds (for "this year", "last month"…). */
  now: number;
}

export interface Parsed {
  /** Every one must hold. */
  filters: { label: string; test: (p: Photo) => boolean }[];
  /** The description left over, for matching names and what's in the picture. */
  text: string;
}

const MONTHS = ['january', 'february', 'march', 'april', 'may', 'june', 'july', 'august', 'september', 'october', 'november', 'december'];
const SHORT_MONTHS = MONTHS.map((m) => m.slice(0, 3));
const MONTH_TITLES = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
/** Words that only join the others: dropped when they're left at the edges. */
const JOINING = new Set(['at', 'in', 'on', 'from', 'with', 'and', 'of', 'the', 'a', 'an', 'by', 'taken', 'photos', 'photo', 'pictures', 'picture', 'shots', 'or', 'more', 'during', 'rated']);
const KINDS: Record<string, { label: string; test: (p: Photo) => boolean }> = {
  raw: { label: 'RAW', test: (p) => p.kind === 'raf' || p.kind === 'dng' || (p.pairId != null && p.kind !== 'video') },
  raws: { label: 'RAW', test: (p) => p.kind === 'raf' || p.kind === 'dng' || (p.pairId != null && p.kind !== 'video') },
  video: { label: 'Videos', test: (p) => p.kind === 'video' },
  videos: { label: 'Videos', test: (p) => p.kind === 'video' },
  movie: { label: 'Videos', test: (p) => p.kind === 'video' },
  movies: { label: 'Videos', test: (p) => p.kind === 'video' },
  screenshot: { label: 'Screenshots', test: (p) => p.kind === 'png' },
  screenshots: { label: 'Screenshots', test: (p) => p.kind === 'png' },
  edits: { label: 'Edits', test: (p) => p.editOf != null || p.kind === 'psd' || p.kind === 'tiff' },
  edited: { label: 'Edits', test: (p) => p.editOf != null || p.kind === 'psd' || p.kind === 'tiff' },
};
const FLAGS: Record<string, { label: string; test: (p: Photo) => boolean }> = {
  picks: { label: 'Picks', test: (p) => p.flag === 1 },
  picked: { label: 'Picks', test: (p) => p.flag === 1 },
  rejected: { label: 'Rejected', test: (p) => p.flag === -1 },
  rejects: { label: 'Rejected', test: (p) => p.flag === -1 },
  favourites: { label: 'Favourites', test: (p) => p.favorite },
  favorites: { label: 'Favourites', test: (p) => p.favorite },
  favourite: { label: 'Favourites', test: (p) => p.favorite },
  favorite: { label: 'Favourites', test: (p) => p.favorite },
  unrated: { label: 'Unrated', test: (p) => p.rating === 0 },
};
const LABELS = ['red', 'yellow', 'green', 'blue', 'purple'];

/** Camera-clock time (naive seconds) as its UTC parts. */
const when = (p: Photo) => (p.takenAt == null ? null : new Date(p.takenAt * 1000));
const fold = (s: string) =>
  s
    .toLowerCase()
    .normalize('NFD')
    .replace(/[̀-ͯ]/g, '');

export function parseQuery(query: string, known: Known): Parsed {
  const filters: Parsed['filters'] = [];
  // Quoted stretches are kept out of it.
  const quoted: string[] = [];
  const bare = query.replace(/"([^"]*)"|“([^”]*)”/g, (_, a, b) => {
    quoted.push((a ?? b ?? '').trim());
    return ' \u0000 ';
  });
  // Stars first, while the punctuation is there: "4+ stars", "★★★", "3 stars or more".
  let rest = bare.replace(/(?:(\d)\s*(\+)?\s*(?:stars?|\*|★)(\s+(?:or|and)\s+(?:more|up|above|better))?|(★{1,5})(\+)?)/gi, (m, n, plus, more, glyphs, plus2) => {
    const stars = glyphs ? glyphs.length : Number(n);
    if (!(stars >= 1 && stars <= 5)) return m;
    const least = !!(plus || more || plus2);
    filters.push({ label: `${'★'.repeat(stars)}${least && stars < 5 ? '+' : ''}`, test: least ? (p) => p.rating >= stars : (p) => p.rating === stars });
    return ' ';
  });
  rest = rest.replace(/[,;]+/g, ' ');
  const words = rest.split(/\s+/).filter(Boolean);
  const used = words.map(() => false);
  const at = (i: number) => (used[i] ? '' : fold(words[i] ?? ''));

  // Names of things, longest first so "New York" beats "York".
  const named: { words: string[]; label: string; test: (p: Photo) => boolean }[] = [];
  for (const who of known.people) {
    const ids = new Set(who.photoIds);
    named.push({ words: fold(who.name).split(/\s+/), label: who.name, test: (p) => ids.has(p.id) || (p.pairId != null && ids.has(p.pairId)) });
  }
  for (const place of known.places) {
    named.push({ words: fold(place.name).split(/\s+/), label: place.name, test: (p) => p[place.field] === place.value });
  }
  for (const album of known.albums) {
    const ids = new Set(album.ids);
    named.push({ words: fold(album.name).split(/\s+/), label: `Album: ${album.name}`, test: (p) => ids.has(p.id) });
  }
  named.sort((a, b) => b.words.length - a.words.length);
  const seen = new Set<string>();
  for (const n of named) {
    if (!n.words[0] || seen.has(n.label)) continue;
    for (let i = 0; i + n.words.length <= words.length; i++) {
      if (n.words.every((w, k) => at(i + k) === w)) {
        n.words.forEach((_, k) => (used[i + k] = true));
        filters.push({ label: n.label, test: n.test });
        seen.add(n.label);
        break;
      }
    }
  }

  const today = new Date(known.now * 1000);
  const year = today.getFullYear();
  for (let i = 0; i < words.length; i++) {
    const w = at(i);
    if (!w) continue;
    const next = at(i + 1);
    const take = (n: number, label: string, test: (p: Photo) => boolean) => {
      for (let k = 0; k < n; k++) used[i + k] = true;
      filters.push({ label, test });
    };
    const month = MONTHS.indexOf(w) >= 0 ? MONTHS.indexOf(w) : w.length === 3 || w === 'sept' ? SHORT_MONTHS.indexOf(w.slice(0, 3)) : -1;
    // "May" alone is more often a word than a month.
    if (month >= 0 && !(w === 'may' && !/^(19|20)\d\d$/.test(next))) {
      if (/^(19|20)\d\d$/.test(next)) {
        const y = Number(next);
        take(2, `${MONTH_TITLES[month]} ${y}`, (p) => when(p)?.getUTCMonth() === month && when(p)?.getUTCFullYear() === y);
      } else {
        take(1, MONTH_TITLES[month], (p) => when(p)?.getUTCMonth() === month);
      }
    } else if (/^(19|20)\d\d$/.test(w)) {
      const y = Number(w);
      take(1, w, (p) => when(p)?.getUTCFullYear() === y);
    } else if (/^(19|20)\d\d[-–](19|20)\d\d$/.test(w)) {
      const [a, b] = w.split(/[-–]/).map(Number);
      take(1, `${a}–${b}`, (p) => {
        const y = when(p)?.getUTCFullYear();
        return y != null && y >= a && y <= b;
      });
    } else if ((w === 'this' || w === 'last') && (next === 'year' || next === 'month')) {
      const back = w === 'last' ? 1 : 0;
      if (next === 'year') {
        take(2, `${year - back}`, (p) => when(p)?.getUTCFullYear() === year - back);
      } else {
        const d = new Date(year, today.getMonth() - back, 1);
        take(2, `${MONTH_TITLES[d.getMonth()]} ${d.getFullYear()}`, (p) => when(p)?.getUTCMonth() === d.getMonth() && when(p)?.getUTCFullYear() === d.getFullYear());
      }
    } else if (w === 'today' || w === 'yesterday') {
      const d = new Date(year, today.getMonth(), today.getDate() - (w === 'yesterday' ? 1 : 0));
      take(1, w === 'today' ? 'Today' : 'Yesterday', (p) => {
        const t = when(p);
        return !!t && t.getUTCFullYear() === d.getFullYear() && t.getUTCMonth() === d.getMonth() && t.getUTCDate() === d.getDate();
      });
    } else if (FLAGS[w]) {
      take(1, FLAGS[w].label, FLAGS[w].test);
    } else if (KINDS[w]) {
      take(1, KINDS[w].label, KINDS[w].test);
    } else if (LABELS.includes(w) && (next === 'label' || next === 'labels' || next === 'labelled' || next === 'labeled')) {
      const label = w[0].toUpperCase() + w.slice(1);
      take(2, `${label} label`, (p) => p.label === label);
    }
  }

  // What's left is the description. Joining words go from its edges, and
  // from around each filter that was lifted out ("at the beach [in] 2024").
  const kept: string[] = [];
  let quote = 0;
  let afterGap = true;
  for (let i = 0; i < words.length; i++) {
    if (used[i]) {
      while (kept.length && JOINING.has(fold(kept[kept.length - 1])) && !kept[kept.length - 1].startsWith('"')) kept.pop();
      afterGap = true;
      continue;
    }
    if (words[i] === '\u0000') {
      kept.push(`"${quoted[quote++] ?? ''}"`);
      afterGap = false;
      continue;
    }
    if (afterGap && JOINING.has(fold(words[i]))) continue;
    afterGap = false;
    kept.push(words[i]);
  }
  while (kept.length && JOINING.has(fold(kept[kept.length - 1])) && !kept[kept.length - 1].startsWith('"')) kept.pop();
  const text = kept
    .map((w) => (w.startsWith('"') ? w.slice(1, -1) : w))
    .join(' ')
    .trim();
  // Nothing recognised: the search is exactly what was typed.
  if (!filters.length) return { filters, text: query.replace(/["“”]/g, '').trim() };
  return { filters, text };
}
