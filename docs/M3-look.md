# "My Sony" look (M3-06)

A 33-point 3D LUT that turns RapidRAW's neutral render of an A7C II RAW into what the camera put in its HIF. Fitted 2026-11-05 from the owner's own pairs. Installed as `My Sony.cube` in the app's `luts` folder (`~/Library/Application Support/com.danielxia.pairview/luts/`), where RapidRAW's LUT picker lists it.

**Method.** 60 ARW+HIF pairs, one at a time across the 2025-06 to 2026-10 shoots, copied read-only from the NAS. RAWs rendered through the export pipeline at full resolution with default settings; both sides reduced to 768 px and compared as aligned 4×4 blocks, skipping edges. Per-channel tone curves first, then a smooth 3D correction that fades back to the curves where there's no data. Every 5th pair never trained the look. Code: `pv/look.rs` (fit, tests), `pv/look_job.rs` (job).

```sh
PV_LOOK_DIR=/path/to/lookfit cargo test --lib pv::look_job -- --nocapture
```

Set up the folder with `scripts/pv-sample-look-pairs.py` (pairs) and `scripts/look-neutral.json` (saved there as `neutral.json`: RapidRAW's default recipe). `PV_LOOK_DIR` must not contain `..`, because RapidRAW's LUT loader rejects such paths.

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

# Fujifilm-inspired styles (M3-13)

LUTs that give the Sony's neutral render the character of a Fuji film mode. Fitted 2026-11-06 from public RAFs, [listed with their licences](M3-look-sources.md). Installed in the app's `luts` folder; not committed, per the plan's rules. Named `Fujifilm …`, which puts them under Camera styles.

| Look | Frames | Scored | Neutral render | With look | Worst frame | On the Sony: saturation |
|---|---|---|---|---|---|---|
| Fujifilm Provia | 31 | 31 | 10.33 | **1.95** | 17.85 | 0.135 |
| Fujifilm Astia | 9 | 8 | 7.92 | **2.26** | 5.37 | 0.124 |
| Fujifilm Classic Chrome | 9 | 8 | 11.00 | **2.22** | 3.44 | 0.116 |
| Fujifilm Velvia (approximate) | 15 | 12 | 12.75 | **3.02** | 8.02 | 0.144 |

Median of per-frame median ΔE2000 against the camera's JPEG, on Fuji's own frames. The bar is ≤ 3 on at least 5 frames. The Sony's own files have saturation 0.139 and its neutral render 0.110, so the order is what these film modes should give, and it wasn't built in. Looks differ from each other by a median of 2.4 to 3.5 on the Sony.

**Velvia misses by 0.02.** Without smoothing it scores 3.00 but keeps a downward step on its grey axis at 0.84. Smoothing was decided on before these numbers, so it stands and the look is labelled approximate, as the plan says. The job writes that label itself.

**Left out, with no file written:** Acros (2 scenes), Eterna (2), Reala Ace (2), Classic Neg (1), Nostalgic Neg (1). There's nothing to hold out, so nothing could say how good they are. Those counts are before the settings filter, which leaves one Acros scene and none of the other four.

**Method.** The same fit as My Sony, with these differences:
- **Camera side.** The JPEG inside the RAF, with its orientation applied, converted from Adobe RGB where needed (12 of the frames).
- **Alignment.** Fuji's JPEGs are lens-corrected and the render isn't, so zoom and shift are searched per frame. Frames with correlation under 0.92 or under 1,000 usable blocks are dropped: 9 of 73.
- **Scoring.** One frame per scene, and each scored frame is held out in turn, with a fit that saw neither it nor anything else by its photographer. The final look trains on all of them. This matters: holding out only the frame, Astia scored 1.18, because 7 of its 9 frames come from two people.
- **Brightness gate.** Frames with a median brightness outside 0.12–0.85 are excluded, 8 of 81. Near-black colours can hardly differ, so night scenes flattered the score. A first run gave Provia 2.27 on a set that happened to include three of them.
- **DR.** DR200 and DR400 frames are fine, because RapidRAW adds +1 and +2 EV for them.
- **Smoothing.** One 1-2-1 pass over what the look adds to the identity, against stray nodes.
- **Moving to the Sony.** One input curve for all looks, so they keep their differences, computed so that Provia on the Sony's neutral render is as bright as the Sony's own file. It's close to identity (0.5 → 0.57). Luma on the 12 held-out Sony pairs is 0.51–0.52 against the camera's 0.49.

Through the real GPU path on 3 Sony pairs, each look matches the CPU calculation to a median of 0.05 or better.

```sh
python3 -I scripts/pv-playraw-harvest.py <dir>/survey            # slow, on purpose
python3 -I scripts/pv-raf-survey.py <urls.txt> <survey.jsonl>
python3 -I scripts/pv-style-select.py <dir>                      # expects survey/all.json
python3 -I scripts/pv-style-fetch.py <dir>
PV_STYLE_DIR=<dir> PV_LOOK_DIR=<Sony pairs> cargo test --lib pv::look_job::fit_camera -- --nocapture
python3 scripts/pv-style-gate.py <dir>                           # then run the job again; needs numpy, pillow
```

**Limits.**
- **These scores are Fuji on Fuji.** Nothing measures how close a look is on the Sony, because no Sony file exists in Provia. The transfer rests on RapidRAW's neutral renders being comparable between brands. Saturation order, brightness and GPU match are checked; the character needs the owner's eye.
- Provia's tone on the Sony follows the Sony's own files, by construction. The other looks keep their difference from Provia.
- Astia's frames come from four photographers, on X-T3, X-T30 and X-T4 bodies only. It has a grey-axis tint of up to 0.04 around 0.62, the largest of the four. Watch for it in grey skies.
- Some frames are far off and stay in the score: an X-T10 frame at 17.85, where the camera lifted the image far more than usual, and an X-T30 frame at 13.52 with no lift at all, unexplained. The X-T30 frame fell out when frames were reselected at random, and was put back by hand so that it couldn't vanish from the score unnoticed.
- Fits came from RapidRAW's default tone mapper, so a change there means fitting again.
