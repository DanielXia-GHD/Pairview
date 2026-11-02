# PRD — Pairview

| | |
|---|---|
| Status | Draft v0.3.1 — open questions resolved, no feature code yet |
| Owner | sunnydanniel@gmail.com |
| Date | 2026-11-01 |
| Platforms | **macOS (Apple Silicon, M2) first** → `.dmg`; Windows 10/11 x64 (Nvidia, HEVC extension installed) → `.exe`, parity where cheap |
| Audience | Personal use only |
| Base | Fork of [RapidRAW](https://github.com/CyberTimon/RapidRAW) (AGPL-3.0), kept mergeable with upstream |
| Camera | Sony **A7C II** (ILCE-7CM2, fw 2.01), shooting primarily **ARW + HIF**, sometimes ARW + JPG |

### Changelog
- **v0.3.1** — Repo renamed to `DanielXia-GHD/Pairview`. FTP folder naming confirmed (same `NNNYMMDD` as the card). Opt-in **auto-tidy** of new FTP shoots. Fixed the wrong year in the `10561031` example (it is 2026-10-31).
- **v0.3** — Named **Pairview**. Fork connected as `origin`. Library Locations manager (§6.1.1). Camera-FTP arrivals (§6.9.1). Keep-RAW is now **AI triage first, human confirmation after**, with a hard rule that AI never drops a RAW on its own (§6.5). Slim review can be triggered manually; retention fixed at 30 days. Existing `arw/`/`hif/` subfolders stay, future shoots are flat.
- **v0.2** — Owner's folder conventions (§5.1). Real sample-file findings (§5.4). Keep-RAW decision workflow (§6.5), now driving delete and space saver. Edited output stays HEIF. Camera-original goes to Recently Deleted. Looks library expanded (Fujifilm/Nikon/etc.). Folder-scoped lazy indexing for the NAS. Lightroom/Capture One library ideas (§6.12). Mac-first milestones with a Windows tabling rule (§10). Name ideas (§13).
- v0.1 — Initial draft.

---

## 1. Problem

The owner shoots Sony **ARW + HIF** (sometimes ARW + JPG). Every tool treats the RAW and the HIF as two unrelated files:

- Deleting a bad shot's HIF leaves an orphan RAW (or vice versa). That wastes space and leaves a messy folder.
- Editing means manually finding the RAW, editing in one app, exporting, and replacing the HIF by hand.
- There's no principled way to decide **which photos deserve their 39 MB RAW** and which are fine as a 9 MB HIF, and no tool to act on that decision in bulk.
- Pixea has the viewer UX the owner likes but handles Sony's 10-bit HIF poorly. Lightroom imports RAW+HEIF as duplicates. Capture One can't read 10-bit HEIF at all.

## 2. Product vision

**One photo = one thing**, however many files sit behind it. A fast, minimal, keyboard-driven viewer and library with the feel of Pixea. It treats `DSC03545.ARW` + `DSC03545.HIF` as a single **Photo**, even when they sit in `arw/` and `hif/` subfolders. The app shows, rates, culls, edits (from the RAW, automatically), and **slims** each Photo as one unit, guided by a Keep-RAW recommendation that explains itself. It includes a capable GPU RAW editor (RapidRAW) and one-click AI editing that learns the owner's style and can start from the owner's own Sony look or other classic camera looks.

## 3. Goals and non-goals

### Goals
1. Browse the NAS photo tree (also the internal SSD and external drives) with **no import step**. **Folders stay the source of truth.**
2. Browse by **date** (year → month → day) as an alternative to the folder tree.
3. Manage RAW + HIF/JPG as one **Photo** through its whole lifecycle: view, rate, cull, edit, **decide Keep RAW**, slim, delete.
4. Pixea-class viewer: instant open, full screen, ←/→ navigation, zoom, slideshow.
5. First-class decoding of `.HIF`, `.ARW`, `.JPG`, plus RapidRAW's full RAW list.
6. One-click AI edit that starts from a camera look and **learns the owner's style**.
7. Ship a `.dmg` (arm64) first, then a `.exe` (x64).

### Non-goals (v1)
- Cloud sync, multi-user, mobile apps, publishing to social networks.
- **Phone photos** (iPhone HEIC / Live Photos). Confirmed out of scope.
- Generative/cloud AI. RapidRAW's cloud features stay disabled.
- Nikon High Efficiency NEF (no open-source decoder).
- Face recognition/people albums (revisit later).
- Signed/notarized distribution.

## 4. Decisions

| # | Topic | Decision | Consequence |
|---|---|---|---|
| D1 | Build path | **Fork RapidRAW** and **keep merging upstream** | New code lives in new modules; minimal edits to upstream files; regular upstream merges (§8.1). |
| D2 | Audience | **Personal use only** | AGPL is no burden. Unsigned/ad-hoc builds are fine. |
| D3 | Edited output | **Overwrite the display copy in its own format** (HIF stays HIF, JPG stays JPG). The camera original goes to **Recently Deleted** (30-day safety net), not destroyed instantly. | RAW + recipe is the master. JPG/HIF-only Photos keep a hidden base (§6.4). |
| D4 | Formats | **Mix of ARW+JPG and ARW+HIF, primarily ARW+HIF** | HIF is the main display format. It must be as fast and correct as JPG everywhere. |
| D5 | Storage | **NAS is primary** (Mac on Wi-Fi 6 → router → NAS on Ethernet), plus internal SSD and external drives | Folder-scoped lazy indexing, local thumbnail cache, polling instead of FS events on SMB (§6.1). |
| D6 | Delete target | **App-managed Recently Deleted** on the same volume/share, 30-day purge | Same behavior everywhere, including the NAS. |
| D7 | AI editing | **Learn my style** | Cold start from a camera look + generic auto; personalize after enough edits. |
| D8 | Extras | **SD-card import, AI culling, video, search & map** | Phased after the core. |
| D9 | Keep-RAW | **App recommends, owner decides**, per shoot and per photo (§6.5) | Drives the delete dialog and the space saver. |
| D10 | Platform priority | **Mac first.** Expensive or hard Windows-only work is raised and tabled (§10.1). | Windows gets parity only where it's cheap. |
| D11 | Triage order | **AI triage first, human rating after.** AI may suggest, sort, and pre-select, but **never drops a RAW or deletes a Photo without explicit human confirmation**. | AI output is stored separately from human ratings (§6.5). |
| D12 | Retention | **30 days** for everything in Recently Deleted; slim review can also be **triggered manually** at any time | §6.5 Stage 4, §6.6 |
| D13 | Folder layout | Existing `arw/`/`hif/` subfolders **stay as they are**; **future shoots are flat** (one folder per day) | §5.1, §6.9 |
| D14 | Name | **Pairview** | Branding (app name, bundle id `com.danielxia.pairview`, icon) changes in M0 |

---

## 5. Core concept — the Photo

### 5.1 The owner's folder conventions (input the app must understand)

```
/Volumes/Public/Photos/A/DCIM/
├── 2025-10-31_15651031/          ← owner-renamed: YYYY-MM-DD_ + camera folder name
├── 2026-10-24_10361024/
│   ├── arw/DSC03538.ARW …        ← optional split into format subfolders
│   └── hif/DSC03538.HIF …
└── 10561031/                     ← camera default (NNN Y MMDD), not yet renamed
    ├── DSC03600.ARW
    └── DSC03600.HIF
```

- **Camera folder name `NNNYMMDD`**: `NNN` is the folder counter (100–999; resets to 100 when the card is formatted), `Y` is the last digit of the year, `MMDD` is the month and day. One folder per day. The app parses it as a **date hint** and resolves the decade from EXIF. **EXIF `DateTimeOriginal` always wins.** The folder name is only the fallback and the "date uncertain" check.
- **Owner prefix `YYYY-MM-DD_`**: recognized, and also treated as a hint.
- **Format subfolders** (`arw/`, `hif/`, `jpg/`, `raw/`, any case): a folder plus its immediate format subfolders form one **shoot** (pairing scope). The grid shows Photos, never the subfolders, unless the owner expands them.
- `A/DCIM/` is simply the path the **camera's FTP transfer** writes to on the NAS. It carries no meaning, and Pairview works with **any** folder added to the library (§6.1.1). Because the camera uploads over FTP, new shoots can **appear while Pairview is running** (§6.9.1).
- Existing shoots with `arw/`/`hif/` subfolders are **left as they are** (D13). Future shoots are flat.
- The manual split into subfolders and the manual rename are **no longer necessary**. Both are handled in the app (shoot view) and by the importer (§6.9). Optional **Tidy shoot** action: rename `10561031` → `2026-10-31_10561031`, and flatten or split subfolders on request. It always previews first.

### 5.2 The Photo (asset model)

```
Photo
├── raw?       arw/DSC03545.ARW              ← master when present
├── display?   hif/DSC03545.HIF | DSC03545.JPG   ← what you see/share
├── base?      .originals/DSC03545.HIF       ← only for Photos without RAW (§6.4)
├── recipe?    DSC03545.ARW.rrdata           ← RapidRAW non-destructive edit
├── xmp?       DSC03545.xmp                  ← rating/label/flag/keywords (Adobe-compatible)
├── keepRaw    auto | keep | drop            ← §6.5
└── state      RAW+DISPLAY | EDITED | DISPLAY_ONLY | RAW_ONLY | ORPHAN_RAW
```

### 5.3 Pairing rules
1. **Same shoot (folder + format subfolders), same basename**, case-insensitive. One folder = one day, so the counter-wrap/collision risk from v0.1 is mostly gone.
2. **Guard:** pairs must also agree on EXIF `DateTimeOriginal` within ±2 s (and camera serial if present). A mismatch means no pairing, and the app shows a "possible mismatch" badge. This protects against the rare same-day counter wrap or a second camera.
3. RAW + HIF + JPG of the same frame → one Photo. Extras are listed as versions.

States and badges are unchanged from v0.1: `RAW+HIF`, `RAW+JPG`, `✎` edited, `HIF`/`JPG` only, `RAW` only, `⚠ RAW` orphan.

### 5.4 Facts from the owner's sample (`DSC03545`, 2026-10-24)

| | ARW | HIF |
|---|---|---|
| Size | 38.8 MB (42.5 MB for DSC03540) | 9.5 MB (7.8–12.9 MB range) |
| Pixels | 7008 × 4672 (33 MP) | 7008 × 4672, **10-bit**, sRGB (not HLG) |
| macOS native decode (`sips`, incl. NAS read) | ~4.7 s full RAW develop | **~0.5 s** |
| rawler (RapidRAW) camera support | `a7cm2.toml` present ✔ | n/a (RapidRAW has **no HEIF support** today) |

- **NAS read speed** measured from the Mac: ~77 MB/s (one 42 MB ARW in 0.55 s). A full 100-photo day (~4.8 GB) takes about 60 s to read in full, so the app must **never read whole files just to browse** (§6.1).
- **Dropping the RAW saves ~80%** per Photo (39 of ~48.5 MB).
- **The camera-look gap is real.** The HIF is visibly brighter, more saturated, and contrastier than a default RAW develop of the same frame. This confirms the need for §6.4 *Match my camera look*.

---

## 6. Functional requirements

Priority: **P0** = MVP, **P1** = v1.0, **P2** = later.

### 6.1 Library, browsing & indexing
- **P0** Add **root folders** (e.g. `/Volumes/Public/Photos`). Nothing is copied.
- **P0** **Folder tree** with photo counts, with toggle for **include sub-folders**. Shoots with `arw/`+`hif/` show as one folder of Photos.
- **P0** **Date view**: Year → Month → Day with counts, day-grouped grid, and a jump-to-date calendar.
- **P0** Grid of **Photos** (S/M/L) with state badge, stars, label, and Keep-RAW indicator.
- **P0** Sort by time/name/rating/size. Filter by rating, flag, label, state, Keep-RAW, edited/unedited, camera/lens.
- **P1** Batch **time shift** (wrong camera clock/time zone), written to XMP.

#### 6.1.1 Library Locations (manage library folder paths) — P0
*Owner's question: "Is there a feature to manage library or photo folder paths?"* Upstream RapidRAW today has a basic list of root folders, pinned folders, and the last-opened folder, but no status, settings, or relocation. Pairview adds a **Library Locations** manager (Settings → Library, plus the sidebar header):

| Capability | Detail |
|---|---|
| Add / remove locations | Any folder on the internal SSD, an external drive, or a NAS share. Removing a location forgets its catalog/cache entries and never touches the files. |
| Status per location | Online/offline, photo count, size (RAW vs HIF/JPG vs video), last scan time, indexing progress, space reclaimable (§6.5) |
| **Relocate / reconnect** | When a path changes (e.g. the NAS mounts as `/Volumes/Public-1`, a drive letter changes on Windows, or a folder is moved), point the location to its new path. Ratings, edits, and cache are kept. Pairview also **suggests** the reconnect automatically when it finds the same folder structure at a new path. |
| Auto-mount hint | If the NAS share isn't mounted, show **Connect** (opens the share's `smb://` URL via the OS) instead of an empty library |
| Per-location settings | Index scope (whole location vs opened folders), poll interval, **exclude** sub-folders (e.g. `@eaDir`, `#recycle`, exports), Recently Deleted location, "watch for new shoots" (FTP inbox) |
| Display name & order | e.g. "NAS — Camera" instead of `/Volumes/Public/Photos/A/DCIM`; drag to reorder in the sidebar |
| Pinned folders | Kept from RapidRAW: pin any sub-folder for quick access |

