# Hand-over: resuming Pairview in a new session

Written 2026-11-06. Read this, then [SUMMARY.md](SUMMARY.md), then the M3 section of [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md). Update the plan when a task finishes.

## To start the new session

Paste this:

> Continue Pairview. Read docs/HANDOVER.md and docs/SUMMARY.md first. Tell me briefly where things stand, then ask what I want next.

## Immediately next

| Task | What | Blocked on |
|---|---|---|
| **Merge to `main`** | Branch `merge/upstream-20261106` holds upstream's 5 commits and the look-path fix. `main` stays behind it until the owner's smoke test (§4.4) | The owner |
| **M3-13 check** | The owner looks at the four Fujifilm styles on their own photos | The owner. Fits can't be measured on the Sony |
| **M3-14** | Leica-inspired styles. Modern Leica DNGs embed a near-full-size camera JPEG, so the method carries over (`scripts/pv-dng-survey.py`, `PV_STYLE_BRAND`, `PV_STYLE_BASE`), but film mode isn't recorded in the samples and the 7 CC0 frames aren't enough. A Play Raw search for DNGs was running (`scratchpad/leica/survey/playraw/` from the M3-13 session). The owner's friend may shoot a Xiaomi RAW + JPEG test | Frames |
| **M3-15** | Tweak some film looks. The owner said only "some film looks need further tweaking" | The owner listing which and what's off. Ask |
| **M3-12** | Sony Creative Looks: fit from the owner's frames | The owner shooting 3–5 RAW + HIF scenes for each of ST, PT, NT, VV, VV2, FL, IN, SH, BW, SE |
| **M3-09** | M3 exit: edit 20 photos including a film look and a camera style | All of M3; the owner |
| **M2-11** | NAS half: delete, restore, slim on the share | The owner present, NAS mounted |

Then M4. Merge upstream before starting a milestone ([routine](IMPLEMENTATION_PLAN.md), §4.4).

**Rules for public samples** are in the plan under the M3 table. In short: the owner chose download without asking per source, within limits: image files only, each source in its own folder outside the repo, read the terms and skip a source that forbids this use, log every source in `docs/M3-look-sources.md`, and commit neither the photos nor the fitted brand LUTs. Name looks "Fujifilm …" and "Leica …" in the app's `luts` folder, which is what puts them under Camera styles. Quality bar: held-out median ΔE ≤ 3 on at least 5 frames; otherwise label approximate or leave out. Fits come from another camera's RAWs, so the owner also checks them by eye. Sources tried for Fuji, and why most were skipped, are in that log.

## How the owner works

- **Keep going without asking** to the next plan task, unless their input or a manual check is critical. Commit and push per task once checks pass. Batch UI checks so they can verify several at once.
- **Test on `~/Desktop/2026-10-23_10261023`** or temp copies. **Tests never write to `/Volumes`.**
- **Reading the NAS needs their go-ahead** each time it's for a new purpose. The share is usually unmounted. Writing to it only with them present (M2-11).
- **English only.** UI text goes in `src/pv/i18n/en.json` keys; `npm run pv:i18n-check` enforces it. No other locales.
- **Raise anything expensive or Windows-only** rather than building it quietly.
- **Hard rule:** never delete or drop a RAW without human confirmation.
- **Effort:** Opus, High for tasks that write or delete user files. The owner sets this in the app.
- The owner has seen that images I open are sent to Anthropic, and I told them I'd switch to numbers only if they ask. They haven't asked. Don't open personal photos more than needed.

## Running things

```bash
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
```

