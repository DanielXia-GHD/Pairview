# M0 — Mac spike findings

Date: 2026-11-01 · Machine: MacBook M2, 16 GB, macOS 27.2 · Library: NAS over Wi-Fi 6 (`/Volumes/Public/Photos/A/DCIM`)
Samples: owner's A7C II shoots `2026-10-18_10161018`, `2026-10-23_10261023`, `2026-10-24_10361024`, `2026-10-31_10561031` (234 ARW+HIF pairs).

Spike tool: [`spikes/m0/imageio_spike.swift`](../spikes/m0/imageio_spike.swift) (`decode`, `thumbs`, `encode`, `pair`). Build: `swiftc -O spikes/m0/imageio_spike.swift -o /tmp/imageio_spike`.

## Results

| # | Spike | Result | Verdict |
|---|---|---|---|
| 1 | **HIF decode in RapidRAW** (new `src-tauri/src/heif.rs`, ImageIO) | 33 MP, **10-bit** kept (16-bit buffer → `ImageRgb32F`). Full decode **0.41–0.51 s**. Thumbnail (1280 px) **32–40 ms** once bytes are in memory. Portrait/landscape orientation correct (taken from the HEIF container via ImageIO, **not EXIF**; the first version got this wrong and the test caught it). Test: `PV_HIF_SAMPLE=… cargo test --lib heif` | ✅ |
| 2 | **10-bit HEIF encode** (ImageIO) | Re-encode from a 16-bit buffer: **354 ms**, readback **depth=10**, ~10 MB, `DateTimeOriginal`+subsec, camera model and color profile preserved | ✅ "Overwrite HIF in its own format" (D3) is viable on Mac |
| 3 | **Grid thumbnails for a whole day from the NAS** (uncached, idle CPU) | **ARW embedded preview, 8 parallel: 4.7 s / 95 photos** ✅. HIF via ImageIO, 8 parallel: **12.9 s / 95 photos** ❌. Even at 160 px, ImageIO reads the **whole ~10 MB HIF**, so HIF thumbnails are bound by NAS bandwidth (95 × 10 MB at 77 MB/s ≈ 12 s) | ⚠️ → **Design change:** thumbnails for pairs come from the **ARW's embedded preview** (small range read). The HIF is used only for HIF-only or edited Photos, or later via a custom HEIF box parser that range-reads just the thumbnail item |
| 4 | **Pairing across `arw/` + `hif/`** | 4 shoots, **234 / 234 paired**, 0 RAW-only, 0 mismatches. Folder listing ~0.3 s per shoot. The EXIF timestamp guard costs ~100 ms per file sequential over SMB | ✅ Basename pairing is reliable. The guard runs in the background (parallel, cached) and is enforced strictly only before destructive actions |
| 5 | rawler support for A7C II | `a7cm2.toml` present in RapidRAW-DngLab | ✅ |
| 6 | NAS throughput | ~77 MB/s (42 MB ARW in 0.55 s) | ✅ Never read full files while browsing |
| 7 | **RapidRAW builds on M2** | Rust 1.98 via Homebrew `rustup` (keg-only: `export PATH=/opt/homebrew/opt/rustup/bin:$PATH`). Debug build ~1m40 incremental; debug + release `target/` uses **~13 GB** (disk is now down to 33 GB free). `npm install` OK | ✅ |
| 8 | **Unsigned `.dmg`** (local) | `npm run tauri build -- --bundles dmg`: 12 min release build → `RapidRAW_1.6.7_aarch64.dmg` (32 MB), arm64, **ad-hoc signed** (opens via right-click → Open). Still RapidRAW-branded: Pairview branding is the next M0 task | ✅ |
| 9 | XAVC HS 4:2:2 video playback | No sample clip yet | ⏸ Needs a sample |
| 10 | CI | Upstream `ci.yml` builds ~10 platform variants (incl. Linux and Android) on every push. Upstream workflows are disabled in the fork's Actions settings (2026-11-03). `pairview-ci.yml` (macOS arm64 `.dmg`, manual or `pv-v*` tag) is the only active workflow | ⏳ First manual run pending |

**Running-app check (2026-11-02, M0-02):** `npm start` with the owner browsing a local copy (`~/Desktop/2026-10-23_10261023`). The log shows HIF files decoding through ImageIO, portrait 4672×7008, in **0.47–0.50 s**. ARWs open in the editor. Two log errors: the update check finds no Pairview release yet (expected; to be silenced), and one upstream `Preview worker error: No original image loaded` while switching images quickly (watch). The grid still shows ARW and HIF as separate tiles until M1 (Photo grid).

**More findings (2026-11-02):**
- **`kamadak-exif` cannot read EXIF from Sony HIF files** (it reads Apple-written HEIF fine). RapidRAW's EXIF panel and date sorting will be empty for HIFs → M1-05 must read HIF metadata through ImageIO (`heif.rs` already has the property plumbing).
- **10-bit HEIF encode in Rust** (`pv::heif::encode_heif`): round-trip test passes on 4 samples (size, orientation, depth 10, `DateTimeOriginal`, model). Orientation is reset to 1 because decoded pixels are already upright.
- **Disk:** macOS reports 130 GB available for apps (31 GB plain free + purgeable). No manual purge needed.
- **TypeScript baseline:** upstream already has 65 `tsc` errors; the gate is "no new errors in touched files".

## Code changes (all marked `// PV:`)

| File | Change |
|---|---|
| `src-tauri/src/heif.rs` (new) | ImageIO decode (16-bit, orientation) + fast thumbnail; non-macOS stubs; sample-file test |
| `src-tauri/src/lib.rs` | `mod heif;` |
| `src-tauri/src/formats.rs` | `hif`, `heif`, `heic` added to non-RAW extensions |
| `src-tauri/src/image_loader.rs` | Non-RAW branch routes HEIF to `heif::decode_heif` |
| `src-tauri/src/file_management.rs` | Thumbnail fast path for HEIF |
| `src-tauri/Cargo.toml` | `objc2-image-io` + extra CoreFoundation/CoreGraphics features (macOS only) |

## Design consequences (to fold into the PRD)

- **Thumbnail source per Photo:** unedited pair → ARW embedded preview; edited pair → rendered HIF (cached once after edit); HIF-only → ImageIO thumbnail. All cached locally, so the second visit is instant.
- **Grid from folder listings first:** basename pairing needs only directory listings (~0.3 s). EXIF (dates, guard) streams in the background at 8× parallelism and is cached.
- **Viewer:** full-res HIF decode ≈ 0.4–0.5 s plus ~0.12 s NAS read, so prefetching the next 2 photos is required for instant ←/→.
- **Future optimization:** a HEIF box parser that range-reads Sony's embedded preview item would make HIF-only thumbnails as cheap as ARW previews.
