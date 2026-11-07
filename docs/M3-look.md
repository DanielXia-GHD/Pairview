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

**Limits.** A LUT can't hold grain, halation or anything local (M3-11, M7).

**When the app moves.** Recipes store a look's full path, which differs between dev and packaged builds and between machines. Recipes aren't rewritten for this. A path that still exists is used as it is; one that doesn't is looked up in this install by folder and file name (`pv::looks::current_path`, in `parse_lut_file`, so every loader gets it). The folder counts, because `pv_film_luts` and upstream's `film_luts` hold different LUTs under the same file names. A look that exists nowhere still renders as no look. The panel and upstream's LUT control recognise a moved look the same way (`isSameLook`). Through the real pipeline, a recipe with a packaged-app path renders identically to the look itself, and the same path into `film_luts` differs by a mean of 0.15. Not yet tried in an actual packaged build, since none exists.

# Fujifilm-inspired styles (M3-13)

LUTs that give the Sony's neutral render the character of a Fuji film mode. Fitted 2026-11-06 from public RAFs, [listed with their licences](M3-look-sources.md). Installed in the app's `luts` folder; not committed, per the plan's rules. Named `Fujifilm …`, which puts them under Camera styles.

| Look | Frames | Scored | Neutral render | With look | Worst frame | On the Sony: saturation |
|---|---|---|---|---|---|---|
| Fujifilm Provia | 31 | 31 | 10.33 | **1.95** | 17.85 | 0.135 |
| Fujifilm Astia | 13 | 10 | 7.92 | **1.81** | 3.83 | 0.135 |
| Fujifilm Classic Chrome | 10 | 8 | 11.00 | **2.22** | 3.44 | 0.116 |
| Fujifilm Velvia | 17 | 14 | 12.75 | **2.57** | 6.52 | 0.149 |

Median of per-frame median ΔE2000 against the camera's JPEG, on Fuji's own frames. The bar is ≤ 3 on at least 5 frames. The Sony's own files have saturation 0.139 and its neutral render 0.110. Velvia is the most saturated and Classic Chrome the least, as they should be, and that wasn't built in. Looks differ from each other by a median of 2.1 to 3.1 on the Sony.

**More data decided Velvia and Astia, with the method unchanged.** After searching part of Play Raw, Velvia scored 3.02, which missed the bar, and Astia 2.26. Searching the rest brought Velvia to 2.57 and Astia to 1.81. Astia's saturation on the Sony also moved, from 0.124 to 0.135, so differences that small between looks aren't reliable. Thin looks would move again with more frames.

**Left out, with no file written:** Acros (2 scenes), Eterna (2), Reala Ace (2), Classic Neg (1), Nostalgic Neg (1), across all of Play Raw and raw.pixls.us. There's nothing to hold out, so nothing could say how good they are. Those counts are before the settings filter, which leaves one Acros scene and none of the other four.

**Method.** The same fit as My Sony, with these differences:
- **Camera side.** The JPEG inside the RAF, with its orientation applied, converted from Adobe RGB where needed (11 of the frames).
- **Alignment.** Fuji's JPEGs are lens-corrected and the render isn't, so zoom and shift are searched per frame. Frames with correlation under 0.92 or under 1,000 usable blocks are dropped: 11 of 82.
- **Scoring.** One frame per scene, and each scored frame is held out in turn, with a fit that saw neither it nor anything else by its photographer. The final look trains on all of them. This matters: with 9 frames, holding out only the frame gave Astia 1.18, and holding out its photographer as well gave 2.26, because 7 of those 9 came from two people.
- **Brightness gate.** Frames with a median brightness outside 0.12–0.85 are excluded, 12 of 94. Near-black colours can hardly differ, so night scenes flattered the score. A first run gave Provia 2.27 on a set that happened to include three of them.
- **DR.** DR200 and DR400 frames are fine, because RapidRAW adds +1 and +2 EV for them.
- **Smoothing.** One 1-2-1 pass over what the look adds to the identity, against stray nodes.
- **Moving to the Sony.** One input curve for all looks, so they keep their differences, computed so that Provia on the Sony's neutral render is as bright as the Sony's own file. It's close to identity (0.5 → 0.57). Luma on the 12 held-out Sony pairs is 0.51–0.52 against the camera's 0.49.

Through the real GPU path on 3 Sony pairs, each look matches the CPU calculation to a median of 0.06 or better.

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
- Astia's frames come from a handful of photographers, on X-T3, X-T20, X-T30 and X-T4 bodies only. Provia and Astia have the largest grey-axis tint, up to 0.026.
- Some frames are far off and stay in the score: an X-T10 frame at 17.85, where the camera lifted the image far more than usual, and an X-T30 frame at 13.52 with no lift at all, unexplained. The X-T30 frame fell out when frames were reselected at random, and was put back by hand so that it couldn't vanish from the score unnoticed.
- Fits came from RapidRAW's default tone mapper, so a change there means fitting again.

# Leica-inspired styles (M3-14)

Not fitted from public files: there aren't enough. [Sources](M3-look-sources.md).

The method does carry over. A modern Leica DNG embeds a near-full-size camera JPEG with no EXIF of its own, and `scripts/pv-dng-survey.py` reads and extracts it, so the job takes DNGs (`PV_STYLE_BRAND=Leica PV_STYLE_BASE=Standard`). RapidRAW renders them, and they line up with their JPEGs.

On the 4 CC0 frames with enough usable blocks, the median is 4.08 and the worst frame 7.45, against a bar of 3 on at least 5 frames. Each comes from a different body, so holding one out leaves very little.

**What would change this:** the owner's friend may shoot a Xiaomi test in RAW + JPEG. Those frames would have known modes, but would be Xiaomi's rendition of Leica's looks and should be named so. Per-scene HDR may also stop a single LUT from reaching the bar, which the test will show.
