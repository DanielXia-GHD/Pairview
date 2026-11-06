# Public samples used for camera styles (M3-13, M3-14)

Rules are in [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) under the M3 table. In short: RAW and JPG files only, terms read per source, a source that forbids this use is skipped, used locally, and neither the photos nor the fitted brand LUTs are committed.

Fuji RAFs carry the camera's own JPEG with the film mode in its EXIF, so one file gives both halves of a pair. `scripts/pv-raf-survey.py` reads the settings with a small range request, so only frames that were actually wanted got downloaded.

## Sources

| Source | Terms as read, 2026-11-06 | Decision |
|---|---|---|
| [raw.pixls.us](https://raw.pixls.us/) | CC0, "for any projects that need access to a library of raw files". Its robots.txt tells crawlers to stay out of `/data/`, while its front page publishes `/data/` and git clones as the way to download. Read as aimed at search crawlers | **Used.** Targeted downloads only |
| [discuss.pixls.us Play Raw](https://discuss.pixls.us/) | Each post states a licence for its raw file. Accepted: CC BY-SA, BY-NC, BY-NC-SA. Fitting locally and not redistributing fits all three. Each frame's licence was checked, some by reading the post | **Used** |
| Imaging Resource | Terms forbid downloading content, automated access, and AI training | Skipped |
| MirrorLessons | Use or duplication without written permission is prohibited | Skipped |
| Fstoppers article (John Ricard's files) | "No reproduction rights are granted" | Skipped |
| PetaPixel sample galleries | Search excerpt limits downloads to viewing and evaluating cameras; members' files. The terms page itself refused the fetch | Skipped |
| Photography Blog | No terms page found, but robots.txt declares `ai-train=no, use=reference` as a reservation of rights and blocks Anthropic's named crawlers. A LUT fit isn't exactly AI training, but it is a statistical model built from the images | Skipped. See below |
| Have Camera Will Travel | Refused the fetch, so its terms couldn't be read | Skipped |
| DPReview | Refused its terms page and robots.txt, so its current terms couldn't be read. An old gallery page asks not to republish | Skipped |
| Internet Archive | The two Fuji items found state no licence | Skipped |

**Photography Blog, what happened.** The header survey ran before its robots.txt was read in full: the same command fetched the file and ran the survey, with the output cut off above the relevant line. 1,703 RAF URLs were requested at about 90 KB each. 479 answered, then the site returned 403 for the other 1,224, which was not retried. No full RAW was downloaded, nothing from it was used, and the settings read from those headers have been deleted. Read a source's robots.txt completely before the first automated request.

**discuss.pixls.us.** Eight search queries went through `/search`, which its robots.txt disallows, and hit a rate limit, before that was noticed. Everything after that used category listings and topic pages, which are allowed. `scripts/pv-playraw-harvest.py` only uses those.

**Excluded by hand.** Play Raw topic 9571 is from an infrared-converted camera, and the two "Analog Scanning Exercise" topics are photos of film negatives. No filter would have caught either.

**Coverage.** All of Play Raw was searched, about 2,100 topics. It holds no more than 2 scenes each of Acros, Eterna, Reala Ace, Classic Neg and Nostalgic Neg.

## Frames behind the installed looks

71 frames: 24 CC0, 47 from Play Raw. "Training only" frames have mild tone settings, so they help the fit but don't score it.

### Provia

| Camera | Frame | Scored | Source and licence |
|---|---|---|---|
| X-T2 | `bbNSVQM3mTipZZ3hJkcRMNWp` | yes | [Play Raw 54213](https://discuss.pixls.us/t/54213), piratenpanda, CC BY-SA 4.0 |
| X-E4 | `fuji_xe4_32_lossless.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-E4/fuji_xe4_32_lossless.RAF), CC0 |
| X-E2S | `_DSF0540.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-E2S/_DSF0540.RAF), CC0 |
| GFX 100 | `_DSF3977.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/GFX%20100/_DSF3977.RAF), CC0 |
| X-T5 | `DSCF0021.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T5/DSCF0021.RAF), CC0 |
| X-H2S | `tbJx3TkISJd4XtNFHohTv4z8` | yes | [Play Raw 49430](https://discuss.pixls.us/t/49430), lockdown, CC BY-SA 4.0 |
| X-H1 | `6SNZVfz4ahddbBWva55wFYEz` | yes | [Play Raw 36881](https://discuss.pixls.us/t/36881), BenBeau, CC BY-SA 4.0 |
| GFX 50S | `20170525_0036TEST.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/GFX%2050S/20170525_0036TEST.RAF), CC0 |
| X-A5 | `DSCF0617.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-A5/DSCF0617.RAF), CC0 |
| X-T20 | `wqZfM7gyjeEmea3KhstMkCtZ` | yes | [Play Raw 24515](https://discuss.pixls.us/t/24515), atmbl, CC BY-NC-SA 3.0 |
| X-T20 | `l4K7RK4OwG8P106a5cKSwsxv` | yes | [Play Raw 15020](https://discuss.pixls.us/t/15020), steve-d-mottram, CC BY-SA |
| X-T20 | `DSCF0526.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T20/DSCF0526.RAF), CC0 |
| X-T4 | `27OQxWb11WghIO70kutM9E92` | yes | [Play Raw 48661](https://discuss.pixls.us/t/48661), gigaturbo, CC BY-SA 4.0 |
| X-A2 | `DSCF2073.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-A2/DSCF2073.RAF), CC0 |
| X-H1 | `2SyffuVw4gDdzK6y44m76mdo` | yes | [Play Raw 41468](https://discuss.pixls.us/t/41468), Leniwiec, CC BY-SA 4.0 |
| X-E3 | `x4eOURoYNmW8WTAQMPr8v1As` | yes | [Play Raw 28995](https://discuss.pixls.us/t/28995), ariznaf, CC BY-NC-SA 4.0 |
| X-T3 | `iFfvoNpIaflkCbCMHyOCgDhs` | yes | [Play Raw 29455](https://discuss.pixls.us/t/29455), hatsnp, CC BY-SA 4.0 |
| X-T4 | `o4G2CAJqBGap7dJayaHTqIDa` | yes | [Play Raw 52337](https://discuss.pixls.us/t/52337), huge_raw_tinkerer, CC BY-NC-SA 4.0 |
| X100T | `DSCF0442.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X100T/DSCF0442.RAF), CC0 |
| X-T10 | `DSCF8146.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T10/DSCF8146.RAF), CC0 |
| X-T20 | `veoZLp7VyeX85niYAQTltL2K` | yes | [Play Raw 36950](https://discuss.pixls.us/t/36950), lockdown, CC BY-SA 4.0 |
| X-E2 | `2uLGjlcAuGzLPPc0jDDqvrXY` | yes | [Play Raw 45206](https://discuss.pixls.us/t/45206), grega, CC BY-NC 4.0 |
| X-T5 | `3QL8NL41rEJUowGjH9eZRMdn` | yes | [Play Raw 58406](https://discuss.pixls.us/t/58406), carstenmann, CC BY-SA 4.0 |
| X-T3 | `4VcZTwK8yGhxjxQ934bH57Uw` | yes | [Play Raw 22024](https://discuss.pixls.us/t/22024), Sebastian_Nowozin, CC BY-SA 4.0 |
| X-E2 | `me4Tu7rZ1inbTa7cGdAKIsuS` | yes | [Play Raw 25262](https://discuss.pixls.us/t/25262), grega, CC BY-NC 4.0 |
| X-T30 | `z8agI3sRcl7dMyOOcqHdXS5W` | yes | [Play Raw 24168](https://discuss.pixls.us/t/24168), Toand007, CC BY-SA 4.0 |
| X-T30 II | `compressed_X-T30II.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T30%20II/compressed_X-T30II.RAF), CC0 |
| X-T4 | `s684YkeZTDJYnqM6a7A4U6YK` | yes | [Play Raw 48460](https://discuss.pixls.us/t/48460), grega, CC BY-NC 4.0 |
| X-T30 | `4HvLHAdlOsVjdwlABNf5lRLJ` | yes | [Play Raw 23885](https://discuss.pixls.us/t/23885), Toand007, CC BY-SA 4.0 |
| X-E2 | `7KoMnPi7UTMIchLaBFt1woJq` | yes | [Play Raw 25113](https://discuss.pixls.us/t/25113), grega, CC BY-NC 4.0 |
| X-T30 | `DSCF0065.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T30/DSCF0065.RAF), CC0 |

### Astia

| Camera | Frame | Scored | Source and licence |
|---|---|---|---|
| X-T30 | `kzQqkauqYKD2yLsfa62UMGqJ` | yes | [Play Raw 50162](https://discuss.pixls.us/t/50162), commutergraphics, CC BY-SA 4.0 |
| X-T3 | `1WElPLopPy68VmYTOqmIClIb` | yes | [Play Raw 43267](https://discuss.pixls.us/t/43267), sguyader, CC BY-SA 4.0 |
| X-T4 | `6xvaL8YfaCe1OZyuKxs8TKrk` | yes | [Play Raw 48246](https://discuss.pixls.us/t/48246), gigaturbo, CC BY-SA 4.0 |
| X-T4 | `aAQidXpaF5tvwjnAsd0SGDbf` | yes | [Play Raw 49538](https://discuss.pixls.us/t/49538), gigaturbo, CC BY-SA 4.0 |
| X-T4 | `qQEmoLm9RcXKgwdb9d77F3mP` | yes | [Play Raw 44714](https://discuss.pixls.us/t/44714), gigaturbo, CC BY-SA 4.0 |
| X-T4 | `nhXGiOwJnx5Wf9cKM4fQTweO` | yes | [Play Raw 44714](https://discuss.pixls.us/t/44714), gigaturbo, CC BY-SA 4.0 |
| X-T3 | `4s5vNqT54SIT2enZdwZ5eJMS` | yes | [Play Raw 31085](https://discuss.pixls.us/t/31085), hatsnp, CC BY-SA 4.0 |
| X-T3 | `8WV9WH9Dikiu2hthA1SD1egA` | yes | [Play Raw 32573](https://discuss.pixls.us/t/32573), hatsnp, CC BY-SA 4.0 |
| X-T3 | `bRN1hIm43svBRPom3FVCU9W6` | training only | [Play Raw 29736](https://discuss.pixls.us/t/29736), hatsnp, CC BY-SA 4.0 |
| X-T20 | `acYkMZmwry3ixB9kxNVexF3N` | yes | [Play Raw 13619](https://discuss.pixls.us/t/13619), chaimav, CC BY-NC-SA |
| X-T4 | `e5rLbpIoO9uOsJd6py2Fpsz8` | yes | [Play Raw 60757](https://discuss.pixls.us/t/60757), luator, CC BY-NC-SA 4.0 |
| X-T20 | `j5MLdBTdC34oZhyinVLuGhGl` | training only | [Play Raw 37215](https://discuss.pixls.us/t/37215), chaimav, CC BY-NC-SA |
| X-T20 | `oclCEocIjvg23oRFaTX5VljK` | training only | [Play Raw 40721](https://discuss.pixls.us/t/40721), chaimav, CC BY-NC-SA 4.0 |

### Classic Chrome

| Camera | Frame | Scored | Source and licence |
|---|---|---|---|
| X-H1 | `o2yHLUyirXAjgLGnCL72t6th` | yes | [Play Raw 21649](https://discuss.pixls.us/t/21649), lhutton, CC BY-SA 4.0 |
| X-A3 | `DSCF7241.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-A3/DSCF7241.RAF), CC0 |
| X-T4 | `bA2YZmCgU7oCfETgcsZ9LO67` | yes | [Play Raw 37070](https://discuss.pixls.us/t/37070), davidvj, CC BY-SA 4.0 |
| X-E2 | `DSCF9050.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-E2/DSCF9050.RAF), CC0 |
| X-T2 | `20170114_173532_TFW04723` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T2/20170114_173532_TFW04723.RAF), CC0 |
| X-H1 | `sJME1j2m58gsXyf4K7QYzpj2` | yes | [Play Raw 20711](https://discuss.pixls.us/t/20711), scrawfor, CC BY-SA 4.0 |
| GFX100S | `Fujifilm-GFX100S-14bits-` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/GFX100S/Fujifilm-GFX100S-14bits-compress-4_3.RAF), CC0 |
| X100VI | `blKUi6njwv0vg6HufbHxqOTB` | yes | [Play Raw 61083](https://discuss.pixls.us/t/61083), Marien_GB, CC BY-NC-SA 4.0 |
| X100F | `cvk6KNaTxEXeVomgs3Qns0b7` | training only | [Play Raw 25624](https://discuss.pixls.us/t/25624), deanc, CC BY-SA 4.0 |
| X-T4 | `eb4ER6Sn82XoGDipsdkODyZB` | training only | [Play Raw 44568](https://discuss.pixls.us/t/44568), luator, CC BY-NC-SA 4.0 |

### Velvia

| Camera | Frame | Scored | Source and licence |
|---|---|---|---|
| X-H2 | `Fuji-X-H2-Compressed-Raw` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-H2/Fuji-X-H2-Compressed-Raw.RAF), CC0 |
| FinePix X100 | `4zO3gUO9yo60u3OYrB32fgnA` | yes | [Play Raw 45637](https://discuss.pixls.us/t/45637), boredphoto, CC BY-SA 4.0 |
| X-T200 | `DSCF0074.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T200/DSCF0074.RAF), CC0 |
| X-T100 | `DSCF0627.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-T100/DSCF0627.RAF), CC0 |
| X100V | `DSCF0733.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X100V/DSCF0733.RAF), CC0 |
| X-A10 | `DSCF8207.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-A10/DSCF8207.RAF), CC0 |
| X100F | `yIs9BzosLsHuSBm4DUOvVWyS` | yes | [Play Raw 22351](https://discuss.pixls.us/t/22351), arctic, CC BY-SA 4.0 |
| X-E1 | `DSCF6490.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-E1/DSCF6490.RAF), CC0 |
| X-T5 | `xTO3aI83gw4qqcdk7IpReCuT` | yes | [Play Raw 53063](https://discuss.pixls.us/t/53063), T_W, CC BY-SA 4.0 |
| X-T30 | `9rZYIrvOQQsIxz98UzQodmmk` | yes | [Play Raw 19738](https://discuss.pixls.us/t/19738), prokoudine, CC BY-SA 4.0 |
| X-H1 | `fuji_xh1_compressed.RAF` | yes | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-H1/fuji_xh1_compressed.RAF), CC0 |
| X-T4 | `22WzucjIucTUGsbRYGpDwTgL` | yes | [Play Raw 45739](https://discuss.pixls.us/t/45739), Iksou, CC BY-NC |
| X-Pro1 | `DSCF2131.RAF` | training only | [raw.pixls.us](https://raw.pixls.us/data/Fujifilm/X-Pro1/DSCF2131.RAF), CC0 |
| X-T30 | `qETEEkyluukbNlp4eonUx500` | training only | [Play Raw 26660](https://discuss.pixls.us/t/26660), Ori, CC BY-SA 4.0 |
| X-T1 | `kXfi9m7chxdPrOlHdFdMmLjk` | training only | [Play Raw 31677](https://discuss.pixls.us/t/31677), Leniwiec, CC BY-SA 4.0 |
| GFX100RF | `9ylYUDiEYcu84FwPZuohj9TN` | yes | [Play Raw 57921](https://discuss.pixls.us/t/57921), Peter3, CC BY-SA 4.0 |
| X-T1 | `cGXoZeBiah1J9FXfyzq5YBRe` | yes | [Play Raw 28682](https://discuss.pixls.us/t/28682), Leniwiec, CC BY-SA 4.0 |
