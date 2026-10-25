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
  multi-select with Ctrl / Shift click. `Ctrl+Z` undoes ratings, flags, labels and
  favourites. *Delete all rejected…* (or `Del`) moves photos to the Recycle Bin, RAW
  twins and sidecars included. Windows has no Recycle Bin for network drives, so from
  the Shield they go to a `.fujicloud-trash` folder in the library instead.
- **Favourites and albums:** press `.` or the heart to favourite, separate from stars.
  Albums live in the sidebar; add photos from the right-click menu, the viewer or the
  info panel.
- **Works with Adobe:** "Edit in Photoshop" (`Ctrl+E`) finds the newest Photoshop
  install and opens the RAF in Camera Raw when there is one. Ratings and labels are
  written to `.xmp` sidecars so Lightroom and Bridge see them. Existing sidecars are
  edited in place, so develop settings are kept.
- **Laptops stay in sync:** ratings, picks, labels, favourites and albums travel in the
  sidecars next to the photos, so every laptop that opens the Shield library sees the
  same choices. Changes made while the Shield is unreachable are saved once it's back.
- **Camera and phone sync over your network:** new shots that land in an inbox folder
  (the camera's PC AutoSave folder, iCloud for Windows' *iCloud Photos* folder, or any
  other) are copied to the Shield's SSD, filed as `YYYY/YYYY-MM-DD/`, several files at
  a time. Every copy is checked against the original. Thumbnails and previews are
  rendered from the local copy *while* it uploads, so a new photo can be browsed the
  moment it arrives. Optionally the inbox file goes to the Recycle Bin once its copy
  checks out (never for iCloud, where that would delete it from the phone).
- **Backup:** point *Settings → Backup* at a second drive or your OneDrive folder and
  the library is mirrored there in the background (after imports and every few
  hours), every copy checked. Nothing is ever deleted from the backup, and the sidebar
  shows when it last finished.
- **When the Shield is off:** the library stays browsable from the thumbnail cache,
  new shots wait on the PC and are copied when it's back, and nothing is removed from
  the library because a share went quiet.
- **Before and after:** edits are matched to the shot they came from: a PSD, a TIFF, or
  an `-Edit.jpg` next to the original or in a subfolder. Press `C` (or *Before / After*
  in the viewer) to see the camera's version and your edit **side by side**, or on
  one image with a **slider** you drag across. Zoom and pan move both sides together,
  so you can compare the same eye or the same sky at 100%. Edits get a badge in the
  grid and inherit the original's date, camera and location. Select any two photos and
  right-click to compare them, too.
- **HEIC and HEIF:** iPhone photos and Fujifilm's HEIF mode (`.HEIC`, `.HIF`) get
  thumbnails and previews with a built-in decoder, so no Microsoft Store codec is
  needed. Rotation and the iPhone's Display P3 colour are handled, and HEIC or PNG
  pictures that were saved with a `.jpg` name show up too.
- **The rest of the iPhone:** Live Photos (the still and its movie as one tile, with
  *LIVE* in the viewer to play it), videos (`.MOV`, `.MP4`) with their length, date and
  place, ProRAW `.DNG` and PNG screenshots. Video thumbnails come from Windows, so iPhone
  HEVC clips need Microsoft's free *HEVC Video Extensions* to show and play.
- **Photoshop files in the grid:** layered PSD/PSB and TIFF files get thumbnails and
  previews from their flattened image (8- or 16-bit, compressed or not). Colour is
  managed, so Adobe RGB and ProPhoto edits look the same as they do in Photoshop
  rather than washed out.
- **Recipe cards:** the full in-camera film recipe is read from every shot: film
  simulation, grain, Color Chrome and FX Blue, white balance and its shift, dynamic
  range, highlight/shadow, color, sharpness, noise reduction, clarity and monochrome
  toning. Photos are grouped by recipe in the sidebar. You can name a recipe
  ("My Kodachrome"), copy it as text, or group the grid by recipe.
- **Places and a world map:** GPS from the camera (via location sync in the FUJIFILM
  XApp) is turned into "Kyoto, Japan" offline, using 171k GeoNames places built into the
  app. Shots without GPS borrow the location of a geotagged shot from the same half
  hour. *Places* shows a 3D globe with photo bubbles that split apart as you zoom; click
  one to see its photos. The globe works offline, and street detail is added when
  you're online.
- **Search by what's in the photo:** type "golden hour at the beach" or "dog in the
  snow". Google's SigLIP model runs on your PC through ONNX Runtime (a one-time
  download, enabled with the ✦ in the search box). Photos are analysed on your
  **graphics card** through DirectML (any DirectX 12 GPU: NVIDIA, AMD or Intel; the
  discrete one on laptops that have two), and on the CPU if there isn't one. Each
  photo is analysed once from its cached thumbnail, and a search over 100k photos takes
  about 20 ms. Right-click any photo to *Find similar photos*. Nothing leaves your
  network.
- **Export and share:** right-click → *Export…* (`Ctrl+Shift+E`) for JPEGs sized for the
  web (2048 px) or Instagram (1080 px wide), full-size JPEGs or the original files.
  Exports are upright sRGB, keep the date and camera, and leave the location out unless
  you ask. Or drag photos straight out of the grid into Explorer, Photoshop, a browser
  upload or a chat.
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
4. **Turn on sync.** In *Settings → Camera and phone sync*, add the PC AutoSave folder
   (and *iCloud Photos* if iCloud for Windows is installed; Fujicloud offers the ones it
   finds) under *Inbox folders*, and set *Copy new photos to* to the Shield library
   folder.

