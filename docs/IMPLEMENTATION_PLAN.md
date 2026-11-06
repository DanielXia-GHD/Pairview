# Pairview — Implementation Plan

| | |
|---|---|
| Status | v1, 2026-11-01. Covers M0 wrap-up and M1 in detail, M2–M3 at task level, M4–M6 as outlines |
| Inputs | [PRD v0.3.1](PRD.md), [M0 findings](M0-findings.md) |
| Rule | **The PRD says *what*; this plan says *how* and *in what order*.** Update this file when a task finishes or a decision changes. |

---

## 1. How to use this plan

Each task has:

- **ID**, e.g. `M1-04`. Reference it in branch names and commit messages (`M1-04: catalog schema`).
- **Tier**: which model should do it (§2).
- **Depends on**: tasks that must be merged first.
- **Touches**: files to create (new) or edit (upstream files, which always get `// PV:` hooks only).
- **Done when**: the acceptance checks. A task is not done until they pass.

**Status markers:** `[ ]` todo · `[~]` in progress · `[x]` done · `[-]` dropped.

A fresh session (or a delegated agent) should be able to pick up any `[ ]` task whose dependencies are `[x]`, reading only this file, the PRD sections it cites, and the files it touches.

## 2. Model tiers and review gate

| Tier | Model | Use for |
|---|---|---|
| **O** | Opus 5.5 | Architecture decisions, schemas, `unsafe`/FFI, anything that **moves, renames, or deletes user files**, the keep-RAW logic, cross-cutting refactors, reviews |
| **S** | Sonnet 5.5 | Well-specified features with clear acceptance tests: UI views, commands, XMP, search, collections |
| **H** | Haiku 5.5 | Mechanical work: branding/renames, UI text + translation keys, registering commands, boilerplate, fixtures, docs |

**Review gate (mandatory):**
1. Every task: `cargo test`, `cargo clippy` clean for `src/pv/`, `cargo fmt`, and **no new** `npm run typecheck` / `npm run lint` errors in touched files (upstream baseline: 65 `tsc` errors).
2. Any task tagged 🛡 (it touches user files on disk) gets an **Opus review of the diff** before merge, regardless of who wrote it, plus the safety test suite (§4.3).
3. Tasks done by H or S that fail review twice get bumped up a tier.

## 3. Architecture decisions for implementation

These refine PRD §8 and are binding unless changed here.

