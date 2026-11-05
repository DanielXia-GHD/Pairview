# M1 exit check (M1-18)

Date: 2026-11-04 · MacBook M2, 16 GB · NAS `smb://qnap-home.local/Public`, root `/Volumes/Public/Photos/A/DCIM` (158 shoot folders).
Measured with `src-tauri/src/pv/nas_check.rs` (read-only; in-memory catalog), run from a cold start (share freshly mounted), while the app was idle. Second run after fixes 1–2 below.

## Performance against PRD §9

| Target | Result | |
|---|---|---|
| Open a 100-photo day, all thumbnails < 5 s cold | **4.5 s** / **5.4 s** for 95 photos over two cold runs (`2026-10-31_10561031`, ARW previews, 8 parallel); 1.3–1.5 s warm | ⚠️ on the line |
| Viewer: cold single HIF ≤ 0.6 s | Read 0.13–0.17 s + full 7008 px decode 0.26–0.29 s = **0.40–0.46 s**; the first decode after start 0.72 s (ImageIO warm-up) | ✅ (first photo ⚠️) |
| Viewer next/prev < 100 ms with prefetch | Prefetch 2 ahead covers it (owner walkthrough) | see below |
| L1 metadata index, whole NAS root ≤ 5 min first run | **22 min** (1331 s) at first; **15 min** (915 s) after fix 2, for 16,891 files, 4.4 GB, 0 failed. Floor on this NAS ≈ 7 min (see below) | accepted (one-time) |
| L0 listing of the whole root (no PRD target) | 160 s / 148 s for 21,880 files in 483 folders; pairing 0.5 s | ⚠️ slow first run |
| Memory < 1.5 GB | Owner reads Activity Monitor during the walkthrough | see below |

Read probe on the NAS (uncached reads, `F_NOCACHE`, 160 different files per row):

| Read per file | 1 thread | 16 threads |
|---|---|---|
| 4 KB | 24 ms | 25 ms |
| 32 KB | 41 ms | 10–25 ms |
| 256 KB | 40–50 ms | 25–60 ms |

Throughput stays at 4–10 MB/s whatever the thread count. Each file costs a fixed ~25 ms at the NAS (opening it and seeking the disk) and parallel requests queue there, so 16,891 files can't go below ~7 min while every file is read. The 77 MB/s figure is for sequential reads only.

## Pairing on the real archive

- Before fix 1: **14,457 Photos** in 157 shoots. After fix 1: **9,673 Photos**, 7,214 RAW + display pairs, 2,261 RAW only, 198 display only. No uncertain dates, no timestamp mismatches.
- `2025-06 … 2025-09` and `2026-06 … 2026-10` (ARW + HIF): every RAW paired (e.g. `2026-10-31_10561031`: 95/95, `2026-10-24_10361024`: 93/93).
- **`2025-09-27 … 2026-05-31` (ARW + JPG): 0 pairs.** The JPGs carry a counter after the camera number: `DSC07714.ARW` ↔ `DSC07714439.jpg`, `DSC03765.ARW` ↔ `DSC037651136.jpg`. Exact-name pairing showed each Photo twice. Fixed (fix 1): every shoot in this range now pairs fully (e.g. `2025-09-27_14250927`: 21/21).
- Archive layout the tree shows as-is: `新建文件夹` ("New folder") sub-folders inside `arw/`, `arw/01 … arw/11` split folders, `(w)` shoots with RAWs only.
- SMB identity: `smb://qnap-home._smb._tcp.local/Public` ✅ (what the location is recognised by after a remount).

## Owner walkthrough

Owner, 2026-11-04: quick pass in the dev app, "looks good". Done on the single-shoot location (`2026-10-31_10561031`), not the whole DCIM root.

| Step | Result |
|---|---|
| Add `/Volumes/Public/Photos/A/DCIM` as a location | Not done in the app; the whole root was scanned, paired and indexed by `nas_check.rs` (above) |
| Browse tree, date view, view and rate, slideshow | ✅ on the single shoot |
| Ratings in `.xmp` and in RapidRAW's editor | ✅ (owner quick pass) |
| Memory (Activity Monitor) | Not measured with the whole root; recheck when it is added |

## Fix list

| # | Issue | Proposal | Triage |
|---|---|---|---|
| 1 | ARW + suffixed JPG (`DSC07714` + `439.jpg`) not paired | Pair a display file whose name is a RAW's name plus digits, in the same shoot, when the capture-time guard agrees | fixed |
| 2 | L1 first run 22 min vs 5 min | Run L1 over all pending files in one pool instead of shoot by shoot; read the representative only (skip the display copy's EXIF when the RAW has it) | one pool + 16 reads: 22 → 15 min. Rest is NAS per-file latency. **Accepted** (owner, 2026-11-04): runs once per file, in the background; later scans read only new or changed files |
| 3 | L0 first listing 160 s | Profile per-folder cost on SMB (stat per file?) | |
| 4 | Viewer `?jpeg` fallback served the 1620 px embedded preview | Fixed: renders from the full image when the preview is smaller than asked | fixed |
