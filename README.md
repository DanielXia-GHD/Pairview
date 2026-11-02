# Pairview

A photo library, viewer and AI-assisted editor for **macOS (Apple Silicon) first, then Windows**. It treats a RAW + HIF/JPG pair as **one photo**: view, rate, cull, edit (from the RAW, automatically), decide whether the RAW is worth keeping, and delete, all without the two files drifting apart.

Built as a personal-use fork of [**RapidRAW**](https://github.com/CyberTimon/RapidRAW) by Timon Käch, a GPU-accelerated, non-destructive RAW editor. All of RapidRAW's editing features are inherited and upstream changes are merged regularly.

> **Status:** PRD v0.3 complete; feature work starts with milestone M0 (Mac spikes).

## What this fork adds (planned)

- **Photo = RAW + display copy.** `DSC03545.ARW` + `DSC03545.HIF` (even across `arw/` and `hif/` subfolders) is one tile, one rating, one delete.
- **First-class Sony HIF** (10-bit HEIF) decode and edited-output encode.
- **Keep-RAW workflow.** AI triage first, then your confirmation. Explainable "Keep RAW / HIF is enough" suggestions, shoot intent, and a batch "Ready to slim" review that reclaims space safely.
- **Recently Deleted** on the same drive/NAS share, orphan-RAW detection, and undo.
- **Edit from RAW, overwrite the HIF/JPG.** "My Sony" camera look fitted from your own files, plus classic camera looks.
- **Pixea-style viewer:** same-window full screen, ←/→, slideshow. Date browsing next to the folder tree. NAS-friendly lazy indexing.
- One-click AI edit that **learns your style**, AI culling, SD-card import, search and map.

Full requirements: [**docs/PRD.md**](docs/PRD.md)

## Build from source

Requires [Rust](https://www.rust-lang.org/tools/install) and [Node.js](https://nodejs.org/).

```bash
npm install
```

```bash
npm start
```

Release build (produces the `.dmg` on macOS and the installer `.exe` on Windows under `src-tauri/target/release/bundle/`):

```bash
npm run tauri build
```

Tethering build (macOS/Linux): `brew install libgphoto2 pkg-config`, then `npm run start:tethering` or `npm run tauri build -- --features tethering`.

**System requirements** (inherited from RapidRAW): macOS 13+ or Windows 10+, 16 GB RAM recommended, a GPU from roughly 2015 or newer.

## Repository layout and upstream sync

This repository sits on top of upstream RapidRAW's git history, so upstream fixes (new cameras, editor features) can be merged directly.

| Remote | Points to |
|---|---|
| `upstream` | `https://github.com/CyberTimon/RapidRAW.git` |
| `origin` | `https://github.com/DanielXia-GHD/PhotoVeiwerLibrary.git` (this fork) |

Fork-specific code lives in its own modules. Upstream files only get small hook points marked with `// PV:` comments, to keep merges painless. To sync:

```bash
git fetch upstream && git merge upstream/main
```

`README.md` is marked `merge=ours` in `.gitattributes`, so upstream README changes never conflict. Enable the driver once per clone:

```bash
git config merge.ours.driver true
```

For RapidRAW's own feature list, supported formats, AI strategy, CLI and troubleshooting, see the [upstream README](https://github.com/CyberTimon/RapidRAW#readme).

## Credits & license

- [RapidRAW](https://github.com/CyberTimon/RapidRAW) by Timon Käch and contributors: the editor, processing pipeline and much of the library code this fork builds on.
- RAW decoding via [rawler / dnglab](https://github.com/CyberTimon/RapidRAW-DngLab), lens profiles via [Lensfun](https://lensfun.github.io/).

Licensed under the **GNU Affero General Public License v3.0 (AGPL-3.0)**, the same as upstream. See [LICENSE](LICENSE).