| # | Decision | Why |
|---|---|---|
| A1 | **New Rust code lives in one module tree inside the existing crate: `src-tauri/src/pv/`** (`pv/catalog`, `pv/scan`, `pv/assets`, `pv/lifecycle`, `pv/xmp`, …), not separate workspace crates. `heif.rs` moves to `pv/heif.rs` in M0-05. | Separate crates would mean restructuring upstream's `Cargo.toml` into a workspace, which guarantees merge conflicts. One `mod pv;` hook is enough. |
| A2 | **New frontend code lives in `src/pv/`** (components, hooks, zustand stores). Upstream components get small hook edits only. | Same reason as A1. |
| A3 | **Catalog = SQLite** via `rusqlite` (`bundled` feature), one DB file in the app-data dir. It is a **cache**: deleting it loses nothing except AI results and the op-log (both are also mirrored to XMP or log files). | Folders + sidecars stay the source of truth (PRD §7.5). |
| A4 | **Photo representative path.** RapidRAW's grid, thumbnails, and editor are file-based. Pairview gives each Photo a **representative path** = the RAW if present, otherwise the display copy. The grid lists representative paths only; badges and members come from `pv`. | Reuses all of RapidRAW's grid and editor machinery. The editor opens the RAW by default (PRD §6.4). Thumbnails use the **ARW embedded preview**, which M0 measured as the fast path (4.7 s per day). |
| A5 | **The viewer shows the display copy** (HIF/JPG) for a Photo, not the representative RAW. | Faster (HIF ~0.5 s vs RAW develop) and it's what the camera intended. Edited Photos show the rendered HIF. |
| A6 | **Ratings/flags/labels are Photo-level.** Write path: `pv/xmp` sidecar next to the RAW (or the display copy if there's no RAW) **and** RapidRAW's `.rrdata` for every member, so the editor and upstream features see the same values. Read path: XMP wins. | Interop with other tools (PRD §6.13) without breaking RapidRAW's own metadata. RapidRAW's existing `enable_xmp_sync` setting is reused where possible. |
| A7 | **All file mutations go through `pv/lifecycle`**: deletes, RAW drops, overwrites, renames, tidy. Upstream delete commands (`delete_files_from_disk`, `delete_files_with_associated`, both in `file_management.rs`, currently using the OS trash and same-folder stem matching) get a `// PV:` hook that routes to lifecycle (Recently Deleted + cross-subfolder pairing + guard). | One audited path for anything that can lose data. |
| A8 | **Recently Deleted** = `<location root>/.pairview-deleted/<op-id>/<original relative path>`, plus `manifest.json` per operation. A same-volume `rename()` only; **refuse** if source and target are on different volumes. | Instant on SMB, restorable, and the manifest makes undo and purge exact. |
| A9 | **Indexing is event-driven, in background workers** (tokio tasks + rayon for CPU), with priorities: opened folder > visible thumbnails > neighbors > whole-location L1. | Matches PRD §6.1 (L0–L3) and the M0 measurements. |

## 4. Cross-cutting engineering

### 4.1 Dev environment
- Rust: `export PATH=/opt/homebrew/opt/rustup/bin:$PATH` (toolchain pinned by `src-tauri/rust-toolchain.toml`).
- Run: `npm start`. Release: `npm run tauri build -- --bundles dmg`.
- Disk: `target/` grows to ~13 GB (debug + release). Clean with `cargo clean` when space is low.

### 4.2 Test fixtures
- **Synthetic fixtures (committed):** `src-tauri/src/pv/testdata/` holds tiny generated JPG/HEIF files with set EXIF dates, plus zero-byte stand-ins for structure-only tests. A builder function creates shoot layouts in a temp dir (flat, `arw/`+`hif/`, mismatched timestamps, orphans).
- **Real fixtures (not committed):** env var `PV_FIXTURES=/path/to/copy` pointing at a **local copy** of a few real shoots (never the live NAS). Tests that need real files skip when it's unset.
- Rule: **no test ever writes to `/Volumes/…`**.

### 4.3 Safety test suite (🛡 tasks must pass it)
1. Delete/drop moves files into Recently Deleted with a correct manifest. Restore puts every file back byte-identical.
2. Pairs whose timestamps mismatch are never treated as a pair by destructive ops.
3. Cross-volume moves are refused.
4. Offline location → every mutation is refused.
5. Crash in the middle of a multi-file operation (simulated) → the op-log replay leaves no half-state (either all moved or none).
6. Purge deletes only paths listed in manifests older than retention.

### 4.4 Upstream merge routine (every 2–4 weeks)
1. `git fetch upstream && git checkout -b merge/upstream-YYYYMMDD && git merge upstream/main`
2. Resolve conflicts (expected only around `// PV:` hooks).
3. Full test suite + manual smoke test (open a NAS shoot, view, edit, delete-restore).
4. Merge to `main`. Tier **S**, or **O** if conflicts touch lifecycle hooks.

---

## 5. M0 wrap-up

| ID | Task | Tier | Depends | Touches | Done when |
|---|---|---|---|---|---|
| `[x] M0-01` | HIF decode/thumbnail via ImageIO | O | — | `heif.rs`, hooks | Test passes on portrait + landscape samples (done) |
| `[x] M0-02` | **UI check of HIF in the running app** | S | M0-01 | — | `npm start`: open `2026-10-24_10361024`; HIF thumbnails appear; a HIF opens in the editor with correct orientation and colors. Findings recorded in M0-findings |
| `[x] M0-03` | **Pairview branding** | H | — | `src-tauri/tauri.conf.json` (`productName`, `identifier` → `com.danielxia.pairview`, window title), `package.json` `name`, icons (placeholder ok) | `.dmg` is named `Pairview_*.dmg`; it installs next to an official RapidRAW without clashing (different bundle id and app-data dir) |
| `[~] M0-04` | **Slim CI** (workflow pushed; upstream CI Build ×2, Lint and Release disabled in the fork's Actions settings on 2026-11-03; waiting on the first manual run) `pairview-ci.yml` (macOS arm64 `.dmg` on `workflow_dispatch` + tags; Windows `.exe` later). Keep upstream workflows but don't run them on push to our fork (via a repository variable check or disabling them in fork settings; **owner decides**) | S | M0-03 | `.github/workflows/pairview-ci.yml` | A manual run produces a downloadable `.dmg` artifact |
| `[x] M0-05` | Create `src-tauri/src/pv/` (A1), move `heif.rs` to `pv/heif.rs`, add `rusqlite`, set up the clippy config for `pv` | H | — | `lib.rs` hook, `pv/mod.rs`, `Cargo.toml` | Builds; `heif` test still passes |
| `[x] M0-06` | 10-bit HEIF **encode** in Rust (port of the Swift spike) | O | M0-05 | `pv/heif.rs` | Round-trip test: decode → encode → readback depth 10, EXIF date and model preserved |

---

## 6. M1 — Library + Viewer (detailed)

**Goal:** The owner replaces Pixea for daily browsing of the NAS: Library Locations, folder tree with shoots, date view, a grid of Photos with badges, viewer, full screen, slideshow, and ratings written to XMP.

### 6.1 Backend

| ID | Task | Tier | Depends | Touches | Done when |
|---|---|---|---|---|---|
| `[x] M1-01` | **Catalog schema + migrations**: `locations`, `folders`, `files` (path, size, mtime, kind), `photos`, `photo_members`, `exif` (date, subsec, serial, model, lens, orientation, gps), `metadata` (rating, flag, label, keep_raw), `ops_log`; schema version table | O | M0-05 | `pv/catalog/{mod,schema,migrate}.rs` | Unit tests: create, migrate from v0, foreign keys enforced, open/close in < 50 ms |
| `[x] M1-02` | **Location identity**: volume UUID / SMB server+share + relative path; online/offline detection; relocation API (done; SMB id parsing is tested on sample mount strings only, so check it against the real NAS mount in M1-10) | O | M1-01 | `pv/locations.rs` | Tests: same folder at a new mount path is matched (fixture with two roots); offline root reports offline without hanging (timeout ≤ 2 s on unreachable SMB) |
| `[x] M1-03` | **L0 scanner**: walks a location, records folders and files (listing only, no file reads), honors excludes (`@eaDir`, `#recycle`, `.pairview-deleted`, dotfiles), incremental by mtime (done; measured on the 1 local shoot only (1.5 ms), the 4-shoot NAS timing is still to do) | S | M1-01, M1-02 | `pv/scan/l0.rs` | 4 real shoots from `PV_FIXTURES` scanned in < 2 s; re-scan with no changes does zero DB writes |
| `[x] M1-04` | **Shoot + pairing engine** (PRD §5.1–5.3): shoot = folder + format subfolders; basename pairing; `NNNYMMDD` and `YYYY-MM-DD_` date-hint parser; Photo state derivation; representative path (A4) (done; local fixture 4/4 pairs, the 234/234 NAS re-check is still to do. The guard splits pairs once M1-05 fills `exif`) | O | M1-03 | `pv/assets/{shoot,pairing,state}.rs` | Synthetic fixtures: flat, split, orphan, RAW-only, display-only, RAW+HIF+JPG, case-mixed names → correct Photos/states. Real fixtures: 234/234 pairs |
| `[x] M1-05` | **L1 EXIF worker** (done: reads only the first 256 KB of each file; HIF EXIF is found through the HEIF `iloc` index and parsed by kamadak, with ImageIO as a whole-file fallback; NAS timing still to check): reads EXIF for Photos (one member per Photo first, the second later for the guard), 8-way parallel, priority queue; writes `exif`; runs the **timestamp guard** and flags mismatches | S | M1-04 | `pv/scan/l1.rs` (reuse `exif_processing`) | Opened 95-photo shoot gets dates in < 3 s; mismatch fixture flagged; guard result cached |
| `[x] M1-06` | **Watcher / poller** (done; files held as still arriving are re-checked after 2 s instead of waiting for the next poll; wiring to the app happens in M1-09): FSEvents where reliable, else poll (default 60 s); stable-size check for in-progress FTP uploads; emits `pv://folder-changed` events | S | M1-03 | `pv/scan/watch.rs` | Copying files into a watched fixture folder shows up within the poll interval; a growing file is ignored until stable for 2 polls |
| `[x] M1-07` | **Thumbnail source per Photo** (done: `pv::thumbs::thumbnail_file` picks the file, and upstream's thumbnail path and cache already use the fast ARW preview (64 KB + preview range) and the M0 HIF hook; local preview 10–15 ms; the NAS 95-photo timing is checked in M1-12 when the grid uses it) (M0 finding): unedited pair → ARW embedded preview; HIF-only/edited → `heif_thumbnail`; local cache keyed by Photo + member mtime | S | M1-04 | hook in `file_management.rs` thumbnail path; `pv/thumbs.rs` | 95-photo uncached day: all thumbnails < 5 s on the NAS; cached revisit < 0.5 s |
| `[x] M1-08` | **Photo-level metadata** (A6) (done: XMP edits touch only the changed properties; images byte-identical in tests; offline and unreadable sidecars refused; reviewed at Opus/High; the in-editor check comes with M1-09/M1-14): get/set rating, flag, label; XMP sidecar read/write (Adobe namespaces + `pairview:` namespace); mirror into `.rrdata` of every member | O 🛡 | M1-04 | `pv/xmp.rs`, `pv/metadata.rs` | Round-trip with Adobe-written XMP sample; RAW file bytes never change (checksum test); RapidRAW shows the same rating in its editor |
| `[x] M1-09` | **Tauri commands** for the frontend (done: one `pv_set_metadata` instead of three setters, `pv_list_photos` takes a folder or date-range scope, plus `pv_locations_rescan`; queries and the command layer are tested, and the app starts with the hook): `pv_locations_*`, `pv_folder_tree`, `pv_list_photos(folder, recursive, sort, filter)`, `pv_dates(year/month/day counts)`, `pv_photo_detail`, `pv_set_rating/flag/label` | S | M1-04, M1-05, M1-08 | `pv/commands.rs`, `lib.rs` hook (`generate_handler!`) | Typed TS bindings in `src/pv/api.ts`; each command has a Rust test or integration test |

### 6.2 Frontend

| ID | Task | Tier | Depends | Touches | Done when |
|---|---|---|---|---|---|
| `[x] M1-10` | **Library Locations manager** (PRD §6.1.1) (done: Settings → Library; owner checked add, offline after eject, Connect, and the buttons): add/remove/rename/reorder, status, Connect (`smb://`), relocate flow, per-location excludes and poll interval | S | M1-02, M1-09 | `src/pv/locations/*`, Settings hook in `SettingsPanel.tsx` | Add the NAS root; unplug the network → offline state; relocate test path works |
| `[x] M1-11` | **Folder tree with shoots** (done: Library section in the folder panel; folders in a location list one entry per Photo through a hook in `handleSelectSubfolder`, so the upstream grid shows Photos and upstream's Recursive mode works): a shoot with `arw/`+`hif/` shows as one folder; counts in Photos; recursive toggle | S | M1-09 | `src/pv/tree/*`, hook in `panel/right/FolderTree.tsx` or replacing it via a route | Owner's DCIM tree renders; counts match the pairing results |
| `[x] M1-12` | **Photo grid** (done: one tile per Photo since M1-11; badge pill on tiles; Photo-type and Keep-RAW filters in upstream's filter menu; upstream's stars/flags/labels and S/M/L sizes kept. Still open: badge in the list view, auto-refresh on pv://folder-changed, and the NAS 95-photo timing): one tile per Photo (representative path), badges (`RAW+HIF`, `RAW+JPG`, `HIF`, `RAW`, `⚠ RAW`, mismatch), stars/flag/label, S/M/L sizes, sort + filter bar | S | M1-07, M1-09 | `src/pv/grid/*`, hook in `MainLibrary.tsx` / `useSortedLibrary.ts` | 95-photo shoot shows 95 tiles (not 190); filters by state work |
| `[x] M1-13` | **Date view** (done: Dates section with Year → Month → Day counts and jump-to-date; `Dates: …` virtual paths listed by capture date; grid grouped by day; uncertain days marked): Year → Month → Day sidebar with counts, day-grouped grid, jump-to-date | S | M1-05, M1-12 | `src/pv/dates/*` | All shoots appear under the right dates; a folder-name/EXIF disagreement shows "date uncertain" |
| `[x] M1-14` | **Viewer** (PRD §6.2) (done: overlay in the main window; Space/double-click opens a Photo's display copy via the `pvimg://` scheme with a JPEG fallback; ←/→/swipe in grid order with prefetch; click/scroll/pinch zoom and pan; F full screen; 0–5, P/X/U, 6–9, K, E; Esc returns to the grid on the last Photo. Owner checked it on the Desktop shoot. Still open: full develop for RAW-only Photos): same window, display copy (A5), ←/→ in grid order, prefetch next 2 / previous 1, zoom fit/100%/scroll/pan, full screen with auto-hide chrome, rating/flag/label keys, Esc back to the grid position | S | M1-12 | `src/pv/viewer/*`, `useKeyboardShortcuts.ts` hook | ←/→ < 100 ms after prefetch; portrait orientation correct; keyboard map matches PRD |
| `[x] M1-15` | **Slideshow**: Enter to start; interval, fade, shuffle, loop; follows current filter incl. sub-folders; Space pauses | S | M1-14 | `src/pv/viewer/slideshow.ts`, `PvViewer.tsx` | Runs over a filtered set (e.g. ★★★+) across sub-folders. Done: lives in the viewer (Enter / ▶); sub-folders via recursive `pv_list_images_for_path` run through upstream `computeSortedLibrary`; options persist in localStorage |
| `[x] M1-16` | **Info overlay** `I`: EXIF, files + sizes, histogram (reuse RapidRAW's) | H | M1-14 | `src/pv/viewer/info.tsx`, `pv/viewer.rs` | Shows both member files with sizes. Done: histogram via upstream `calculate_histogram` on a 512 px rendering (`pv_photo_histogram`; upstream `HistogramData` fields made `pub(crate)`), drawn like the editor's; open state remembered |
| `[x] M1-17` | **Translation keys + UI text** for all M1 strings (English; other locales fall back to English) | H | M1-10..16 | `src/pv/i18n/*` | No hard-coded UI strings in `src/pv` (lint rule or grep check). Done: `npm run pv:i18n-check` (`src/pv/i18n/check.mjs`, TypeScript AST: JSX text, visible attributes, and missing `pv.*` keys); upstream's `i18next/no-literal-string` reports nothing under this ESLint version. Decision (2026-11-04, owner): English only. No other locales are written or maintained; Pairview text stays in keys only to keep it in one place, and backend messages / non-English branding are not worked on |

### 6.3 M1 exit
- `[x] M1-18` **Exit check (O):** owner-style walkthrough on the real NAS: add location → browse tree → date view → view/rate 50 photos → slideshow → ratings visible in XMP and in RapidRAW's editor. Performance numbers recorded against PRD §9. Fix list triaged. Done (2026-11-04): results in `docs/M1-exit.md`. Owner walkthrough on a single NAS shoot location; the whole DCIM root was scanned, paired and indexed by `nas_check.rs` (headless), not added in the app. L1 first run 15 min accepted (NAS per-file latency).

**M1 critical path:** M0-05 → M1-01 → M1-04 → M1-09 → M1-12 → M1-14. Parallelizable: M1-02/03 (after 01), M1-06, M1-07, M1-08, and M1-10/11 (after 09).

---

## 7. M2 — Lifecycle + Keep-RAW (task level)

| ID | Task | Tier | Depends |
|---|---|---|---|
| `[x] M2-01` | `pv/lifecycle` core: op-log (write-ahead), Recently Deleted layout + manifest (A8), restore, purge (30 days), same-volume check, offline refusal 🛡. Done: `pv/lifecycle/{mod,plan,manifest}.rs`; Photo-level plan (Delete photo / Keep HIF only / Keep RAW only, lock and guard-split aware, Photo XMP copied next to the kept copy); rename-only moves; crash → rollback replay at startup and before each operation; one operation at a time; purge only manifest-listed files. Safety suite §4.3 1–6 in `lifecycle/tests.rs`, plus a `PV_FIXTURES` round trip on a copy of a real shoot. Self-reviewed at Opus/High | O | M1-04 |
| `[x] M2-02` | Route upstream delete commands through lifecycle (A7 hook) 🛡. Done: `// PV:` hooks in `delete_files_from_disk`, `delete_files_with_associated` and `delete_folder` call `pv/lifecycle/upstream.rs`; paths inside a location go to Recently Deleted, the rest to upstream unchanged. Exact files map to the Photo choice they amount to (RAW alone → Keep HIF only, lock honoured; display alone → Keep RAW only; both → Delete photo); with-associated = whole Photo across `arw/`+`hif/`; a virtual copy = its sidecar only; a folder = every file under it (location root and Recently Deleted refused). Renames/moves stay upstream for now (no data loss; the watcher re-pairs; pair-safe rename is M5) | O | M2-01 |
| `[x] M2-03` | Delete dialog (Delete photo / Keep HIF only / Keep RAW only + "don't ask again") and global setting (PRD §6.6). Done: `src/pv/delete/` (dialog with sizes from `pv_delete_preview`, keys 1–3/Enter/Esc; Settings → Library → Deleting); one `// PV:` hook in upstream `executeDelete` routes every delete of Pairview Photos (Delete key, context menu, culling) through it; with-associated and Delete rejects = whole Photos without the dialog; the choice applies to RAW+display pairs, other Photos go whole, locked Photos are kept. Owner checked 2026-11-05 | S | M2-02 |
| `[x] M2-04` | Recently Deleted view grouped by reason; Restore; Empty now. Done: sidebar entry with count under the Pairview trees opens `src/pv/deleted/RecentlyDeleted.tsx` (reason tabs with sizes, operations newest first with days left, previews via `pvimg://localhost/deleted/<loc>/<op>/<photo>` from the manifest, Restore all / Restore selected, Empty now behind a confirm); 30-day purge runs at startup in the background. Owner checked 2026-11-05 | S | M2-01 |
| `[x] M2-05` | Orphan detection (Photo that used to have a display copy) + review grid 🛡. Done: `⚠ RAW` comes from the pairing state history (M1-05); a display copy dropped through Pairview (Keep RAW only) now leaves plain RAW-only instead. Sidebar banner → `src/pv/orphans/OrphanReview.tsx`: everything not locked starts selected; Delete RAWs (to Recently Deleted, behind a confirm), Keep as RAW-only (catalog only), Ignore (per-Photo, remembered). Nothing is deleted without a click. Owner visual check pending | S → O review | M1-05, M2-01 |
| `[x] M2-06` | Undo for batch ops (op-log replay) 🛡. Done: every delete is one action (its operations per location/kind share an action id in the op-log and manifest); `lifecycle::undo_last` restores the newest action still in Recently Deleted, again for the one before, until purge; offline locations are skipped. Cmd/Ctrl+Z in the library and an Undo button on the delete toast. Metadata batch undo (ratings) is not part of this: XMP edits are not file moves | O | M2-01 |
| `[x] M2-07` | Heuristic RAW-value signals (clipping, crushed shadows, ISO, WB/CCT, DR, rating, edited) computed from the display copy's histogram + EXIF; reason chips. Done: `pv/signals.rs`; statistics at 512 px from the grid's source (the RAW's camera preview = the HIF's rendering, read cheaply; the rendered HIF when edited), cached in `raw_signals` (catalog v2) per file version; verdict derived per request: lock > reject > rated ★★★+/Pick/edited > image/EXIF signals (1 → Your call, 2 → Keep RAW) > shoot intent. Gray-world cast thresholds raised after the real shoot (warm rooms read 1.7–2.0 R/B after AWB). Shown in the viewer info panel (I). Recommends only | S | M1-05 |
| `[x] M2-08` | Shoot intent chip (manual in M2; the AI guess arrives in M4) + Keep-RAW lock `K`. Done: Casual · Normal · Important chip next to the folder path (one `// PV:` line in `MainLibrary.tsx`), stored in `<shoot>/.pairview-shoot.json` (catalog v3 keeps a copy) and fed into the RAW-value verdicts; K in the grid cycles Auto → Keep → Drop for the selection (capture phase, so upstream's K = AI panel only fires when no Pairview Photo is selected). Owner visual check pending | S | M1-08 |
| `[x] M2-09` | Ready-to-slim (30-day) + **manual Review for slimming** screen; Apply → lifecycle drop 🛡. Done: `pv/slim.rs` + `src/pv/slim/SlimReview.tsx`. Ready to slim (sidebar) = shoots past 30 days with unlocked pairs and no finished review (catalog only); ⌘⇧K reviews the selection or the whole view any time. Tabs HIF is enough (pre-selected) · Your call · Keep RAW with chips and a "Suggestion only" mark for unrated Photos; locked Keep can't be selected. Apply (confirmed) → Keep HIF only through lifecycle (Slimmed RAWs, undoable); edits newer than their render are refused (`NeedsRender`). Done/Apply mark the shoot reviewed in `.pairview-shoot.json` (catalog v4). No automatic slimming. Owner visual check pending | S → O review | M2-01, M2-07 |
| `[x] M2-10` | Offline handling across UI (disabled mutations, badges). Done: `src/pv/offline.ts` keeps location statuses (tree loads, focus, every 30 s); an offline folder shows "<name> offline · read-only" in place of the intent chip; Delete, K and ⌘⇧K say the location is offline instead of trying. The backend still refuses every change on its own (lifecycle, metadata, shoot file), so anything else that slips through fails safely with the same message. Owner visual check pending | S | M1-02 |
| `[ ] M2-11` | Exit check: delete/restore/slim on a **local copy** of a real shoot, then on the NAS with owner present. Local half done (`docs/M2-exit.md`); NAS half waits for the owner | O | all M2 |

## 8. M3 — Edit loop + Looks (task level)

| ID | Task | Tier | Depends |
|---|---|---|---|
| `[x] M3-01` | `E` opens the editor on the RAW (representative path) from grid and viewer. Done: the viewer had it (M1-14); E in the grid takes over upstream's E (export panel) only when a Pairview Photo is active | S | M1-14 |
| `[x] M3-02` | **Done → overwrite** the display copy in its own format (HIF 10-bit via M0-06, JPG q92), metadata copy, camera original to Recently Deleted 🛡. Done: `pv/edit.rs` (encode, decode back to verify), `lifecycle::replace_display` (renames under the op-log; all crash points tested), `pairview:Edited` XMP marker, `render_for_pairview` hook, Done button. Owner checked 2026-11-05 on a real ARW+HIF: the edit reached the HIF, the original is in Replaced originals. Open point: a render from the RAW encodes to about twice the camera's size at quality 0.9 (23.9 vs 12.5 MB) | O | M0-06, M2-01 |
| `[x] M3-03` | Hidden base for no-RAW Photos (`.originals/`), revert, re-edit from recipe 🛡. Done: first Done on a Photo without a RAW moves its original to `<folder>/.originals/<name>`; `read_file_mapped` redirects to the base, so RapidRAW's editor, thumbnails and export use base + recipe and edits don't stack (the viewer reads the render); a whole-Photo delete takes the base along. Revert puts the original back (camera file from Recently Deleted, or the base) and sends render + recipe to Recently Deleted; refused after the 30-day purge rather than re-rendering in another look. All crash points tested. Owner visual check pending | O | M3-02 |
| `[x] M3-04` | Thumbnail/viewer cache invalidation after overwrite. Done: grid thumbnails were already keyed on RAW mtime + recipe; the viewer URL gets a per-session display version; RAW-value statistics follow the file's mtime | S | M3-02 |
| `[x] M3-05` | **Edit-history capture** for style learning (features + final sliders + look), local store. Done: `pv/history.rs` appends one JSON line per Done to `pairview-edit-history.jsonl` next to the catalog (not in it: the catalog is a rebuildable cache, this isn't). Tone statistics of the pre-edit image, EXIF, sliders without blobs, look; Revert appends a marker; the trainer (M6) takes the latest line per Photo. CLIP embeddings come with M4 | S | M3-02 |
| `[x] M3-06` | **"My Sony" look fitting**: sample ARW→HIF pairs, fit tone curve + 3D LUT, save as a RapidRAW LUT preset; quality metric (ΔE vs camera HIF). Done: `pv/look.rs`, `pv/look_job.rs`, results in `docs/M3-look.md`. On 12 held-out pairs the median ΔE2000 falls from 13.13 to 1.17; installed as `My Sony.cube`. Nothing applies it by default yet (M3-07/08) | O | M3-01 |
| `[ ] M3-07` | Looks picker (live thumbnails, intensity), classic looks library v1 (LUT files) | S | M3-06 |
| `[ ] M3-08` | Generic Auto button wired to "My Sony" base + RapidRAW auto adjustments | S | M3-06 |
| `[ ] M3-09` | Exit check: edit 20 photos end-to-end; default render vs camera HIF ΔE report | O | all M3 |

## 9. M4–M6 and Windows (outline)

- **M4 — AI triage + Import:** ONNX models (CoreML) for sharpness/eyes (reuse RapidRAW face/culling), CLIP intent guess, burst grouping, AI ★ (`pairview:ai*` XMP), human-pass UI, FTP New arrivals + auto-tidy (quiet 30 min), SD import. AI never mutates files (enforced by lifecycle requiring a user-confirmed op token).
- **M5 — Find + Collections:** smart collections on top of RapidRAW albums, quick collection, filter bar, CLIP semantic search, map (MapLibre), video (AVFoundation), pair-safe batch rename 🛡.
- **M6 — Style:** regressor training, Auto = my style, named styles, "Why?", batch auto, rebake-before-drop 🛡.
- **W — Windows:** after M3. Per the tabling rule (PRD §10.1): WIC/HEVC-extension HIF decode first; HEIF encode tabled (JPG output); video tabled.

---

## 10. Risks to watch during implementation

| Risk | Where | Mitigation in the plan |
|---|---|---|
| RapidRAW's grid assumes one file = one item in many places (selection, filmstrip, delete, export) | M1-12 | A4 representative paths. Audit `useSortedLibrary`, `Filmstrip`, `useFileOperations` early in M1-12, and list every assumption before coding |
| Upstream changes `file_management.rs` often (4,000+ lines) | A7 hooks, M1-07 | Keep hooks to 1–5 lines that call into `pv`. Merge upstream before starting each milestone |
| SMB edge cases (case-insensitive names, `._` AppleDouble files, slow `stat`) | M1-03/04/06 | Excludes list, case-folded basenames, timeouts on every NAS call |
| Data loss bug in lifecycle | M2 | 🛡 review gate, safety suite, local-copy testing before the NAS, Recently Deleted for everything |
| Disk space on the Mac (33 GB free) | all | `cargo clean` between milestones; release builds only when packaging |

## 11. Cost guidance (approximate share of tasks)

| Tier | M0–M3 tasks | Share |
|---|---|---|
| O | 15 | ~34% (but most of the risk) |
| S | 25 | ~57% |
| H | 4 | ~9% |

Biggest savings come from running **S** tasks on Sonnet instead of Opus. Keep **O** for 🛡 and design tasks; Haiku helps mainly with i18n, branding, and boilerplate.
