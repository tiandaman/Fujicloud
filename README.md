# Fujicloud

A fast, good-looking photo library for Fujifilm shooters on Windows. It brings the
SSD plugged into your NVIDIA Shield, your laptop, and new shots coming off the camera
over Wi-Fi into one place, with a Photos-style grid that stays smooth at tens of
thousands of images.

```
 ┌────────────┐  Wi-Fi   ┌──────────────────────────┐   LAN (SMB)   ┌──────────────────┐
 │  Fujifilm  │ ───────▶ │  Laptop: Fujicloud       │ ────────────▶ │  NVIDIA Shield   │
 │  camera    │ PC Auto- │  • watches the inbox     │  parallel     │  USB SSD         │
 └────────────┘  Save    │  • indexes + previews    │  copies       │  \\SHIELD\T7\... │
                         │  • hands off to Photoshop│ ◀──────────── │  (the library)   │
                         └──────────────────────────┘   browsing    └──────────────────┘
```

## What it does

- **Browse everything at once:** justified "photo rows" or a square grid, grouped by
  day, month, year, film simulation, camera, lens or folder. Ctrl + scroll resizes
  thumbnails.
- **Sort and filter the Fujifilm way:** film simulation (read from the camera's
  MakerNote: Classic Chrome, Classic Neg., Acros +R, Reala Ace, ...), plus camera, lens,
  focal length, aperture, shutter speed, ISO, rating, file type and date. Search covers
  names, lenses and film sims.
- **Click to grow:** a photo morphs from its tile to full screen. The cached thumbnail
  shows instantly, a 2560px preview swaps in, and full resolution loads only when you
  zoom past what the preview can show. Scroll to zoom at the cursor, drag to pan,
  double-click or `Z` for 100%, arrows or swipe to move through photos, and a filmstrip
  along the bottom.
- **RAW + JPEG as one photo:** pairs are stacked into one tile, and ratings and labels
  apply to both.
- **Culling:** `1`–`5` stars, `P` pick, `X` reject, `6`–`9` colour labels, and
  multi-select with Ctrl / Shift click.
- **Works with Adobe:** "Edit in Photoshop" (`Ctrl+E`) finds the newest Photoshop
  install and opens the RAF in Camera Raw when there is one. Ratings and labels are
  written to `.xmp` sidecars so Lightroom and Bridge see them. Existing sidecars are
  edited in place, so develop settings are kept.
- **Camera sync over your network:** new shots that land in the inbox folder are copied
  to the Shield's SSD, filed as `YYYY/YYYY-MM-DD/`, several files at a time. Thumbnails
  and previews are rendered from the local copy *while* it uploads, so a new photo can
  be browsed the moment it arrives.
- Histogram, full EXIF, drag a folder onto the window to add it, and Windows 11 Mica.

## Install (Windows)

Every push builds an installer on GitHub Actions. Open the latest **Build** run under
the repo's *Actions* tab, download **Fujicloud-Windows-installer**, and run the
`.exe` (per-user install, no admin needed) or the `.msi`.

## Set up your library and camera sync

1. **Share the Shield's SSD.** On the Shield: *Settings → Device Preferences → Storage
   → Transfer files over local network* → on. From Windows it then appears as
   `\\SHIELD\<drive name>` (try `\\SHIELD` in Explorer to see the name).
2. **Add it in Fujicloud.** Paste `\\SHIELD\<drive name>\Photos` (or click
   *Browse…*) on the welcome screen. Indexing starts immediately; thumbnails build in
   the background, and whatever is on screen always goes first.
3. **Send photos from the camera.** Install FUJIFILM **PC AutoSave**, pair the camera,
   and note the folder it saves into.
4. **Turn on sync.** In *Settings → Camera sync*, set the *Inbox folder* to the PC
   AutoSave folder and *Copy new photos to* to the Shield library folder.

**For speed:** put the Shield on Ethernet, keep laptops on 5/6 GHz Wi-Fi, and raise
*Parallel copies* (4–6 works well over SMB). The first full thumbnail build over the
network takes a while for a big library; after that, everything comes from the local
cache.

## Why it's fast

| Layer | Choice | Why |
| --- | --- | --- |
| App shell | [Tauri 2](https://tauri.app) (Rust) | Uses Windows' built-in WebView2: ~10 MB installer and low RAM, versus ~150 MB for Electron. |
| UI | Svelte 5 (runes) + Vite 8 | Compiles to plain DOM updates with no virtual DOM. The whole UI is ~45 KB gzipped. |
| Grid | Custom virtualised layout | Layout for 50k photos takes ~12 ms, and only on-screen tiles exist in the DOM. Measured at 60 fps median while flinging through 43k photos. |
| Images | `fuji://` protocol + `<img>` | No base64 over IPC. The webview decodes off the main thread, and URLs are immutable so they're cached. |
| Scheduling | 3-lane worker pool | The open photo comes first, then the visible thumbnails (LIFO, so a fast scroll serves what's on screen *now*), then background warm-up. |
| Decoding | DCT-domain downscale + SIMD resize | A 26 MP JPEG decodes at 1/8 size for thumbnails: ~60 ms thumbnail / ~150 ms preview per core. |
| RAW | RAF embedded JPEG | Reads a few MB from the RAF header instead of demosaicing 50 MB of X-Trans data. |
| Index | SQLite (WAL) | Incremental rescans only touch new or changed files. Ratings survive rescans. |
| Network | Parallel directory walk + metadata reads | SMB is latency-bound, so many requests stay in flight at once. |

## Project layout

```
crates/fuji-core/   Rust core, tested on its own
  raf.rs            RAF container → embedded JPEG
  fuji.rs           Fujifilm MakerNote (film simulation, grain, DR)
  meta.rs           EXIF extraction
  thumbs.rs         thumbnail/preview pipeline + disk cache
  db.rs             SQLite index, RAW+JPEG pairing, culling
  scan.rs           parallel incremental scanning
  ingest.rs         inbox → library filing
  xmp.rs            Lightroom/Bridge-compatible sidecars
src-tauri/          desktop shell: commands, fuji:// protocol, camera sync
src/                Svelte UI (grid, viewer, info panel, settings)
```

## Develop

Prerequisites: Node 22+, Rust (stable), and on Windows the
[Tauri prerequisites](https://tauri.app/start/prerequisites/) (WebView2 ships with
Windows 11).

```sh
npm install
npm run tauri dev        # the desktop app, with hot reload
npm run dev              # UI only, in a browser, with a generated demo library
                         #   ?days=3000 → ~50k photos   ?empty → onboarding screen
npm run tauri build      # installer → target/release/bundle/
cargo test -p fuji-core  # core tests
cargo run --release -p fuji-core --example thumb_bench [photo.jpg]
```

## Roadmap ideas

- HEIF (`.HIF`) previews through Windows' WIC codecs (today HIF files are indexed but
  show a placeholder).
- Read ratings back from `.xmp` sidecars during scans, so every laptop sees the same
  stars.
- Direct camera import over PTP/IP (no PC AutoSave needed).
- Side-by-side compare and a loupe for culling bursts.
- Map view from GPS tags, and local AI search ("red car at night") with an on-device
  CLIP model.