**Indexing strategy (answers Q12.5: "how expensive is indexing?")**

| Layer | What | When | Cost on the NAS (~few hundred GB, ~5–10k Photos) |
|---|---|---|---|
| L0 folder index | Folder names + file names + sizes/mtimes | Background, whole root, on launch and on poll | Directory listings only. Seconds to ~1–2 min (dominated by SMB round-trips), near-zero bytes |
| L1 metadata | EXIF (date, camera, lens, exposure, GPS) | Background, whole root, low priority; **immediately for an opened folder** | ~64 KB read per file → under 1 GB total, a few minutes once, then incremental. Powers date view, map and search. |
| L2 thumbnails | **Pairs: the ARW's embedded JPEG preview** (small range read; M0 measured 4.7 s for a 95-photo day). HIF-only/edited: ImageIO thumbnail (reads the whole ~10 MB file, so it's bandwidth-bound). → 256 px + 1024 px local cache | **Lazily when a folder is opened**, nearest-first, plus prefetch of adjacent days | A 100-photo day: placeholders instantly, all thumbnails in a few seconds (reads ~0.5–1 MB/photo). Cached locally (~150 KB/photo → ~1.5 GB for 10k) |
| L3 AI features | CLIP embedding, sharpness/eyes, RAW-value signals | Background after L2, **only for opened or imported folders** unless enabled for the whole root | ~50–200 ms/photo on M2 Neural Engine |

