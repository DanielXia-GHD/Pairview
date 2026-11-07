# AI triage, first pass (M4)

Bursts, the best of each, suggested rejects and a dimmed AI rating, for one shoot at a time. Press **AI triage** in the library header, next to the shoot intent chip. Code: `pv/triage.rs`, `src/pv/grid/PvTriage.tsx`.

**It only suggests.** It reads images, and writes only `triage_signals` in the catalog, which is rebuildable. It changes no file, rating or flag: a test snapshots a shoot's bytes around a run, and another run on the real Desktop shoot checks sizes and modified times. Nothing starts by itself; the owner presses the button.

**Human input wins.** Suggestions are derived on request with current ratings and flags, so a rating counts at once. A Photo that's rated, picked or locked Keep RAW is never suggested for rejection, and a pick is the best of its burst whatever the numbers say.

## What it measures

From the same preview the grid uses, at 768 px:

| Signal | How |
|---|---|
| Sharpness | Laplacian variance of the 3 sharpest tiles out of a 12-wide grid, so a sharp subject on a soft background counts as sharp |
| Likeness | 64-bit difference hash |
| Exposure | Mean, spread, clipped and near-black shares |

## What it suggests

| Suggestion | Rule |
|---|---|
| Burst | Consecutive frames within 2 s and 20 hash bits, or within 60 s and 6 bits |
| Best of burst | Owner's pick, then rating, then sharpness marked down for clipping |
| Reject: misfire | Spread under 5, or mean under 6 or over 250 |
| Reject: blurry | Under 0.1× the shoot's median sharpness (at least 6 Photos), or under 0.35× the burst's best |
| Reject: not the best | The other frames of a burst |
| AI ★ 0–5 | Rank by sharpness in the shoot; +1 best of burst, −1 the rest, −1 harsh clipping or shadows; 1 if blurry, 0 if a misfire |

## Measured

**60 of the owner's local frames, one per shoot** (numbers only; `pv::triage::calibrate_on_real_frames`):

| | p10 | Median | p90 |
|---|---|---|---|
| Sharpness | 192 | 966 | 3,189 |
| Sharpness kept after a 1.5 px blur | 6% | 11% | 20% |
| Sharpness kept after a 4 px blur | 0.3% | 0.8% | 2.7% |
| Hash bits between different scenes | 24 | 31 | 38 |

| "Blurry" limit against the median | Sharp frames flagged | With 1.5 px blur | With 4 px blur |
|---|---|---|---|
| 0.2 | 7 of 60 | 49 | 60 |
| **0.1 (used)** | 1 | 24 | 60 |
| 0.05 | 0 | 9 | 60 |

A first choice of 0.2 would have flagged about 1 ordinary photo in 10: scenes differ a lot in texture. These frames come from different shoots, so this overstates the spread within one.

**The Desktop shoot, 4 Photos:** measured in 0.22 s. No bursts, no rejects.

## Limits

- **Burst grouping has not been tested on a real burst.** None is available locally. `DSC03536` and `DSC03537` are the same scene 2.06 s apart and 28 bits apart, and unrelated scenes have a median of 31, so the hash separates poorly once anything moves. It errs towards not grouping, which suggests no rejects. 39 of 1,770 pairs of different scenes fall within 20 bits, which only matters if they're also within 2 s. Real bursts may need the limit moved.
- **AI ★ is mostly a sharpness rank within the shoot.** It sorts, but doesn't know whether a photo is good. A 4-Photo shoot spreads across the range whatever the photos are like.
- **Sharpness is confused by noise and texture.** High ISO reads as detail, and a sharp foggy scene reads as soft.
- **The screens have not been run.** The button, tile badges and filter pass type checks and lint, but nothing here can click them.
- The owner's `K` lock, pick and rating are honoured. Upstream's own culling (`culling.rs`) is separate and untouched.

## Not built yet (M4)

| What | Why not yet |
|---|---|
| Eyes closed, face blur, shoot intent guess, "document" content | Need face and CLIP models, which download. The owner's call |
| `pairview:ai*` in XMP (PRD §6.5) | Writes a sidecar per Photo, including on the NAS. Catalog only for now. The owner's call |
| Running by itself on arrival | Waits for FTP arrivals, and for the owner to trust the output |
| Cull view, sorting by AI ★, "accept AI rejects" in one press | Next, once the suggestions look right |
| FTP New arrivals, SD import | Need the owner's setup, and write files |
