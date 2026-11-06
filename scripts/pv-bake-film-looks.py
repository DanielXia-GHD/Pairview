#!/usr/bin/env python3
"""Bakes Pairview's film looks from Spektrafilm (docs/IMPLEMENTATION_PLAN.md M3-10, PRD D15).

A dev tool: the app only ships the resulting .cube files, never Python. Baked like the six stocks
RapidRAW bundles in resources/film_luts (V-Log in, sRGB out, one LUT, 25 points): with no exposure
compensation, this reproduces upstream's Portra 400 exactly.

Exposure: Sony RAWs keep highlight headroom the camera normally lifts, so at 0 EV these looks come
out about a stop darker than the camera's own files. Each is baked with the film exposure compensation
that lands it near the camera's brightness on average (`EV`, measured by pv::look_job::film).

    git clone https://github.com/andreavolpato/spektrafilm && cd spektrafilm
    uv venv --python 3.13 && uv pip install --no-deps -e . && uv pip install <its runtime deps>
    python pv-bake-film-looks.py <out dir>

Slide films are meant to be viewed directly, so they're baked as a scan of the film. The bake tool
requires a print profile even then; with `scan_film` on, the pipeline doesn't use it.
"""
import shutil
import sys
import tempfile
from pathlib import Path

import spektrafilm.runtime.params_builder as params_builder
from spektrafilm_lut_creator.builders import BundleBuilder
from spektrafilm_lut_creator.bundles import BundleSpec

CRYSTAL = "fujifilm_crystal_archive_typeii"
PORTRA = "kodak_portra_endura"
SUPRA = "kodak_supra_endura"
PREMIER = "kodak_endura_premier"
ULTRA = "kodak_ultra_endura"
CINEMA = "kodak_2383"

# Look name (the file name, shown in the panel) → film, print; None = slide, scanned directly.
LOOKS = {
    "F-C200": ("fujifilm_c200", CRYSTAL),
    "F-Pro 400H": ("fujifilm_pro_400h", CRYSTAL),
    "F-Provia 100F": ("fujifilm_provia_100f", None),
    "F-Velvia 100": ("fujifilm_velvia_100", None),
    "F-Xtra 400": ("fujifilm_xtra_400", CRYSTAL),
    "K-Ektachrome 100": ("kodak_ektachrome_100", None),
    "K-Ektar 100": ("kodak_ektar_100", ULTRA),
    "K-Gold 200": ("kodak_gold_200", SUPRA),
    "K-Kodachrome 64": ("kodak_kodachrome_64", None),
    "K-Portra 160": ("kodak_portra_160", PORTRA),
    "K-Portra 400": ("kodak_portra_400", PORTRA),
    "K-Portra 800": ("kodak_portra_800", PORTRA),
    "K-Portra 800 Push +1": ("kodak_portra_800_push1", PORTRA),
    "K-Portra 800 Push +2": ("kodak_portra_800_push2", PORTRA),
    "K-Ultramax 400": ("kodak_ultramax_400", SUPRA),
    "K-Verita 200D": ("kodak_verita_200d", PREMIER),
    "K-Vision 50D Cinematic": ("kodak_vision3_50d", CINEMA),
    "K-Vision 200T Cinematic": ("kodak_vision3_200t", CINEMA),
    "K-Vision 250D Cinematic": ("kodak_vision3_250d", CINEMA),
    "K-Vision 500T Cinematic": ("kodak_vision3_500t", CINEMA),
}

# Film exposure compensation per look, in EV.
EV = {
    "F-C200": 1.30,
    "F-Pro 400H": 1.25,
    "F-Provia 100F": 1.10,
    "F-Velvia 100": 1.15,
    "F-Xtra 400": 1.30,
    "K-Ektachrome 100": 1.15,
    "K-Ektar 100": 1.70,
    "K-Gold 200": 1.35,
    "K-Kodachrome 64": 1.20,
    "K-Portra 160": 1.30,
    "K-Portra 400": 1.30,
    "K-Portra 800": 1.25,
    "K-Portra 800 Push +1": 1.35,
    "K-Portra 800 Push +2": 1.40,
    "K-Ultramax 400": 1.30,
    "K-Verita 200D": 1.35,
    "K-Vision 50D Cinematic": 1.30,
    "K-Vision 200T Cinematic": 1.35,
    "K-Vision 250D Cinematic": 1.35,
    "K-Vision 500T Cinematic": 1.40,
}

_init_params = params_builder.init_params
_digest_params = params_builder.digest_params


def bake(name, film, paper, ev, out):
    scan_film = paper is None

    def init_params(*args, **kwargs):
        params = _init_params(*args, **kwargs)
        params.io.scan_film = scan_film
        return params

    def digest_params(params):
        # digest_params zeroes this in LUT mode, so it has to go in afterwards.
        params = _digest_params(params)
        params.camera.exposure_compensation_ev = ev
        return params

    params_builder.init_params = init_params
    params_builder.digest_params = digest_params
    spec = BundleSpec(
        film_profile=film,
        print_profiles=(paper or PORTRA,),
        input_color_space="Panasonic V-Log",
        output_color_space="sRGB",
        topology="1lut",
        resolution=25,
    )
    builder = BundleBuilder(spec)
    with tempfile.TemporaryDirectory() as tmp:
        builder.write(builder.build(), Path(tmp))
        cube = next(Path(tmp).glob("*.cube"))
        shutil.move(cube, out / f"{name}.cube")
    print(f"{name}: {film} on {paper or 'direct scan'}, {ev:+.2f} EV", flush=True)


if __name__ == "__main__":
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for name, (film, paper) in LOOKS.items():
        bake(name, film, paper, EV[name], out)
