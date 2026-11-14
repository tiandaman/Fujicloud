// A movie played while it's still being converted (iPhone HEVC on a PC
// whose window can't play it): the converter's output is fed into the
// player a stretch at a time, so playing starts in about a second
// instead of after the whole movie is done.

/** Still converting. */
export const RUNNING = 0;
/** All made, and the whole movie is in the cache. */
export const KEPT = 1;
export const FAILED = 2;
/** All made, but not kept for next time. */
export const DONE = 3;

export interface Chunk {
  state: number;
  bytes: Uint8Array;
}

export interface Playing {
  /** For the `<video>`'s `src`. */
  url: string;
  /** How it ended: `KEPT`, `DONE` or `FAILED`. Never rejects. */
  finished: Promise<number>;
  stop(): void;
}

/** Seconds kept ready ahead of the playhead; and, when the player is full, kept behind it. */
const AHEAD = 60;
const BEHIND = 30;

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Can this window play a movie as it's made? */
export const canStream = () => typeof MediaSource !== 'undefined';

/**
 * What the player needs to be told about the stream, read from its header:
 * the H.264 profile and level, and whether there's sound. Null until the
 * whole header (everything before the first fragment) has arrived.
 */
export function mimeOf(head: Uint8Array): string | null {
  const find = (tag: string, from = 0) => {
    outer: for (let i = from; i + 4 <= head.length; i++) {
      for (let k = 0; k < 4; k++) if (head[i + k] !== tag.charCodeAt(k)) continue outer;
      return i;
    }
    return -1;
  };
  if (find('moof') < 0) return null;
  const c = find('avcC');
  if (c < 0 || c + 8 > head.length) return null;
  const hex = (n: number) => n.toString(16).padStart(2, '0');
  const video = `avc1.${hex(head[c + 5])}${hex(head[c + 6])}${hex(head[c + 7])}`;
  return `video/mp4; codecs="${video}${find('mp4a') >= 0 ? ', mp4a.40.2' : ''}"`;
}

/**
 * Plays what `read(offset)` hands over. `video` is asked for now and then
 * (it isn't there until the player has the URL). `seconds` is the movie's
 * length when known, so the scrubber is the right size from the start.
 */
export function playStream(
  read: (offset: number) => Promise<Chunk>,
  video: () => HTMLVideoElement | null | undefined,
  seconds: number | null,
  mime?: string,
  /** Told why, when it fails. */
  why: (text: string) => void = () => {},
): Playing {
  const source = new MediaSource();
  const url = URL.createObjectURL(source);
  let stopped = false;

  const run = async (): Promise<number> => {
    if (source.readyState !== 'open') await new Promise((r) => source.addEventListener('sourceopen', r, { once: true }));
    let buffer: SourceBuffer | null = null;
    let held: Uint8Array[] = [];
    let offset = 0;
    const settled = () => new Promise<void>((r) => (buffer!.updating ? buffer!.addEventListener('updateend', () => r(), { once: true }) : r()));
    const playhead = () => video()?.currentTime ?? 0;
    /** Makes room: drops what's well behind the playhead. */
    const trim = async (keep: number) => {
      const b = buffer!.buffered;
      const until = playhead() - keep;
      if (!b.length || until <= b.start(0)) return false;
      buffer!.remove(0, until);
      await settled();
      return true;
    };
    const append = async (bytes: Uint8Array) => {
      for (let tries = 0; !stopped; tries++) {
        try {
          buffer!.appendBuffer(bytes as BufferSource);
          await settled();
          return;
        } catch (e) {
          // Full: make room behind the playhead, or wait for it to move on.
          if ((e as DOMException).name !== 'QuotaExceededError' || tries > 600) throw e;
          if (!(await trim(BEHIND)) && !(await trim(5))) await wait(500);
        }
      }
    };

    for (;;) {
      if (stopped) return FAILED;
      if (source.readyState !== 'open') {
        why(`the player closed the stream (${video()?.error?.message || 'no reason given'})`);
        return FAILED;
      }
      // Enough is ready: let the playhead catch up before fetching more.
      if (buffer) {
        const b = buffer.buffered;
        if (b.length && b.end(b.length - 1) - playhead() > AHEAD) {
          await wait(400);
          continue;
        }
      }
      const { state, bytes } = await read(offset);
      if (stopped) return FAILED;
      if (state === FAILED) {
        why(offset ? 'the converter stopped part-way' : 'the converter made nothing');
        return FAILED;
      }
      if (!bytes.length) {
        if (state === RUNNING) {
          await wait(120);
          continue;
        }
        if (!buffer) {
          why('the converter finished without a playable start');
          return FAILED;
        }
        await settled();
        if (source.readyState === 'open') source.endOfStream();
        return state;
      }
      offset += bytes.length;
      if (!buffer) {
        // The player has to be told what's coming before it's given any.
        held.push(bytes);
        const head = new Uint8Array(held.reduce((n, b) => n + b.length, 0));
        held.reduce((at, b) => (head.set(b, at), at + b.length), 0);
        held = [head];
        const type = mime ?? mimeOf(head);
        if (!type) continue;
        if (!MediaSource.isTypeSupported(type)) {
          why(`the window can't play ${type}`);
          return FAILED;
        }
        buffer = source.addSourceBuffer(type);
        if (seconds && seconds > 0) source.duration = seconds;
        await append(head);
        held = [];
        // A movie whose first picture isn't quite at zero would never start.
        const v = video();
        const b = buffer.buffered;
        if (v && b.length && b.start(0) > v.currentTime && b.start(0) < 1) v.currentTime = b.start(0);
      } else {
        await append(bytes);
      }
    }
  };

  const finished = run().catch((e) => {
    if (!stopped) why(`the player refused the movie (${e instanceof Error ? `${e.name}: ${e.message}` : String(e)})`);
    return FAILED;
  });
  return {
    url,
    finished,
    stop() {
      stopped = true;
      URL.revokeObjectURL(url);
    },
  };
}