- Settings: **index scope** = whole root vs. opened folders only (L1–L3), cache size cap, poll interval (default 60 s on SMB). The defaults suit the owner: L0+L1 whole root, L2/L3 on demand.
- The catalog (SQLite, local app-data dir) is keyed by **volume/share identity + relative path**. When the NAS or a drive is unreachable it shows **offline** with cached thumbnails, and mutations are disabled.

### 6.2 Viewer (Pixea-inspired)
- **P0** Space/double-click opens the Photo **in the same window**. Esc returns to the grid at the same position.
- **P0** ←/→ and trackpad swipe, with **prefetch** of the next 2 / previous 1 (full-size HIF decode is ~0.5 s, so prefetch makes navigation feel instant).
- **P0** **Full screen** with auto-hiding chrome.
- **P0** Zoom: fit `\`, 100% `0`/Z-click, +/−, scroll, Space-drag. Show the HIF first; a RAW-only Photo shows the embedded preview first, then the full develop.
- **P0** Rate `0–5`, pick `P` / reject `X`, labels `6–9`, **Keep-RAW toggle `K`**. All apply to the whole Photo.
- **P0** **Slideshow** (Enter): interval, fade/none, shuffle, loop, follows the current filter, includes sub-folders, Space to pause.
- **P1** Info overlay `I` (EXIF, histogram, files + sizes, Keep-RAW reasons, GPS mini-map). Clipping overlay `J`. **Focus mask** (highlights in-focus areas, from Capture One) for culling.
- **P1** Compare `C` (2-up, synced zoom) and Survey `N` (2–6 up) — the Lightroom pattern.
- **P1** HDR display of HLG HIF on the Mac's EDR screen.

### 6.3 Format support

| Format | Decode (Mac) | Decode (Windows) | Priority |
|---|---|---|---|
| **HIF/HEIF** (Sony 10-bit 4:2:2, HLG) | **Apple ImageIO** (hardware HEVC) — verified on the owner's sample | WIC/Media Foundation via the purchased HEVC extension; fallback libheif + libde265 | **P0** |
| ARW (A7C II verified in rawler), DNG, other RAW | rawler (RapidRAW); Apple RAW (`apple_raw.rs`) already available as an alternative developer | rawler | P0 |
| JPG/PNG/TIFF/WebP/JXL | RapidRAW `image` stack | same | P0 |
| MP4 / XAVC S / XAVC HS | AVFoundation | Media Foundation (tabling candidate) | P1 |

**Edited-output encoding:** HIF → **10-bit HEIF via ImageIO** on Mac (to verify in M0 that 10-bit is preserved; Apple's encoder writes 4:2:0, which is fine for a display copy). On **Windows**, HEIF encoding is a **tabling candidate** (§10.1): until solved, Windows edits of HIF Photos write a JPG and the HIF goes to Recently Deleted.

### 6.4 Editing ("edit from RAW, overwrite the display copy")
- **P0** `E` opens the RapidRAW editor on the **RAW** when present, otherwise on the display copy. The owner never has to find the RAW.
- **P0** Edits live in the recipe sidecar (`.rrdata`), so RAW + recipe is the master.
- **P0** **Done** renders full resolution and **overwrites the display copy in the same format and filename** (HIF → 10-bit HEIF, JPG → JPG q92), copying EXIF/GPS/XMP and adding an "edited" marker. The **camera original moves to Recently Deleted** (D3; restorable for 30 days). State → `EDITED`.
- **P0** **Re-edit** always reopens RAW + recipe, so there's no generational loss. **Revert** = delete the recipe and re-render (restores the camera original if it's still in Recently Deleted).
- **P0** **Photos without RAW**: the first edit stores the original as a hidden base `.originals/…`, and every render starts from that base (confirmed, Q12.2).
- **P1** Copy/paste recipe, batch apply, and export resized copies for sharing (separate from overwrite).

**Looks (camera styles) — P1, M3**
1. **"My Sony" (from your own files):** fit a per-camera base look (tone curve + 3D LUT) by comparing the owner's own ARW→HIF pairs. There are thousands of training pairs on the NAS, so the fitted look is exactly what the A7C II produces. All edits start from "looks like my camera". This is also the default base for style learning (§6.8).
2. **Sony Creative Looks** (ST, PT, NT, VV, VV2, FL, IN, SH, BW, SE): fitted the same way if the owner shoots a few test frames per Creative Look, or approximated.
3. **Classic looks library** (for personal use): **Fujifilm**-inspired (Provia, Velvia, Astia, Classic Chrome, Classic Neg, Nostalgic Neg, Eterna, Reala Ace, Acros), **Nikon** Picture Controls (Standard, Vivid, Flexible Color looks), **Canon** Picture Styles, **Leica**-style looks, plus RapidRAW's existing **Spektrafilm** film stocks.
   - Implementation: each look is a LUT + tone curve applied on top of the neutral camera-matched base, in RapidRAW's existing LUT pipeline, with intensity 0–100%.
   - Source: fit each look from public sample pairs (that brand's RAW + in-camera JPG of the same frame), then transfer to the Sony base. Hand-tune against reference images. Expect "very close", not bit-exact.
4. A look picker with live thumbnails (as in Pixea/Fuji X RAW Studio), and favorite looks.

### 6.5 Keep-RAW decision workflow (answers "casual vs. valuable")

The owner's question was how to tell whether a photo is casual (HIF is enough) or valuable (keep the RAW). The answer is a **staged process: AI triage first, human rating and confirmation after** (D11). The AI does the tedious first pass the moment a shoot arrives. The owner rates and confirms at their own pace.

> **Hard rule:** AI never moves a RAW (or any file) to Recently Deleted by itself. Every RAW drop and every delete requires an explicit human action on a review screen. AI output is stored **separately** from human ratings (XMP custom namespace `pairview:ai*`) and never overwrites a human rating, flag, or lock.

**What the HIF already gives you.** A 10-bit HIF holds about 4× the tonal steps of a JPG, so moderate edits (exposure ±1 EV, WB tweaks, looks) work fine on HIF alone. The RAW mainly matters when highlights are blown, shadows need big lifts, ISO is high (RAW denoise is far better), WB is badly off or mixed, or it's a "portfolio" shot you may re-edit for years.

**Stage 1 — AI triage (automatic, on arrival or first open).** It runs in the background when a new shoot lands (FTP, import) or an un-triaged folder is opened. The results appear as dimmed, clearly labelled "AI" overlays.
1. **Shoot intent guess:** Casual · Normal · Important, inferred from content and context (e.g. many indoor snapshots at home → Casual; golden-hour landscapes, wildlife at long focal lengths, or a dense event burst → Important). It shows as a chip in the folder header, and the owner can change it with one click.
   - *Casual* → default recommendation **HIF is enough** unless a photo has a strong keep signal.
   - *Important* → default **Keep RAW** for every non-reject.
   - *Normal* → per-photo signals decide.
2. **Burst grouping + best-of** (§6.7).
3. **Suggested rejects:** out of focus, eyes closed, misfires (lens cap, ground shots), near-duplicates that aren't the best of their group.
4. **AI score** (0–5 "AI ★", shown dimmed and separate from human stars) for sorting and to speed up the human pass.
5. **Per-photo RAW value:** Keep RAW / HIF is enough / Your call, with reason chips (table below).

**Stage 2 — Human pass (whenever the owner likes).** The owner goes through the shoot in AI-score order or the usual order. They rate (stars), pick or reject, accept or override the intent chip, and lock Keep-RAW (`K`) where they care. Human input always wins. Accepting AI suggestions is one key per photo or one button per group ("accept AI rejects in this burst"), but it's still a human action. Rejects confirmed here are deleted as whole Photos only via **Delete rejects** (§6.6).

**Stage 3 — RAW value signals (computed in Stage 1, refreshed after Stage 2).** Human ratings and flags from Stage 2 feed back in, so the recommendation sharpens after the human pass:

| Signal (computed locally) | Pushes toward | Example chip |
|---|---|---|
| Rating ≥ ★★★ or Pick flag | Keep | "★★★★" |
| Edited from RAW, or in a collection like "Portfolio"/"Print" | Keep | "Edited" |
| Highlight clipping in HIF (> ~0.5% pixels at max, outside specular) | Keep | "Highlights clipped 3%" |
| Deep shadows with detail (large area near black) | Keep | "Shadows crushed" |
| ISO ≥ 3200 | Keep | "ISO 6400" |
| Extreme/mixed WB (low CCT indoor, strong tint, mixed lighting) | Keep | "Tungsten / mixed light" |
| High-dynamic-range scene (backlit, sunset — wide histogram span) | Keep | "High contrast scene" |
| Shoot intent = Important | Keep | "Important shoot" |
| Burst duplicate, not the best of group | Drop | "Burst — not best" |
| Document/screen/receipt/menu content (CLIP) | Drop | "Document" |
| Unrated & not edited after the grace period | Drop | "Untouched 30 days" |
| Well-exposed, normal DR, base-ISO, shoot intent = Casual | Drop | "Casual, well exposed" |

- The owner can **lock** a decision (`K` cycles Auto → Keep 🔒 → Drop). Locked decisions are never changed by rules.
- Signals and weights are visible and adjustable in settings. Later (P2), the app learns the weights from the owner's own overrides, just like style learning.

**Stage 4 — Slim review (human confirmation, required).** It's reached two ways:
- **Automatically suggested** 30 days after capture, so there's time to edit first. A banner and a sidebar item, **Ready to slim**, list shoots past the grace period: *"2026-10-24 — 71 of 93 photos: HIF is enough · saves 2.8 GB"*.
- **Manually, any time:** **Review for slimming** on any folder, selection, date range, or smart collection (context menu, toolbar, or `⌘⇧K`). This ignores the grace period.

In both cases nothing happens until the owner presses **Apply** on the review screen.
- The review screen has three columns/tabs: **HIF is enough** (pre-selected) · **Your call** · **Keep RAW**. The owner can drag between them, and each photo shows its reason chips.
- Photos the owner hasn't rated or confirmed are flagged "AI-only suggestion" in the review, so it's obvious which drops rest solely on AI judgement.
- **Apply** → RAWs (+ their recipes) go to Recently Deleted. Retention is **30 days** (D12), with **Empty now** when space is needed immediately.
- Optional **Rebake before drop** (P1): for a Photo marked "HIF is enough" that has clipped highlights, render the RAW once with the camera look + highlight recovery and replace the HIF. The HIF-only copy then keeps the best version the RAW could give. Off by default, because it changes the look slightly.
- Safety: never drop a RAW whose recipe is newer than its rendered HIF (re-render first). Never drop on an offline volume. **Never drop without the review screen and an explicit Apply. There is no silent or AI-only auto-slimming, ever (D11).**

### 6.6 Delete & orphans (linked to §6.5)
- **P0** Delete on a `RAW+HIF` Photo opens one dialog with context-aware choices:
  - **Delete photo** (RAW + HIF + recipe + xmp) — the default
  - **Keep HIF only** (= drop RAW; same as a §6.5 "drop" decision applied now)
  - **Keep RAW only**
  - ☐ *Don't ask again — always do the selected action* (toggle in the dialog). The same default lives in **Settings → Deleting**, which can bring the dialog back (Q12.7).
- **P0** In **Cull** mode, `X` + "Delete rejects" removes whole Photos without the dialog (it's an explicit batch action with its own confirmation and count).
- **P0** Everything goes to **Recently Deleted** (`.recently-deleted/` at the root on the same volume/share — an instant move, including on SMB). Restore, 30-day auto-purge, **Empty now**. It is grouped by reason (Deleted · Slimmed RAWs · Replaced originals), so it's clear what each GB is.
- **P0** **Orphan detection**: a HIF deleted outside the app leaves `⚠ RAW` Photos → banner → review grid with **Delete RAWs** / **Keep as RAW-only** / **Ignore**. A deleted HIF is treated as a strong "drop" signal, so it is pre-selected in review. It is never auto-deleted.
- **P0** Operation log + **Undo** for every batch action until purge.

### 6.7 AI culling assist (P1)
- Burst/near-duplicate grouping (capture-time gaps + CLIP similarity) → **auto-stacks** (Lightroom-style).
- Per photo: subject/face **sharpness**, **eyes closed**, **face blur**, clipping. Suggests the **best of each group**.
- Cull view: large preview + group filmstrip, `P`/`X`/←/→, "keep best, reject rest", focus mask, 2-up compare.
- **Delete rejects** → whole Photos to Recently Deleted.
- Runs locally (CoreML on M2). RapidRAW already has `culling.rs`, CLIP tagging and face models to build on.

### 6.8 One-click AI edit → learns my style (P0 Auto, P1 learning)
- **P0** **Auto**: the "My Sony" look (once fitted) + generic auto-adjust (exposure, WB, highlights/shadows, contrast, vibrance, straighten). It only sets normal sliders, so everything stays editable.
- **Edit-history capture starts in M3**: every finished edit stores *(image features → final slider values + chosen look)* locally.
- **P1** At ≥ ~50 edits, train a small local regressor (GBDT/kNN on CLIP embeddings + tone stats + EXIF, then an MLP at ≥ 500). **Auto** becomes "how I would edit this", with a confidence value; low confidence falls back to generic auto.
- **P1** Named styles ("Travel", "Family indoor"), auto-chosen by scene. A **"Why?"** panel shows the 3 most similar past edits. **Batch Auto** for a whole shoot, with review before overwrite.
- **P2** Bootstrap from previously edited exports.
- All local; models and training data can be exported or reset.

### 6.9 SD-card import (P1)
- Detect a card (`DCIM/` present) and show **only new** Photos (dedupe by serial + capture time + basename).
- Destination default: `{Root}/{YYYY-MM-DD}_{cameraFolder}/`, the owner's current convention, created automatically. **No `arw/`/`hif/` split by default** (a setting can restore it).
- Merges both card slots if Sort Recording is used.
- Optional second copy to another drive, checksum-verified. The app never formats the card.
- Optional at import: set shoot intent (§6.5), apply a look. AI triage runs automatically afterwards.

#### 6.9.1 Camera FTP arrivals — P1 (detection P0)
The A7C II uploads to the NAS over FTP (into `…/A/DCIM/`), so shoots arrive without an import step.
- **P0** New files in a watched location are detected by the SMB poller (default 60 s, plus FSEvents when they work). Pairs are formed as soon as both files exist. A half-uploaded file (size still changing) is skipped until it's stable.
- **P1** **New arrivals** sidebar item and a "New" badge on the shoot. AI triage (§6.5 Stage 1) starts automatically. An optional macOS notification: *"2026-11-02: 84 new photos — triage done, 9 suggested rejects"*.
- **Confirmed:** FTP creates the same one-folder-per-day `NNNYMMDD` names as the card (e.g. `10561031` = counter 105, 2026-10-31), and future shoots are flat (D13). A new `NNNYMMDD` folder in a watched location = a new shoot.
- Folder names can repeat: the counter resets to 100 on every card format, and `Y` repeats every decade. So a folder name is never treated as an ID. Shoots are identified by path + capture-date range, and dates always come from EXIF.
- **P1 Auto-tidy (opt-in, replaces the owner's manual rename):** once a new FTP shoot has been **quiet for 30 minutes** (no new or growing files), rename `10561031` → `2026-10-31_10561031`, with the date taken from the photos' EXIF (the earliest `DateTimeOriginal`). It waits for quiet, because renaming a folder mid-upload would make the camera recreate `10561031` and split the shoot in two. If a late file still arrives in a re-created `10561031`, it's merged into the tidied folder on the next quiet period. Every auto-tidy is logged and can be undone. A manual **Tidy shoot** is still available for older folders.

### 6.10 Video (P1)
- Sony MP4/XAVC clips in grid and date views, viewer playback (AVFoundation on Mac), and in slideshows.
- On Mac, XAVC HS 4:2:2 10-bit playback must be verified on M2 in M0. On Windows it's a tabling candidate (Nvidia hardware decode of 4:2:2 HEVC only exists on RTX 50-series).

### 6.11 Search & map (P1)
- Metadata search (camera, lens, focal, ISO, date range, rating, state, Keep-RAW, folder). **Semantic search** with local CLIP ("sunset", "dog on beach"). **Map view** (MapLibre + OSM tiles, online only) for GPS-tagged Photos.

### 6.12 Library ideas borrowed from Lightroom / Capture One (answers Q12.6)

| Idea | Origin | In this app | Priority |
|---|---|---|---|
| **Smart Collections** (saved live filters) | LR | e.g. "★★★★+ last 90 days", "Ready to slim", "Edited this month", "Keep RAW but unrated". Built on RapidRAW's existing albums. | P1 |
| **Collections / albums** (manual, cross-folder) | LR/C1 | RapidRAW already has albums; make them Photo-aware (pairs move together) | P0 |
| **Quick Collection / Selects** (temporary basket, `B`) | LR / C1 Selects | Gather photos to share or export, then clear | P1 |
| **Library filter bar & metadata columns** (Camera → Lens → Date) | LR | Click-through browsing of EXIF facets | P1 |
| **Auto-stack by capture time** | LR | Bursts as one tile (§6.7) | P1 |
| **Virtual copies** | LR/C1 | Exists in RapidRAW. Extra versions render as `DSC03545-2.HIF` siblings inside the same Photo. | P1 |
| **Compare / Survey view** | LR | §6.2 | P1 |
| **Focus mask** | C1 | §6.2/§6.7 | P1 |
| **Paste previous / Sync settings** | LR | `⌘⇧V` pastes the last edit onto the selection | P1 |
| **Smart Previews** (edit/rate while the source is offline) | LR | Locally cached 2560 px proxies, so the owner can rate, cull, and even edit **away from home without the NAS**. Changes sync back when the NAS is reachable. | P2 |
| **Sessions** (Capture / Selects / Output / Trash per shoot) | C1 | Our shoot = folder model already matches; Recently Deleted ≈ session trash | — |
| **Metadata presets** (copyright, author) on import | LR/C1 | Applied at import, written to XMP | P2 |
| **Batch rename with tokens** | LR/C1 | Pair-safe rename (both files + sidecars) | P1 |
| **Keyword hierarchy + AI auto-keywords** | LR | RapidRAW CLIP tagging → keywords in XMP | P2 |
| **Delete Rejected Photos** command | LR | §6.6 | P0 |

### 6.13 Metadata
- **P0** Ratings, labels, flags, keywords, and Keep-RAW lock are written to **XMP sidecars** (custom namespace for Keep-RAW) and read back. Never written into RAW files.

---

## 7. UX principles
1. **Pixea feel:** one window, content-first, disappearing chrome, everything on the keyboard, instant.
2. **The Photo, not the file:** never two tiles for one shot. Files are visible in Info.
3. **No surprise deletes:** Recently Deleted, review screens, undo, no silent automation.
4. **Explain, don't decide:** Keep-RAW and AI suggestions always show their reasons.
5. **The library stays usable without the app:** plain folders, standard XMP, standard HIF/JPG.

Screens: **Library** (sidebar: Folders · Dates · Collections/Smart · Map · Ready to slim · Recently Deleted), **Viewer**, **Editor** (RapidRAW, `E`), **Cull**, **Slim review**, **Import**, **Settings**.

---

## 8. Architecture

```
Tauri 2 shell (fork of RapidRAW — React + TS + zustand / Rust + wgpu + ONNX Runtime)
├── existing RapidRAW: editor UI, rawler + apple_raw decode, WGSL pipeline, LUTs, masks,
│   culling.rs, tagging (CLIP), albums, folder tree, thumbnails, .rrdata sidecars, viewer_window
└── NEW (own modules, minimal upstream edits)
    ├── library/   SQLite catalog, L0–L3 indexer, SMB poller, volume identity, offline state
    ├── assets/    shoot/pairing engine, Photo state machine, lifecycle ops, op-log/undo, Recently Deleted
    ├── keepraw/   signal extraction, scoring, shoot intent, slim review
    ├── heif/      macOS ImageIO decode/encode bridge | Windows WIC (HEVC ext) / libheif
    ├── looks/     camera-look fitting from ARW→HIF pairs, LUT library
    ├── xmp/       Adobe-compatible sidecars + custom namespace
    ├── style/     edit-history store, regressor training/inference
    ├── ingest/    card detection, copy/verify, naming
    └── ui/        Library grid (Photo-based), date view, Pixea-style viewer, slideshow, slim review
