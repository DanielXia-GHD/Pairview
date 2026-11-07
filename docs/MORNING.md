# For the owner: morning of 2026-11-07

Written overnight. Delete this file once it's done with.

## What happened overnight

Built the first pass of **AI triage (M4)**: bursts, the best of each, suggested rejects and a dimmed AI rating. [Details](M4-triage.md).

- **It's on `main` and in your dev app**, but **nobody has seen the screens yet**: the type checks and tests pass, and nothing here can click them.
- **It changes nothing in your library.** No file, rating or flag. It writes only to the rebuildable catalog, and only when you press the button.
- The catalog moved to schema version 5, tried first on a copy of yours. An older build will refuse to open it, so going back to an older commit means rebuilding the catalog from disk.

## Try AI triage (5 minutes)

1. Open `2026-10-23_10261023` in the library.
2. Press **AI triage** in the header, next to Casual / Normal / Important. It should read "AI: 0 to reject · 0 bursts" almost at once.
3. Tiles should show a dimmed "AI ★★" at the bottom left, apart from your own stars. Hover for the reason.
4. Filter menu → **Filter by AI suggestion**: All, Rejects, Best, Bursts. With this shoot, Rejects, Best and Bursts should be empty. Set it back to All.
5. Try it on a shoot with bursts, if you have one locally. **This matters most**: burst grouping has not been tested on a real burst.

Things to look for: the button or the tile badges looking wrong or getting in the way, and whether the AI's picks make any sense. AI ★ is mostly a sharpness rank within the shoot, so expect it to be crude.

## Decisions for you

| Question | My suggestion |
|---|---|
| May I read one NAS shoot with bursts, to test burst grouping? Read-only | Yes. It's the biggest unknown, and you can name a shoot |
| Download face and CLIP models, for eyes closed, shoot intent guess and "document" detection? They're upstream's models, several hundred MB | Yes, after you've seen the first pass |
| Write AI results into `pairview:ai*` XMP, as the PRD says? That's one sidecar write per Photo, including on the NAS | Not yet. The catalog is enough until the suggestions earn it |

## Quick checks left from last night

- [ ] Delete and restore a photo on the Desktop shoot
- [ ] Interface scale in Settings, including 100% zoom in the viewer
- [ ] Ctrl+click context menus in the grid

## Waiting on you

- [ ] **Fujifilm looks:** try Provia, Astia, Classic Chrome and Velvia under Camera styles, on your own photos. Skies, greens, skin. Nothing else can measure these on the Sony
- [ ] **Film looks (M3-15):** which ones feel off, and how
- [ ] **Leica (M3-14):** your friend's Xiaomi test: 3 scenes in Leica Authentic and Vibrant, RAW + JPEG, original files, in `~/Desktop/leica-xiaomi/<look>/`
- [ ] **Sony Creative Looks (M3-12):** 3–5 RAW + HIF scenes for each of ST, PT, NT, VV, VV2, FL, IN, SH, BW, SE
- [ ] **NAS half of M2-11:** needs you present with the share mounted

If the editor shows a fragment of a photo on black again, tell me before restarting the app.