- **Dev app:** `npm start` (`tauri dev`). When this was written it was running from an earlier session (log in that session's temp folder), so a new session may need to start it.
- **Rust checks, from `src-tauri`, always with their own build folder while the dev app runs:**

```bash
CARGO_TARGET_DIR=target/pv cargo test --lib pv::
```
```bash
CARGO_TARGET_DIR=target/pv cargo clippy --lib --tests
```

Sharing `target/debug` with `tauri dev` corrupted the build twice (`symbol(s) not found`; the app stops running). To recover: `cargo clean -p RapidRAW`, then touch a Rust file so `tauri dev` rebuilds (about 95 s). It also happened on 2026-11-06 with every build of mine in `target/pv`, during a merge and a quick run of Rust edits, so `tauri dev` restarting its build repeatedly seems to be enough to cause it. The first build in `target/pv` is slow, about 15 GB. **Before asking the owner to try something, confirm the app actually rebuilt and is running.**

- **Frontend:** `npx tsc --noEmit -p .` must stay at the **64-error upstream baseline**. Then `npx eslint src/pv`, `npx prettier --write src/pv`, `npm run pv:i18n-check`.
- **Real-file tests:** `PV_FIXTURES=$HOME/Desktop/2026-10-23_10261023 CARGO_TARGET_DIR=target/pv cargo test --lib pv::lifecycle::tests:: -- --nocapture`

## Where things are

| What | Where |
|---|---|
| New Rust code | `src-tauri/src/pv/`. `lifecycle/` moves files, `edit.rs` is Done and Revert, `look.rs` fits looks, `look_job.rs` is the headless render job, `signals.rs` RAW value, `slim.rs`, `history.rs`, `looks.rs` the panel catalogue |
| New frontend code | `src/pv/` |
| Edits to upstream | Small hooks marked `// PV:`. Keep it that way |
| Bundled film looks | `src-tauri/resources/pv_film_luts/` (20 + licence + `CHANGELOG.txt`). Upstream's six in `film_luts` stay in the Effects LUT control |
| Bake film looks | `scripts/pv-bake-film-looks.py`. Exposure per stock is in its `EV` table; re-bakes take seconds |
| Sample pairs from the NAS | `scripts/pv-sample-look-pairs.py` (read-only). `scripts/look-neutral.json` is the neutral recipe |
| App data | `~/Library/Application Support/com.danielxia.pairview/`: catalog, settings, `luts/My Sony.cube`, `pairview-edit-history.jsonl` |
| NAS | `/Volumes/Public/Photos/A/DCIM`, `smb://qnap-home._smb._tcp.local/Public`, 158 shoots |

**`My Sony.cube` and the `Fujifilm …` cubes exist only in the app data folder,** plus the temp copies below. Refitting needs the NAS again, or the temp copies; the Fuji frames can be fetched again from `docs/M3-look-sources.md`.

**Temp data from this session, which may vanish on reboot:** `/private/tmp/claude-501/-Users-xpy-Code-Camera-Photo-Library/91845d97-6a26-4f35-bf43-5fb9389def2d/scratchpad/`
- `lookfit/`: 60 pairs and their renders, 3.2 GB. Delete when M3 closes.

And from the M3-13 session, `/private/tmp/claude-501/-Users-xpy-Code-Camera-Photo-Library/886f801d-b267-4c1a-a969-c0a99819db6b/scratchpad/fuji/`: `frames/` (one folder per source), `work/` (cached renders), `manifest.tsv`, `cubes/`, about 4 GB. Delete when M3 closes.

From the earlier session, also:
- `sf/`: Spektrafilm clone and a Python 3.13 environment. To rebuild: clone the repo, then a Python 3.13 venv with its runtime dependencies and `pip install --no-deps -e .`. Skip its GUI dependencies. There's no `uv` or `conda` on this Mac; `uv` was installed inside a venv.

## State of the test data

The Desktop shoot isn't pristine, because the owner has been testing on it:
- `DSC03533`: **ARW in Recently Deleted**, so it's HIF only. Good for testing edits without a RAW.
- `DSC03536`: edited. The render is the old 23.9 MB one at quality 0.9, the camera file is in Replaced originals. Good for testing Revert.
- `DSC03534`: edited.
- `DSC03537`: untouched pair.
- The catalog also has a location for one NAS shoot, `2026-10-31_10561031`, which shows as offline when the share is unmounted.

## Traps found the hard way

- **`DynamicImage::thumbnail` corrupts floating-point images** (mean 0.10 became 0.60). Use `pv::thumbs::shrink`.
- **Measure the way the app runs.** My first Auto fix passed because the test shrank the rendered output, while the app shrinks a linear input.
- **RapidRAW's LUT loader rejects any path containing `..`,** and a LUT that fails to load now renders as no look. Don't pass such paths to the job.
- **`spektrafilm-lut` zeroes exposure compensation in LUT mode** (`digest_params`). The bake script sets it afterwards. Check by reading the LUT's gray axis.
- **Recipes without a saved look show My Sony only through the frontend default.** A saved recipe keeps its own look, or none.
- **Upstream merges conflict rarely,** and so far only near `// PV:` lines.
- **Near-black frames flatter a look's score,** and public RAWs lean towards difficult night shots. Keep the brightness gate (`scripts/pv-style-gate.py`).
- **Holding out a frame isn't enough if its photographer stays in training:** Astia went from 1.18 to 2.26 when they were held out too.
- **Pillow reads 16-bit RGB PNGs as 8-bit,** so the job's cached PNGs divide by 255 in Python.
- **Read a source's robots.txt in full before the first automated request.** Photography Blog's reserved its rights below the 25 lines I printed, and it was surveyed before that was noticed.

## Not done, on purpose

- Recently Deleted's Restore on a Replaced original refuses because the name is taken. Revert is the way back; a hint says so.
- Undo covers deletes and slims, not overwrites or rating changes.
- Revert refuses after the 30-day purge instead of re-rendering in another look.
- Renames and moves still go through upstream (pair-safe rename is M5).