```

### 8.1 Staying mergeable with upstream RapidRAW
- The repo is built **on top of upstream RapidRAW history** (re-based 2026-11-01). `upstream` = `CyberTimon/RapidRAW`. `origin` = the owner's fork [`DanielXia-GHD/Pairview`](https://github.com/DanielXia-GHD/Pairview) (renamed from `PhotoVeiwerLibrary`), whose `main` was identical to upstream `main` (`d1a9e2a5`) when connected.
- New code goes in **new files/modules**. Upstream files only get small, well-marked hook points (`// PV:` comments) such as registering new Tauri commands, adding HEIF to `formats.rs`, and routing the main view.
- Merge `upstream/main` **every 2–4 weeks** or when a camera/feature fix lands. CI builds after each merge.
- `README.md` is ours and is marked `merge=ours` in `.gitattributes`, so upstream README edits never conflict. Upstream credit and build instructions are kept inside it.
- Branding (app name, bundle id `io.github.CyberTimon.RapidRAW` → our own id, icons) is changed in config only, so it can live alongside an installed official RapidRAW.

**Packaging:** Tauri bundler → `.dmg` (aarch64, ad-hoc signed; first launch via right-click → Open) and NSIS `.exe` (x64; SmartScreen "Run anyway"), built on GitHub Actions. AI via ONNX Runtime: CoreML on M2, DirectML on Windows.

