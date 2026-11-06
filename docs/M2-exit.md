# M2 exit check (M2-11)

## Local copy of a real shoot: done (2026-11-05)

Temp copy of `~/Desktop/2026-10-23_10261023` (ARW + HIF). Headless, through the same code the app calls:
`PV_FIXTURES=~/Desktop/2026-10-23_10261023 cargo test --lib pv::lifecycle::tests::exit_check -- --nocapture`

| Step | Result |
|---|---|
| Rate a Photo, then slim straight away | Its new XMP and `.rrdata` go along; nothing is left in `arw/` |
| Drop 3 RAWs (135 MB) | 0.13 s, HIFs byte-identical |
| Rating after the drop | Kept: XMP next to the HIF, also in a catalog rebuilt from disk |
| Undo | All 5 files back, library byte-identical, no second XMP |
| Delete, Keep HIF only, restore, Empty now | Round trip byte-identical (`real_shoot_round_trip`) |
| Safety suite (plan §4.3, items 1–6) | Passes on synthetic fixtures |

Found and fixed by this check:
1. **Rate, then delete at once left sidecars behind.** The catalog hears of new sidecars at the next scan. `lifecycle::plan::sync_sidecars` now lists the Photos' folders right before a real delete.
2. **Restore left the XMP copy next to the HIF.** That second XMP could bring back an old rating on the next drop. A restore now folds the copy into the original; if the rating changed while the RAW was gone, the newer one wins.

## Not done yet

| Step | Needs |
|---|---|
| Delete, restore, slim on the NAS | Owner present (plan M2-11); the share mounted |
| Owner check of the M2-05 to M2-10 screens in the app | Owner |
| `rename()` and listing speed for a large operation on SMB | The NAS run above |

Headless runs can't cover: the screens themselves, SMB behaviour, and Recently Deleted on a share where the user can't create dot-folders.