**For speed:** put the Shield on Ethernet, keep laptops on 5/6 GHz Wi-Fi, and raise
*Parallel copies* (4–6 works well over SMB). The first full thumbnail build over the
network takes a while for a big library; after that, everything comes from the local
cache.

### If the smart search model won't download

Fujicloud fetches the model from huggingface.co the first time you turn on smart
search, and resumes if the connection drops. If your network blocks Hugging Face
(some antivirus web filters, routers and ISPs do), download the files in a browser
instead, from a network that allows it, and put them in
`%LOCALAPPDATA%\app.fujicloud.desktop\models\siglip-base-patch16-224\`:

- [tokenizer.json](https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/tokenizer.json)
- [text_model_quantized.onnx](https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/onnx/text_model_quantized.onnx)
- [vision_model_fp16.onnx](https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/onnx/vision_model_fp16.onnx)
  for the graphics card, and/or
  [vision_model_quantized.onnx](https://huggingface.co/Xenova/siglip-base-patch16-224/resolve/main/onnx/vision_model_quantized.onnx)
  for the CPU

Then switch smart search off and on in Settings. It uses the files it finds there.

### If something goes wrong

Fujicloud keeps a log at `%LOCALAPPDATA%\app.fujicloud.desktop\logs\fujicloud.log`
(*Settings → Help → Open log folder*). A photo it can't read is skipped and noted
there, with the reason, instead of stopping the app. Include that file when reporting
a problem. If thumbnails look wrong after an update, *Settings → Rebuild thumbnails*
makes them all again.

## Why it's fast

| Layer | Choice | Why |
| --- | --- | --- |
| App shell | [Tauri 2](https://tauri.app) (Rust) | Uses Windows' built-in WebView2: ~10 MB installer and low RAM, versus ~150 MB for Electron. |
| UI | Svelte 5 (runes) + Vite 8 | Compiles to plain DOM updates with no virtual DOM. The UI is ~70 KB gzipped; the map loads only when you open Places. |
| Grid | Custom virtualised layout | Layout for 50k photos takes ~12 ms, and only on-screen tiles exist in the DOM. Measured at 60 fps median while flinging through 43k photos. |
| Images | `fuji://` protocol + `<img>` | No base64 over IPC. The webview decodes off the main thread, and URLs are immutable so they're cached. |
| Scheduling | 3-lane worker pool | The open photo comes first, then the visible thumbnails (LIFO, so a fast scroll serves what's on screen *now*), then background warm-up. |
| Decoding | DCT-domain downscale + SIMD resize | A 26 MP JPEG decodes at 1/8 size for thumbnails: ~60 ms thumbnail / ~150 ms preview per core. |
| RAW | RAF embedded JPEG | Reads a few MB from the RAF header instead of demosaicing 50 MB of X-Trans data. |
| Index | SQLite (WAL) | Incremental rescans only touch new or changed files. Ratings survive rescans. |
| Network | Parallel directory walk + metadata reads | SMB is latency-bound, so many requests stay in flight at once. |
| Smart search | SigLIP via ONNX Runtime + DirectML | fp16 on the GPU (batches of 32) or int8 on the CPU. Results are stored as 8-bit vectors (~770 bytes per photo, 75 MB per 100k in RAM) and searched by a brute-force scan with no index to maintain. |
| Map | MapLibre GL (WebGL) + Supercluster | A GPU-rendered globe, loaded only when you open Places. Clustering 100k points takes milliseconds. |
| Places | GeoNames gazetteer bucketed in a 1° grid | Offline reverse geocoding in microseconds per photo. |

## Project layout

```
crates/fuji-core/   Rust core, tested on its own
  raf.rs            RAF container → embedded JPEG
  fuji.rs           Fujifilm MakerNote (film simulation, grain, DR)
  meta.rs           EXIF extraction
  decode.rs         PSD/TIFF decoding and ICC colour conversion to sRGB
  thumbs.rs         thumbnail/preview pipeline + disk cache
  db.rs             SQLite index, RAW+JPEG pairing, edit linking, culling
  scan.rs           parallel incremental scanning
  ingest.rs         inbox → library filing
  xmp.rs            Lightroom/Bridge-compatible sidecars
  geo.rs            offline reverse geocoding (GeoNames, bundled)
  embed.rs          SigLIP image/text embeddings (ONNX Runtime)
  search.rs         8-bit vector index for smart search
src-tauri/          desktop shell: commands, fuji:// protocol, camera sync, smart search jobs
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
pwsh scripts/fetch-directml.ps1   # Windows: DirectML runtime for GPU smart search
npm run tauri build -- --config src-tauri/tauri.directml.conf.json   # …bundled with it
cargo test -p fuji-core  # core tests (add `-- --ignored` to test the real search model)
cargo run --release -p fuji-core --example thumb_bench [photo.jpg]
```

## Roadmap ideas

- Direct camera import over PTP/IP (no PC AutoSave needed).
- Group bursts into stacks using smart search vectors.
- Import a phone's location history (GPX / Google Takeout) for shots taken without
  location sync.

## Credits

Place names © [GeoNames](https://www.geonames.org) (CC BY 4.0). Map data ©
OpenStreetMap contributors, tiles by [OpenFreeMap](https://openfreemap.org). Country
borders from Natural Earth via `world-atlas`. Smart search uses Google's SigLIP model,
via the [Xenova ONNX export](https://huggingface.co/Xenova/siglip-base-patch16-224). HEIC decoding by
[heic-decoder](https://github.com/Multipad-cyber/heic-decoder-rust); test images from
[heic-rs](https://github.com/tbraun96/heic-rs).