---

## 9. Non-functional requirements

| Area | Target (M2 Mac on Wi-Fi 6 → NAS, ~77 MB/s) |
|---|---|
| Open a 100-photo day on the NAS (cold) | Grid placeholders < 0.5 s; all thumbnails < 5 s; warm (cached) < 0.5 s |
| Viewer next/prev | < 100 ms with prefetch; cold single HIF ≤ 0.6 s |
| Full RAW develop for zoom/editor | ≤ 2 s on M2 (GPU pipeline), excluding NAS transfer (~0.5 s) |
| L1 metadata index, whole NAS root | ≤ 5 min first run; incremental thereafter |
| Memory (library/viewer) | < 1.5 GB |
| Safety | No permanent delete outside Recently Deleted purge; moves are same-volume renames |
| Offline | All but map tiles; NAS-offline shows cached thumbnails (Smart Previews in P2) |

---

## 10. Milestones (Mac first)

| Milestone | Scope | Exit criteria |
|---|---|---|
| **M0 — Mac spikes** (in progress, see [M0-findings](M0-findings.md)) | Fork builds and runs on M2 (`npm start`). **HIF decode in RapidRAW's loader via ImageIO**. **10-bit HEIF encode** via ImageIO (verify 10-bit, EXIF copy). Embedded-preview extraction speed for ARW + HIF on the NAS. Pairing over `arw/`+`hif/`. XAVC HS playback check. Unsigned `.dmg` from CI. | The owner's 2026-10-24 shoot opens, pairs, and thumbnails in < 5 s from the NAS; a HIF round-trips decode → edit → 10-bit HEIF write |
| **M1 — Library + Viewer** | **Library Locations** manager (add/relocate/exclude/status), new-file detection, L0–L2 indexing, folder tree + shoots, date view, Photo grid + badges, viewer, full screen, ←/→, zoom, slideshow, ratings → XMP, albums Photo-aware | The owner replaces Pixea for daily browsing |
| **M2 — Lifecycle + Keep-RAW** | Delete dialog + defaults, Recently Deleted, orphans, op-log/undo, shoot intent chip, heuristic RAW-value signals (clipping, ISO, WB, rating), Ready-to-slim + **manual Review for slimming**, offline handling | RAW and HIF are never out of sync again; a shoot is slimmed in < 2 min |
| **M3 — Edit loop + Looks** | `E` on RAW, overwrite HIF/JPG + original to Recently Deleted, base rule for no-RAW, revert, generic Auto, **"My Sony" look fitting**, looks library v1, **edit-history capture** | Edit a shot end-to-end in < 30 s; default renders match the camera HIF closely |
| **M4 — AI triage + Import** | Full **AI triage** on arrival (intent guess, bursts, suggested rejects, AI ★, RAW value), human-pass UI (accept or override), FTP New arrivals, SD import, compare/survey, focus mask | An FTP-uploaded shoot is triaged before the owner opens it; the human pass takes minutes, not an hour |
| **M5 — Find + Collections** | Smart collections, quick collection, filter bar, metadata + semantic search, map, video, pair-safe batch rename | |
| **M6 — Style** | Style training, Auto = my style, named styles, "Why?", batch auto, rebake-before-drop, learned Keep-RAW weights | Auto edits accepted unchanged ≥ 50% |
| **W — Windows** | `.exe` build + parity pass after M3, then per milestone; tabled items stay tabled | Core library/viewer/lifecycle/edit loop work on Windows |

