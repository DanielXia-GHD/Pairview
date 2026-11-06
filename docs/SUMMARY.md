# Pairview: project summary

As of 2026-11-06, after M3-13.

Pairview is a personal photo library, viewer and editor for macOS (M2), forked from [RapidRAW](https://github.com/CyberTimon/RapidRAW). A RAW and its display copy (ARW + HIF or JPG) are managed as one "Photo". The library lives mainly on a QNAP NAS.

- **What to build:** [PRD.md](PRD.md) (v0.4)
- **How, and in what order:** [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) (every task and its status)
- **Resuming work:** [HANDOVER.md](HANDOVER.md)

## Where it stands

| Milestone | Status |
|---|---|
| M0 Mac spikes | Done |
| M1 Library + viewer | Done |
| M2 Lifecycle + Keep-RAW | Built. Open: M2-11's NAS half |
| M3 Edit loop + looks | Mostly built. Open: M3-14, M3-15, M3-12, M3-09 |
| M4–M7, Windows | Not started |

204 Pairview tests pass. Nothing is packaged yet: the app runs through `tauri dev`.

## What works today

**Library and viewer (M1).** Locations on disk, external drives or the NAS, with offline handling. RAW + HIF/JPG pairing across `arw/` and `hif/`, including JPGs with a counter suffix, and a capture-time guard that splits false pairs. Folder and date views. Ratings, flags and labels in XMP, mirrored to RapidRAW. A Pixea-style viewer with slideshow, info panel and histogram.

**Deleting and slimming (M2).** Nothing is erased directly: files move to `.pairview-deleted/` on the same volume, with a manifest, and every crash point rolls back. Three-way delete dialog (Delete photo, Keep HIF only, Keep RAW only), Recently Deleted with Restore and Empty now, 30-day purge, undo, orphan review, RAW-value verdicts with reasons, shoot intent, the Keep-RAW lock (K), Ready to slim and Review for slimming.

**Editing (M3).** E opens the RAW. **Done** renders the recipe at full size and replaces the HIF or JPG in its own format, verified by decoding it back, with the old file in Recently Deleted. **Revert** brings the original back. Photos without a RAW keep a hidden original in `.originals/`, so edits don't stack. Each Done adds a line to a local edit history for later style learning.

**Looks (M3).** A Looks panel with live thumbnails, favourites and intensity.
- **My Sony:** fitted from 60 of the owner's ARW + HIF pairs, and the default for a RAW with no recipe. Median colour error against the camera on 12 photos it never saw: 13.13 before, **1.17** after (ΔE2000). [Details](M3-look.md).
- **20 Spektrafilm film stocks:** baked into LUTs and calibrated to the camera's brightness. With an optional per-stock grain + halation toggle.
- **Fujifilm-inspired styles:** Provia, Classic Chrome, Astia and Velvia, fitted from public RAFs and held to median ΔE ≤ 3 against the camera's JPEG. Acros, Eterna, Reala Ace, Classic Neg and Nostalgic Neg are left out for lack of public frames. [Details](M3-look.md), [sources](M3-look-sources.md).
- **Auto** judges exposure on the look's result, so it doesn't brighten twice.

## Measured on the real NAS (M1)

[M1-exit.md](M1-exit.md). 158 shoots, 9,673 Photos. Opening a day of thumbnails takes about 4.5–5.4 s cold (target 5 s). A HIF opens in about 0.45 s (target 0.6 s). First EXIF index takes 15 minutes against a 5-minute target: the NAS costs about 25 ms per file whatever is read, so the owner accepted this as a one-time cost.

## Checked by the owner, and not yet

| Checked in the app | Not yet checked in the app |
|---|---|
| M1 walkthrough (on one shoot) | Orphan review, undo, RAW-value chips (M2-05 to M2-07) |
| Delete dialog, Recently Deleted (M2-03, M2-04) | Shoot intent chip and K in the grid (M2-08) |
| Done on a real ARW + HIF (M3-02) | Ready to slim, Review for slimming (M2-09) |
| Looks panel appears with all stocks (M3-07, M3-10) | Offline badge (M2-10) |
| | Space and double-click from `arw/` and `hif/`, before/after on a RAW |
| | Revert, editing without a RAW (M3-03) |
| | Auto with a look (M3-08), grain + halation toggle (M3-11) |
| | Fujifilm styles on the owner's photos (M3-13) |
| | Smoke test of upstream merge `47cdc439` |
| | NAS half of M2-11: delete, restore, slim on the share |

The logic behind these has automated tests. What's unchecked is the screens themselves and SMB behaviour.

## Key decisions

- **Personal use**, Mac first. English only. Windows parity only where cheap.
- **Nothing is deleted without a human.** AI never drops a RAW. Retention is 30 days.
- **Folders and sidecars are the truth;** the SQLite catalog is a rebuildable cache.
- **Edits overwrite the display copy in its own format,** with the camera original in Recently Deleted. HEIF quality 0.8 (the owner's choice: a render from the RAW at 0.9 was 23.9 MB against the camera's 12.5 MB, and 0.8 measured about 16 MB).
- **Film looks are baked LUTs now;** a native simulation via the Rust port is M7. Fujifilm- and Leica-inspired styles are in M3; Nikon and Canon are out of v1.
- **A Photo has one look at a time,** because RapidRAW has one LUT slot.

## Known limits

- A single LUT can't reproduce per-photo processing: scenes the camera lifted hard, such as dusk, come out darker than its file.
- My Sony is specific to this camera and RapidRAW's default tone mapping.
- Film look highlights top out near 0.85 rather than pure white. That's part of the simulation, and intensity softens it.
- Grain and halation values are reasoned starting points, not measured.
- Fujifilm styles are scored on Fuji's own frames. Nothing can measure how close they are on the Sony, so that rests on the owner's eye.
- The Fujifilm LUTs exist only in the app data folder, like `My Sony.cube`. They aren't committed, by the rules for public samples.
- Recipes store a look's absolute path, and bundled looks live elsewhere in a packaged build. A missing look now renders as no look rather than black, but this needs solving before packaging.
- First L0 listing of the whole NAS takes about 150 s. Still to investigate (M1 fix list).
