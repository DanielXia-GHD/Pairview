# PRD — Camera Photo Library (working title)

| | |
|---|---|
| Status | Draft v0.1 — for review, no code yet |
| Owner | sunnydanniel@gmail.com |
| Date | 2026-11-01 |
| Platforms | macOS (Apple Silicon, M2) → `.dmg`; Windows 10/11 x64 (Nvidia GPU) → `.exe` installer |
| Audience | Personal use + friends (unsigned/ad-hoc builds acceptable) |
| Base | Fork of [RapidRAW](https://github.com/CyberTimon/RapidRAW) (AGPL-3.0) |

---

## 1. Problem

The owner shoots Sony **RAW+JPG** and **RAW+HEIF (.HIF)**. Today the RAW and the JPG/HIF are two unrelated files to every tool:

- Deleting a bad shot's JPG leaves an orphan RAW (or vice versa) → wasted space and a messy folder.
- Editing means manually finding the RAW, editing in one app, exporting, and replacing the JPG by hand.
- There's no easy way to say "this is a casual photo, keep only the JPG" across hundreds of files.
- Pixea has the viewer UX the owner likes, but handles Sony's 10-bit 4:2:2 HIF poorly. Lightroom treats HIF as a second photo, and Capture One doesn't read 10-bit HIF at all.

## 2. Product vision

**One photo = one thing**, however many files sit behind it. A fast, minimal, keyboard-driven viewer and library (the feel of Pixea) that treats `DSC01234.ARW` + `DSC01234.JPG` (or `.HIF`) as a single **Photo**. The app shows the pair, rates it, deletes it, edits it (from the RAW, automatically), and slims it down (drop the RAW) as one unit. It has a capable GPU RAW editor (RapidRAW) and one-click AI editing that learns the owner's style over time.

## 3. Goals and non-goals

### Goals
1. Browse any folder tree (internal SSD, external drive, NAS) with no import step. **Folders stay the source of truth.**
2. Browse by **date** (year → month → day timeline/calendar) as an alternative to the folder tree.
3. Manage RAW + JPG/HIF as one **Photo** through its whole lifecycle: view, rate, cull, edit, delete, slim down.
4. Pixea-class viewer: instant open, full screen, ←/→ navigation, zoom, slideshow.
5. First-class decoding of `.JPG`, `.ARW`, `.HIF`/`.HEIF`/`.HEIC`, `.NEF` (non-HE), `.DNG`, and the rest of RapidRAW's list.
6. One-click AI edit that starts generic and **learns the owner's style**.
7. Ship `.dmg` (arm64) and `.exe` (x64) from CI.

### Non-goals (v1)
- Cloud sync, multi-user, or mobile apps.
- Publishing to social networks or galleries.
- Generative AI (inpainting or sky replacement through cloud services). RapidRAW's cloud features stay disabled.
- Nikon **High Efficiency** NEF. No open-source decoder exists; it's patent-encumbered TicoRAW. Lossless-compressed NEF is supported.
- Face recognition/people albums. The owner did not select it; revisit later.
- Signed/notarized public distribution. Possible later if the audience grows; see §12.

## 4. Decisions already made (from the interview)

| # | Question | Decision | Consequence |
|---|---|---|---|
| D1 | Build path | **Fork RapidRAW** | Inherit the Tauri + Rust + React + wgpu editor, AI masks, and ONNX runtime. App is AGPL-3.0. |
| D2 | Audience | **Personal / friends** | AGPL is not a burden. Unsigned builds OK (with documented first-launch steps). |
| D3 | Edited output | **Overwrite the JPG, discard the camera original** | The RAW plus the edit recipe is the true master. See §6.4 for the one exception (JPG-only photos). |
| D4 | Capture formats | **Sony RAW+JPG and RAW+HEIF** | HIF 10-bit 4:2:2 decoding is a P0 risk to spike first. |
| D5 | Storage | **Internal SSD + external drive + NAS (SMB)** | Needs volume identity, offline handling, polling for NAS, and a local thumbnail cache. |
| D6 | Delete target | **App-managed "Recently Deleted"** (same volume, 30-day purge) | Same behavior on Mac, Windows, external drives, and NAS. |
| D7 | AI editing | **Learn my style** | Cold start with a generic auto-edit. Personalize after enough edits (§6.8). |
| D8 | Extras wanted | **SD-card import, AI culling, video files, search & map** | Phased in after the core (§10). |

---

## 5. Core concept — the Photo (asset model)

This is the heart of the product, and the thing every other tool gets wrong.

```
Photo (asset)
├── raw?        DSC01234.ARW                ← master when present
├── display?    DSC01234.JPG | DSC01234.HIF ← what you see, share, and export from
├── recipe?     DSC01234.ARW.rrdata         ← RapidRAW non-destructive edit
├── xmp?        DSC01234.xmp                ← rating / label / flag / keywords (Adobe-compatible)
└── state       RAW+DISPLAY | DISPLAY_ONLY | RAW_ONLY | EDITED | ORPHAN_RAW
```

### 5.1 Pairing rules (in priority order)
1. **Same folder, same basename**, case-insensitive (`DSC01234.ARW` ↔ `dsc01234.jpg`). Covers ~99% of cases.
2. **Basename collision guard.** Sony's counter wraps at 9999, and two Sony bodies both write `DSC0xxxx`. A pair must also match on EXIF `DateTimeOriginal` (+ `SubSecTimeOriginal`) within ±2 s **and** the camera serial when present. Otherwise the files are not paired, and the app shows a "possible mismatch" badge.
3. **Cross-folder pairing (opt-in).** Sony "Sort Recording" writes the RAW to slot 1 and the JPG/HIF to slot 2. If copied card-by-card, the files land in different folders. With the option on, pair by (camera serial + capture time + basename) inside the same date folder tree. **Better fix:** the SD-card importer merges both slots into one folder (§6.9), so this rule rarely fires.
4. RAW + JPG + HIF of the same frame (e.g. after a manual export) → one Photo. The newest non-RAW file is the display copy; the others are listed as "extra versions".

### 5.2 Photo states and what the UI shows

| State | Files on disk | Thumbnail badge | Notes |
|---|---|---|---|
| `RAW+DISPLAY` | ARW + JPG/HIF | `RAW+JPG` / `RAW+HIF` | The normal state after shooting. |
| `EDITED` | ARW + recipe + rendered JPG | `RAW+JPG ✎` | The JPG was rendered from RAW + recipe. |
| `DISPLAY_ONLY` | JPG/HIF only | `JPG` | RAW was intentionally dropped ("Keep JPG only") or never existed. |
| `RAW_ONLY` | ARW only | `RAW` | Shot RAW-only, or the user deliberately deleted the JPG in the app. |
| `ORPHAN_RAW` | ARW only, but a JPG **used to exist** | `⚠ RAW` | The JPG was deleted *outside* the app. Triggers the orphan prompt (§6.5). |

The app knows "used to exist" because the catalog remembers every pairing it has seen.

---

## 6. Functional requirements

Priority: **P0** = MVP, **P1** = v1.0, **P2** = later.

### 6.1 Library & browsing
- **P0** Add one or more **root folders**. Recursive scan; no copying or importing.
- **P0** **Folder tree** sidebar with photo counts. Clicking a folder shows its photos; a toggle includes **sub-folders** (recursive view).
- **P0** **Date view**: Year → Month → Day timeline with counts, scrollable grid grouped by day headers, and a jump-to-date calendar. Uses `DateTimeOriginal` + `OffsetTimeOriginal`. Falls back to file mtime, flagged "date uncertain".
- **P0** Grid of thumbnails (S/M/L), one tile per **Photo** (never per file). Shows the state badge, rating stars, and color label.
- **P0** Sort by capture time, filename, rating, or file size. Filter by rating, flag, label, state (`RAW+JPG`, `JPG only`, `RAW only`, `Edited`), file type, and camera.
- **P1** Batch **time shift** for a wrong camera clock or time zone (writes XMP, never touches the RAW).
- **P1** Stacks: bursts and brackets collapse into one tile (from culling, §6.7).
- **P2** Multiple windows/tabs.

**Catalog & cache (technical requirements)**
- SQLite catalog in the app data dir, keyed by **volume UUID + relative path**. An external drive keeps its catalog entries when unplugged and shows as **offline** with cached thumbnails. Mutating actions are disabled while offline.
- Thumbnails come from the RAW's **embedded JPEG preview** or the HIF's embedded thumbnail first (fast), then upgrade to a rendered preview in the background. Stored in a local cache (size cap, LRU).
- File watching: FSEvents (macOS) and ReadDirectoryChangesW (Windows) via the `notify` crate. **NAS/SMB:** events are unreliable, so the app polls (configurable, default 60 s) plus a manual "Rescan".
- Initial scan target: **50,000 photos on a local SSD in < 2 min to a browsable state**, with thumbnails continuing in the background.

### 6.2 Viewer (Pixea-inspired)
- **P0** Double-click or Space opens the Photo **in the same window** (no modal app switch). Esc returns to the grid at the same scroll position.
- **P0** ←/→ (and trackpad swipe) go to the previous/next Photo in the current grid order/filter. The next 2 and previous 1 Photos are prefetched so navigation is instant.
- **P0** **Full screen** (F / ⌃⌘F on Mac, F11 on Windows), with minimal chrome that auto-hides.
- **P0** Zoom: fit (`\`), 100% (`0` / Z-click), +/−, scroll-zoom, Space-drag pan. Show the display copy immediately, then swap to a full RAW decode on zoom when it's a RAW-only Photo.
- **P0** Rating `0–5`, pick `P` / reject `X`, color labels `6–9`, all available while viewing (they apply to the whole Photo).
- **P0** **Slideshow** (Enter or ▶): interval 1–30 s, fade/none transitions, shuffle, loop, follows the current filter (e.g. "★★★+ in 2026-10"), includes sub-folders, Space to pause.
- **P1** Info overlay (`I`): EXIF (camera, lens, shutter/aperture/ISO, focal), histogram, file list of the Photo (ARW 24 MB + JPG 8 MB), and GPS mini-map.
- **P1** Compare mode: 2–4 Photos side by side, synced zoom/pan (useful for culling bursts).
- **P1** HDR: Sony HLG HIF shown with HDR on capable displays (EDR on Mac), tone-mapped to SDR elsewhere.
- **P2** Open single files from Finder/Explorer ("Open with…") with a sibling-aware ←/→ through that folder.

### 6.3 Format support

| Format | Decode path (proposed) | Priority |
|---|---|---|
| JPG/PNG/TIFF/WebP/JXL… | RapidRAW `image` crate stack | P0 |
| ARW (all Sony incl. lossless-compressed), DNG, CR2/CR3, RAF, ORF, RW2… | RapidRAW → **rawler** (dnglab) | P0 |
| NEF (lossless / uncompressed) | rawler | P0 |
| NEF High Efficiency / HE* | **Not supported**; show a clear message: "Convert with Adobe DNG Converter or shoot lossless" | — |
| **HIF / HEIF / HEIC** (incl. Sony 10-bit 4:2:2, HLG) | **macOS:** Apple ImageIO (native, hardware HEVC). **Windows:** libheif + libde265 (LGPL, dynamically linked) via `libheif-rs`. Keep 10-bit through the edit pipeline. | **P0 — spike first** |
| MP4 / XAVC S / XAVC HS (H.264/H.265, up to 4:2:2 10-bit) | Thumbnails via FFmpeg (LGPL build); playback via native player where possible (§6.10) | P1 |

### 6.4 Editing workflow ("edit from RAW, overwrite the JPG")
- **P0** Pressing `E` on any Photo opens the RapidRAW editor on the **RAW** if present (the owner never has to find it), else on the display copy.
- **P0** Edits are stored non-destructively in the recipe sidecar (`.rrdata`). The recipe and RAW are the master.
- **P0** **Done / Save** renders a full-resolution JPG (quality 92 default, EXIF/GPS/rating copied, XMP `CreatorTool` + "edited" marker) and **overwrites the display copy under the same basename**, discarding the camera original (D3). State → `EDITED`.
  - If the display copy was a **HIF**: write `DSC01234.JPG` and move the `.HIF` to Recently Deleted (default). *Open question Q1.*
- **P0** **Re-edit** always reopens RAW + recipe, never the rendered JPG, so there's no generational loss.
- **P0** **Revert** = delete the recipe and re-render with defaults (or "match camera look", below).
- **P0 exception — JPG/HIF-only Photos (no RAW):** overwriting would compound JPEG loss and make edits irreversible. For these only, the **first** edit keeps the original as a hidden base (`.originals/DSC01234.JPG`), and every later render starts from that base. *(Deviates from D3 on purpose; confirm — Q2.)*
- **P1** **Match camera look:** the camera JPG has Sony's Creative Look / picture profile baked in. A RAW render with default settings looks different (flatter, different color), which will surprise the owner on the first edit. Before discarding the camera JPG, the app fits a per-camera base profile (tone curve + color matrix/LUT) by comparing many RAW→JPG pairs. Edits then start from "looks like my camera" instead of "flat RAW". The owner's own library supplies the training pairs.
- **P1** Copy/paste recipe and batch-apply to a selection (renders each, with progress and cancel).
- **P1** **Export** (separate from overwrite): resized copies for sharing (long edge, quality, sRGB, strip GPS option) to a chosen folder.

### 6.5 Delete & orphan handling
- **P0** Delete (`⌫`/`Del`) on a Photo opens one dialog with three choices; the default is remembered:
  - **Delete photo** (all files: RAW + JPG/HIF + recipe + xmp) — default
  - **Delete JPG only, keep RAW** → `RAW_ONLY`
  - **Delete RAW only, keep JPG** → `DISPLAY_ONLY` (same as "Keep JPG only", §6.6)
- **P0** Deleted files go to **Recently Deleted**: a hidden `.recently-deleted/` folder **at the root folder on the same volume** (instant move, no copy; works on NAS and external drives). Shows in the sidebar, supports restore, auto-purges after 30 days (configurable). "Empty now" reclaims space immediately.
- **P0** **Orphan detection.** When a scan finds a RAW whose paired JPG/HIF vanished *outside* the app (Finder, Explorer, phone sync), it marks it `ORPHAN_RAW` and shows a non-blocking banner: *"23 RAW files lost their JPG — Review"*. Review is a grid of orphans with **Delete RAWs** / **Keep as RAW-only** / **Ignore**. Never auto-delete.
- **P0** The reverse case (RAW vanished, JPG remains) is silently treated as `DISPLAY_ONLY`. There's no data-loss risk, though any recipe for that RAW becomes orphaned and is cleaned up.
- **P0** Every destructive batch operation writes an **operation log** and supports **Undo** until the files are purged.

### 6.6 Space saver ("Keep JPG only")
- **P0** Per selection: **Keep JPG only** → moves RAW(s) + recipe(s) to Recently Deleted. A confirmation shows the reclaimable size.
- **P0** Safety: if the Photo is `EDITED`, the rendered JPG is already baked, so this is allowed. If the recipe is newer than the rendered JPG, the app re-renders first.
- **P1** **Rule-based slimming** (dry run first): e.g. *"RAWs of photos rated ≤ 2 (or unflagged), unedited, older than 90 days, in folders except /Portfolio"*. Shows the count and GB, then lets the owner review in a grid before applying.
- **P1** Storage dashboard per root/volume: GB in RAW vs JPG vs video, plus the reclaimable estimate.
- **P2** Lossy-DNG option: convert RAWs to lossy DNG (~¼ size, still editable) as a middle ground between keeping and dropping the RAW.

### 6.7 AI culling assist (P1)
- Group **bursts / near-duplicates** by capture-time gaps + image-embedding similarity (CLIP is already bundled in RapidRAW).
- Per photo: **sharpness score** (on the detected subject/face, not the whole frame), **eyes-closed** and **face-blur** flags (MediaPipe face landmarks, already bundled), and exposure clipping warnings.
- Suggest the **best of each group** (★ badge). One key to "keep best, reject rest" per group, then **Delete rejects**, which removes whole Photos (both files).
- Cull view: big preview + filmstrip of the group, with P/X/←/→ only.
- Runs locally in the background after scan/import (CoreML on M2, DirectML on Nvidia).

### 6.8 One-click AI edit → "learns my style" (D7)
- **P0** **Auto** button: a generic local auto-adjust (exposure, WB, highlights/shadows, contrast, vibrance, straighten) from RapidRAW's auto + a small model. It sets **normal sliders**, so the result is fully tweakable.
- **P1** **Style learning**:
  - Every time the owner finishes an edit, the pair *(image features → final slider values)* is stored locally. Features: CLIP embedding, histogram/tone stats, EXIF (ISO, WB, lens, scene).
  - Once ≥ ~50 edits exist, train a small local regressor (gradient-boosted trees or kNN in embedding space, then a small MLP at ≥ 500) that predicts sliders. Training runs in seconds on-device.
  - **Auto** then means "how *I* would edit this": predicted sliders + confidence. Low-confidence predictions fall back to generic auto.
  - Multiple named **styles** (e.g. "Travel", "Family indoor") learned from edits tagged with that style. The app auto-picks one using nearest-neighbor on scene type.
  - A "Why?" panel shows the 3 most similar past edits that drove the prediction (transparent, builds trust).
- **P1** **Batch Auto**: apply style-predicted edits to a selection or a whole day, then review before overwriting.
- **P2** **Style from reference**: bootstrap by pointing at 20–50 already-edited JPGs (e.g. old Lightroom exports) when their RAWs exist, so it doesn't wait for 50 new edits.
- Privacy: all local; models and training data live in the app data dir, with export/reset.

### 6.9 SD-card import (P1)
- Detect an inserted card (DCIM present) → import sheet showing **only new** Photos (dedupe by camera serial + capture time + basename, with a hash fallback).
- Destination template, default `{Root}/{YYYY}/{YYYY-MM-DD}/`, and optional rename template (`{YYYYMMDD}_{HHMMSS}_{seq}`). Renames keep the pair together.
- **Merges both card slots into one destination**, so Sony Sort Recording pairs end up side by side (fixes §5.1 rule 3).
- Optional **second copy to a backup drive/NAS** during import; checksum verified; the app never formats or erases the card (the camera does that).
- Optional at import: "Keep JPG only for all", "RAW only for ★ later", run AI culling.

### 6.10 Video (P1)
- Sony MP4 / XAVC S / XAVC HS clips appear in grid and date views (duration badge, poster frame) and play in the viewer (space play/pause, scrub, J/K/L).
- **Risk:** XAVC HS **4:2:2 10-bit HEVC** has limited hardware decode (Apple M2 media engine support needs verification; Nvidia supports it only from RTX 50-series). Playback falls back to software decode or a cached proxy. Slideshows can include videos (play to end or clip to N s).
- No video editing beyond trim-to-export (P2).

### 6.11 Search & map (P1)
- Metadata search: camera, lens, focal range, ISO, aperture, date range, rating, state, folder.
- **Semantic search** ("dog on beach", "sunset", "snow"): local CLIP text→image embeddings, indexed in the background.
- **Map view**: Photos with GPS (Sony via the Creators' App location link) plotted on a MapLibre map with clustering. Click a cluster → grid. Map tiles need internet (OpenStreetMap-based), and the map is hidden when offline.

### 6.12 Metadata interoperability
- **P0** Ratings, labels, pick/reject, and keywords are written to **XMP sidecars** (`DSC01234.xmp`, Adobe convention) and read back, so Lightroom/Bridge/Capture One see the same ratings. Nothing is trapped in the catalog. *(RapidRAW's own `.rrdata` stays for edit recipes.)*
- **P1** Read existing XMP/Lightroom ratings on first scan.
- Never write into RAW files.

---

## 7. UX principles
1. **Pixea feel:** one window, content-first, chrome that disappears, everything on the keyboard, instant.
2. **The Photo, not the file.** The UI never shows two tiles for one shot. Files are visible in the Info panel when wanted.
3. **No surprise deletes.** Everything goes to Recently Deleted, batch ops have dry runs and undo, and the app never auto-deletes.
4. **The library stays usable without the app.** Plain folders, standard XMP, standard JPGs. Uninstalling loses nothing but the cache, recipes stay as sidecars, and learned styles are exportable.

Screens: **Library** (sidebar: Roots/Folders · Dates · Map · Recently Deleted · Smart filters; main: grid), **Viewer** (full-bleed), **Editor** (RapidRAW panels, entered with `E`), **Cull** (group view), **Import** (sheet), **Settings**.

---

## 8. Architecture (proposed)

```
Tauri 2 shell (forked RapidRAW)
├── Frontend: React + TypeScript + zustand
│   ├── existing RapidRAW editor UI (kept, restyled)
│   └── NEW: Library (folder tree, date timeline, grid), Viewer, Slideshow, Cull, Import, Map, Search
└── Backend: Rust
    ├── existing: rawler decode · wgpu/WGSL pipeline · ONNX Runtime (CoreML / DirectML) · Lensfun
    ├── NEW crate `library`:   SQLite catalog (rusqlite), volume IDs, scanner, watcher (notify) + NAS poller
    ├── NEW crate `assets`:    pairing engine, Photo state machine, lifecycle ops, op-log/undo, Recently Deleted
    ├── NEW crate `heif`:      macOS ImageIO bridge | Windows libheif+libde265
    ├── NEW crate `thumbs`:    embedded-preview extraction, cache (LRU), background render queue
    ├── NEW crate `xmp`:       read/write Adobe-compatible sidecars
    ├── NEW crate `ingest`:    card detection, copy/verify/backup, rename templates
    ├── NEW crate `style`:     edit-history store, feature extraction, regressor training/inference
    └── NEW: video thumbnails (FFmpeg LGPL, dynamic), CLIP index, MapLibre front-end
```

- **Fork strategy:** keep new code in **new crates/modules** and touch RapidRAW files minimally, so upstream fixes (new cameras, editor features) can be merged regularly. Track `upstream/main` and merge monthly.
- **GPU:** wgpu → Metal on Mac, DX12/Vulkan on Windows. AI via ONNX Runtime: CoreML EP on M2, DirectML EP on Windows (works on Nvidia without shipping ~1 GB of CUDA libs; CUDA/TensorRT EP is a later optimization).
- **Packaging:** Tauri bundler → `.dmg` (aarch64-apple-darwin, ad-hoc signed — required on Apple Silicon) and NSIS `.exe` (x86_64-pc-windows-msvc). Built on GitHub Actions macOS + Windows runners. Unsigned → documented first-launch steps (macOS: right-click → Open / `xattr -d com.apple.quarantine`; Windows: SmartScreen "More info → Run anyway").
- **Licensing:** whole app AGPL-3.0. libheif/libde265/FFmpeg LGPL, dynamically linked. Fine for personal/friends distribution; source shared with friends on request (or a public repo).

---

## 9. Non-functional requirements

| Area | Target (M2 MacBook / mid-range Nvidia PC, local SSD) |
|---|---|
| Grid scroll | 60 fps with 10k+ Photos in view; thumbnails visible within 100 ms when cached |
| Viewer next/prev | < 100 ms with prefetch (display copy); full RAW zoom-in < 1.5 s for 24–61 MP ARW |
| HIF decode | < 300 ms for a 33 MP 10-bit 4:2:2 HIF on Mac (ImageIO); < 800 ms on Windows (libheif, software) — **verify in spike** |
| Initial scan | 50k Photos browsable in < 2 min (local SSD); NAS proportional to link speed |
| Memory | < 1.5 GB in library/viewer mode; editor per RapidRAW (16 GB system recommended) |
| Safety | No code path permanently deletes outside "purge Recently Deleted"; all file moves are atomic renames on the same volume |
| Offline | Everything except map tiles works with no internet |

---

## 10. Milestones

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0 — Spikes (1–2 wk)** | Fork builds on Mac + Windows; produce unsigned `.dmg` + `.exe` in CI. **Sony HIF decode** on both OSes (10-bit 4:2:2 + HLG sample files). Pairing engine prototype on a real card dump. Video 4:2:2 playback check. | Real Sony ARW/JPG/HIF files from the owner's camera decode correctly on both machines |
| **M1 — Library + Viewer** | Roots, folder tree, recursive view, date timeline, grid of Photos with badges, catalog/cache/watcher, viewer, full screen, ←/→, zoom, slideshow, ratings → XMP | Owner can replace Pixea for daily browsing |
| **M2 — Lifecycle** | Delete dialog, Recently Deleted, orphan detection/review, Keep-JPG-only, op-log/undo, offline volumes, NAS polling | "JPG and RAW are never out of sync" — the original pain point is gone |
| **M3 — Edit loop** | `E` → editor on RAW, Done → overwrite JPG, re-edit/revert, JPG-only base rule, generic Auto, export copies, batch paste | Edit a shot end-to-end in < 30 s without touching Finder/Explorer |
| **M4 — Import + Cull** | SD import (slot merge, rename, backup copy), AI culling (bursts, blur, eyes, best-of) | Card → culled library in one sitting |
| **M5 — Find** | Metadata search, CLIP semantic search, map, video in grid/viewer/slideshow, storage dashboard, rule-based slimming | |
| **M6 — Style** | Edit-history capture (starting in M3!), style training, Auto = my style, named styles, "Why?", batch auto, match-camera-look | Auto edits accepted without changes ≥ 50% of the time |

Note: **edit-history capture ships in M3**, even though style learning is M6, so training data accumulates from day one.

---

## 11. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Sony 10-bit 4:2:2 HIF decoding is wrong/slow on Windows (libde265 is software-only) | Med | High | M0 spike first. Fallbacks: FFmpeg HEVC decoder via libheif plugin, or Windows Media Foundation/WIC (needs the HEVC extension). |
| Forking RapidRAW: large, fast-moving codebase (~10k★, daily changes) → merge pain | High | Med | Isolate new code in new crates; minimal edits to upstream files; merge upstream monthly. |
| RAW render ≠ camera JPG look → "my edit looks worse than the original" | High | Med | Match-camera-look profile (§6.4); show before/after against the camera JPG *before* it's discarded. |
| Discarding the camera JPG (D3) loses the Sony look forever | Med | Med | Covered by match-camera-look plus an optional setting to keep originals; confirm Q2/Q3. |
| NAS: slow scans, no FS events, SMB locking/renames | Med | Med | Local thumbnail cache, polling, atomic renames within the share, conservative concurrency. |
| Cross-folder/collision pairing makes a wrong pair → deleting the wrong RAW | Low | **High** | Strict multi-key matching; mismatch badge; deletes always go to Recently Deleted; orphan review never auto-deletes. |
| Style learning underwhelms with few edits | Med | Low | Generic Auto fallback; confidence display; "style from reference" bootstrap. |
| Unsigned builds blocked by Gatekeeper/SmartScreen for friends | High | Low | Documented steps; upgrade to Apple Developer ID ($99/yr) if friends complain. |
| 4:2:2 10-bit video can't play smoothly | Med | Low | Proxy generation; video is P1, not core. |

---

## 12. Open questions (please answer before M1)

1. **HIF → JPG on edit?** When the display copy is a `.HIF`, should the edited output be a **JPG** (universal; HIF moves to Recently Deleted) or a **10-bit HEIF** (keeps HDR/10-bit, smaller, but needs an HEVC encoder — x265 is GPL, kvazaar BSD — and less compatible when sharing)? *Proposed: JPG.*
2. **JPG-only edit base:** OK to keep a hidden original **only** for Photos that have no RAW (§6.4 exception)?
3. **Keep camera JPG during "trial" edits?** Should the camera JPG go to Recently Deleted (30-day undo) instead of being destroyed instantly on overwrite? It costs no extra effort and gives a safety net. *Proposed: yes.*
4. **Camera bodies:** which exact Sony models (e.g. A7 IV, A7C II, A1 II)? Needed for HIF/ARW test files and rawler coverage checks. Please provide ~10 sample pairs (ARW+JPG, ARW+HIF, an HLG HIF, an XAVC HS clip).
5. **Library size:** roughly how many photos/GB today, and how much on the NAS? Sets the indexing/cache budget.
6. **Other apps:** do you still use Lightroom/Capture One alongside this? (Decides how much XMP compatibility matters vs RapidRAW's own format.)
7. **Delete dialog default:** "Delete photo (both files)" every time, or do you want a setting to skip the dialog?
8. **Phone photos:** none for now (not selected). Confirm iPhone HEIC + Live Photos are out of scope?
9. **App name** (for the `.dmg`/`.exe` and window title)?

---

## 13. Research notes (what other photographers do, and what we borrowed)

- **"Cull on JPG, drop matching RAWs"** is the most common manual workflow. Users filter to JPGs, delete rejects, then select RAWs by filename and delete the inverse ([Capture One community](https://support.captureone.com/hc/es/community/posts/8371248557597-RAW-JPG-workflow-culling)). Tools like Photo Workflow do this with one button ([App Store](https://apps.apple.com/app/id6466960429)). **→ We make it implicit: the Photo is the unit.**
- **Pair-aware deletes are fragile elsewhere.** Geeqie deletes pairs together; darktable users report deleting the RAW but leaving the JPG; Lightroom's "JPG as sidecar" mode can't delete the JPG separately ([pixls.us](https://discuss.pixls.us/t/culling-raw-and-matching-jpg/15769), [Adobe community](https://community.adobe.com/questions-675/what-to-do-with-jpg-raw-files-964221)). **→ Explicit 3-way delete dialog + Recently Deleted.**
- **Orphan RAW scripts.** Some photographers keep RAWs in a subfolder, cull JPGs, then run a script to find RAWs whose JPG is gone ([pixls.us](https://discuss.pixls.us/t/culling-photos-fast/15119/19)). **→ Built-in orphan detection.**
- **RAW+HEIF is badly supported.** Lightroom imports RAW+HEIF as duplicates and has no "HEIF next to RAW" setting ([Adobe feature request](https://community.adobe.com/t5/lightroom-classic-ideas/only-import-raw-files-when-raw-heif-both-exists/idi-p/14140133)). Capture One doesn't read 10-bit HEIF ([Capture One support](https://support.captureone.com/hc/en-us/articles/360016552258/comments/11624055277341)). **→ HIF is a first-class display copy here.**
- **Sony Sort Recording** splits RAW and JPG/HEIF across the two card slots ([Sony help guide](https://helpguide.sony.net/ilc/2440/v1/en/contents/201h_file_format.html)). **→ Importer merges slots; cross-folder pairing fallback.**
- **AI culling** (Aftershoot, Narrative Select) centers on burst grouping, best-shot suggestion, focus and closed-eye detection ([Imagen overview](https://imagen-ai.com/post/best-ai-photo-culling-software/), vendor-written, so the claims are unverified). **→ §6.7, local models.**
- **Personalized auto-edit** is what Imagen AI and Aftershoot Edits sell (profiles trained on your past edits). **→ §6.8, done locally.**
- **Nikon HE NEF** has no open-source decoder (TicoRAW, proprietary SDK) ([pixls.us](https://discuss.pixls.us/t/nikon-z-series-high-efficiency-raw-support/50428)). **→ Explicit non-goal.**
- **libheif** decodes HEVC HEIF via libde265 or FFmpeg; LGPL; Rust bindings via `libheif-sys`/`libheif-rs` ([libheif](https://github.com/strukturag/libheif)). 4:2:2 10-bit behavior must be verified on real Sony files (M0).
- **RapidRAW** already provides: folder tree, ratings, labels, pick/reject, culling view, AI masks, denoise, CLIP auto-tagging, presets, batch export, `.rrdata` sidecars, ONNX on CoreML/DirectML, Mac/Windows builds. It does **not** list HEIF/HIF support, date browsing, or RAW+JPG pairing ([RapidRAW README](https://github.com/CyberTimon/RapidRAW)).
- **Pixea** UX to emulate: same-window viewer, WASD/arrow/zoom shortcuts, theatre mode + slideshow on Enter, auto-hiding thumbnail strip, status bar with resolution and date ([Pixea](https://www.imagetasks.com/pixea/)).