### 10.1 Windows tabling rule
Any feature where the Windows path is meaningfully harder or more expensive than on the Mac is **raised to the owner and tabled** rather than built by default. Current candidates:

| Feature | Mac path | Windows issue | Proposal |
|---|---|---|---|
| 10-bit HEIF **encode** (overwrite HIF) | ImageIO | No built-in encoder; x265 is GPL/heavy, kvazaar is slower | **Table.** Windows writes JPG for edited HIF Photos. |
| HIF **decode** | ImageIO | WIC + purchased HEVC extension *should* work; Sony 4:2:2 untested | Try WIC first (cheap); libheif fallback only if needed |
| XAVC HS 4:2:2 video playback | AVFoundation | No hardware decode before RTX 50; software decode is heavy | **Table** (show thumbnail, open in an external player) |
| Smart Previews / offline | — | — | P2 on both |
| Camera-look fitting / style training | CoreML | DirectML works but needs separate tuning | Run fitting on the Mac; looks and models are files that work on both |

---

## 11. Risks

| Risk | L | I | Mitigation |
|---|---|---|---|
| Upstream RapidRAW moves fast → merge conflicts | High | Med | New modules, `// PV:` hook points, merges every 2–4 weeks, CI after merge |
| ImageIO 10-bit HEIF encode drops to 8-bit or loses metadata | Med | Med | M0 spike; fall back to libheif + x265 on Mac (GPL is fine for personal use) |
| SMB latency makes large folders feel slow | Med | Med | Lazy L2, local cache, prefetch, never read full files while browsing |
| Keep-RAW recommendation drops a RAW the owner later wants | Med | High | Review screen required; 30-day Recently Deleted; locks; conservative defaults; Important-shoot override |
| Fitted "looks" don't match brand rendering closely | Med | Low | Labeled "-inspired", intensity slider, owner-tunable |
| Wrong pair → wrong RAW deleted | Low | High | Shoot scope + timestamp guard + mismatch badge + Recently Deleted |
| RAW develop ≠ camera HIF | High (confirmed) | Med | "My Sony" look as default base (M3) |

