# "My Sony" look (M3-06)

A 33-point 3D LUT that turns RapidRAW's neutral render of an A7C II RAW into what the camera put in its HIF. Fitted 2026-11-05 from the owner's own pairs. Installed as `My Sony.cube` in the app's `luts` folder (`~/Library/Application Support/com.danielxia.pairview/luts/`), where RapidRAW's LUT picker lists it.

**Method.** 60 ARW+HIF pairs, one at a time across the 2025-06 to 2026-10 shoots, copied read-only from the NAS. RAWs rendered through the export pipeline at full resolution with default settings; both sides reduced to 768 px and compared as aligned 4×4 blocks, skipping edges. Per-channel tone curves first, then a smooth 3D correction that fades back to the curves where there's no data. Every 5th pair never trained the look. Code: `pv/look.rs` (fit, tests), `pv/look_job.rs` (job).

```sh
PV_LOOK_DIR=/path/to/lookfit cargo test --lib pv::look_job -- --nocapture
```

**Result on the 12 held-out pairs** (CIEDE2000; about 1 is barely visible side by side, 2–3 noticeable if you look for it):

| | Neutral render | With My Sony |
|---|---|---|
| Median of per-photo medians | 13.13 | **1.17** |
| All pixels, median | 13.83 | 1.50 |
| All pixels, 95th percentile | | 4.30 |
| Worst photo (median) | 18.67 | 3.83 |

Through the real GPU path on 3 held-out pairs: 1.11, 0.74, 1.65, matching the CPU check (1.13, 0.74, 1.73).

**Limits.**
- These numbers skip edges and textured blocks, where a pixel of misalignment would swamp the measure. Smooth areas decide the look, but this is not a whole-image score.
- A single LUT can't reproduce what varies per photo. 1 of 48 training pairs was left out (ΔE > 8): a dusk scene where the camera lifted the whole image much further than usual. Scenes like that will come out darker than the camera's file.
- Fitted on one camera with its usual settings, and only against sRGB-decoded HIFs. Another camera needs its own fit.
- It depends on RapidRAW's default tone mapper (`agx`). Changing the default or a large upstream change to the pipeline means fitting again.

## Applying it (M3-07, M3-08)

A RAW Photo with no recipe opens with My Sony on, as an ordinary LUT in the recipe; Reset and "before" in before/after keep it. Saved recipes are never changed. Photos without a RAW don't get it: their file already is the camera's rendering.

**Auto.** RapidRAW's Auto judges the neutral image, which reads dark because the look does the brightening, so it added about +0.7 EV on most photos and brightened twice. With a look on, exposure is now judged on a small render through the look, and left alone while mean luma stays in 0.33–0.62, the range the camera's own files sit in. The other Auto sliders stay.

| Same 12 held-out pairs, My Sony | Median ΔE range | Mean luma vs camera |
|---|---|---|
| Auto + look, before the fix | 1.6–15.1 | up to 0.17 too bright |
| Auto + look, after | 0.7–3.3 | within 0.03 |
| Look alone | 0.6–3.8 | within 0.03 |

Measured the way the app computes it (`pv::look_job::auto_with_looks`). A first version shrank its input with `DynamicImage::thumbnail`, which corrupts floating-point images, so its exposure judgement was wrong; use `pv::thumbs::shrink`.

# Film looks (M3-10)

All 20 Spektrafilm stocks, baked by `scripts/pv-bake-film-looks.py` into `src-tauri/resources/pv_film_luts/`, with stocks, papers and exposure listed in its `CHANGELOG.txt`. These are what the Looks panel shows. Upstream's six in `film_luts` stay in the Effects LUT control, unchanged.

- **Same as the reference, apart from exposure.** With no exposure compensation the bake reproduces RapidRAW's `K-Portra 400` exactly (max difference 0.0).
- **Exposure.** At 0 EV these looks came out about a stop darker than the camera: mean luma 0.25 against 0.44, because Sony RAWs keep highlight headroom the camera normally lifts. Each is baked with film exposure compensation (+1.10 to +1.70 EV) that lands it near the camera's brightness. On the held-out pairs every stock's median is 0.44–0.46 against the camera's 0.444, and each photo keeps its own brightness.
- **Auto.** With Portra 400 on 12 held-out pairs, exposure stays at +0.00 and mean luma stays within 0.05 of the camera.
- **Slide films** have no print stage, so they're baked as a direct scan of the film.
- **Highlights are not pure white:** Portra 400's gray axis tops out near 0.85, its print's maximum in the simulation. That's part of the look; it can be dialled back with intensity.

**Limits.** A LUT can't hold grain, halation or anything local (M3-11, M7). Recipes store a look's absolute path, and a bundled look's path differs between dev and packaged builds; a look that can't be found now renders as no look instead of black, but the recipe would have to be pointed at it again. To sort out before packaging.