---

## 12. Open questions

**Resolved**
- v0.2: HEIF output, hidden base for no-RAW Photos, camera original to Recently Deleted, A7C II, indexing cost, LR/C1 ideas, delete prompt + toggle + setting, no phone photos.
- v0.3:
  - Fork URL → `origin` (§8.1).
  - `A/` means nothing (camera FTP path), and any folder can be added (§6.1.1 Library Locations).
  - Retention is 30 days, plus manual slim review (D12).
  - AI triage first, human confirmation after, and AI never drops a RAW (D11, §6.5).
  - Existing subfolders stay, future shoots are flat (D13).
  - More samples later.
  - Name: **Pairview** (D14).
- v0.3.1: repo renamed to `Pairview`; FTP folders use `NNNYMMDD`, one per day (§6.9.1).

**Remaining (doesn't block M0)**
- **More samples (later):** an HLG HIF, an ARW+JPG pair, a high-ISO indoor shot, a burst, a backlit/clipped shot, an XAVC HS clip, and a few frames in different Creative Looks.

## 13. Name

**Pairview** (chosen in v0.3): the RAW and the HIF, viewed as one. Bundle id `com.danielxia.pairview`. App name, window title, and icon change in M0. Branding lives in config only (`tauri.conf.json`, `package.json`), so it doesn't collide with an installed official RapidRAW. Other candidates that were considered: Keeper, Negprint, Contact Sheet, Duet, Lumen Library, Shoebox.

## 14. Research notes
*(from v0.1, still valid)*

- "Cull on JPG, then drop matching RAWs" is the most common manual workflow ([Capture One community](https://support.captureone.com/hc/es/community/posts/8371248557597-RAW-JPG-workflow-culling), [Photo Workflow](https://apps.apple.com/app/id6466960429)). → The Photo is the unit.
- Pair-aware deletes are fragile in other tools ([pixls.us](https://discuss.pixls.us/t/culling-raw-and-matching-jpg/15769), [Adobe community](https://community.adobe.com/questions-675/what-to-do-with-jpg-raw-files-964221)). → Explicit dialog + Recently Deleted.
- Orphan-RAW cleanup scripts ([pixls.us](https://discuss.pixls.us/t/culling-photos-fast/15119/19)). → Built-in orphan review.
- RAW+HEIF is badly supported elsewhere ([Adobe request](https://community.adobe.com/t5/lightroom-classic-ideas/only-import-raw-files-when-raw-heif-both-exists/idi-p/14140133), [Capture One 10-bit](https://support.captureone.com/hc/en-us/articles/360016552258/comments/11624055277341)).
- Sony Sort Recording splits formats across slots ([Sony help](https://helpguide.sony.net/ilc/2440/v1/en/contents/201h_file_format.html)).
- AI culling patterns: bursts, best-of, focus, eyes ([overview, vendor-written](https://imagen-ai.com/post/best-ai-photo-culling-software/)).
- libheif: LGPL, libde265/FFmpeg decoders, Rust bindings ([libheif](https://github.com/strukturag/libheif)).
- RapidRAW: AGPL-3.0, rawler, wgpu, ONNX (CoreML/DirectML), albums, culling, CLIP tagging; no HEIF ([RapidRAW](https://github.com/CyberTimon/RapidRAW)). rawler includes `a7cm2.toml` ([RapidRAW-DngLab](https://github.com/CyberTimon/RapidRAW-DngLab)).
- Pixea UX: same-window viewer, keyboard-first, theatre mode/slideshow ([Pixea](https://www.imagetasks.com/pixea/)).
