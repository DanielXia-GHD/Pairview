//! "My Sony" look fitting (docs/IMPLEMENTATION_PLAN.md M3-06, PRD §6.4): a 3D LUT that turns
//! RapidRAW's neutral render of a RAW into what the camera put in its HIF, learned from the
//! owner's own pairs.
//!
//! RapidRAW applies a user LUT last, on the display-referred image, so both sides of the fit are
//! sRGB-encoded values in 0..1. The fit has two layers, because pairs cover colour space unevenly:
//! one tone curve per channel (robust, covers everything), then a smooth 3D correction learned
//! where there are samples, fading back to the curves where there aren't. Saturated colours the
//! sample never saw therefore get a sensible result, not noise.

// The fitting job (`pv::look_job`) is a gated test for now; nothing in the app calls this yet.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;

pub type Rgb = [f32; 3];

/// One pair's pixels, already aligned: `(neutral render, camera)` in sRGB-encoded 0..1.
pub type Samples = Vec<(Rgb, Rgb)>;

/// Lattice size. 33 is the standard "fine" LUT and what the GPU path handles quickly.
pub const LUT_SIZE: usize = 33;
const CURVE_BINS: usize = 256;
/// Below this much kernel weight a node trusts the tone curves alone.
const SHRINK_WEIGHT: f64 = 20.0;
const KERNEL_RADIUS: i32 = 2;
const KERNEL_SIGMA: f64 = 1.0;

#[derive(Debug, Clone)]
pub struct Lut3d {
    pub size: usize,
    /// `.cube` order: red varies fastest.
    pub data: Vec<Rgb>,
}

impl Lut3d {
    pub fn identity(size: usize) -> Lut3d {
        let mut data = Vec::with_capacity(size * size * size);
        let step = 1.0 / (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    data.push([r as f32 * step, g as f32 * step, b as f32 * step]);
                }
            }
        }
        Lut3d { size, data }
    }

    fn at(&self, r: usize, g: usize, b: usize) -> Rgb {
        self.data[r + g * self.size + b * self.size * self.size]
    }

    /// Tetrahedral interpolation, as the shader does (`sample_lut_tetrahedral`).
    pub fn apply(&self, c: Rgb) -> Rgb {
        let n = (self.size - 1) as f32;
        let pos = c.map(|v| v.clamp(0.0, 1.0) * n);
        let lo = pos.map(|v| (v.floor() as usize).min(self.size - 2));
        let f = [
            pos[0] - lo[0] as f32,
            pos[1] - lo[1] as f32,
            pos[2] - lo[2] as f32,
        ];
        let v = |dr, dg, db| self.at(lo[0] + dr, lo[1] + dg, lo[2] + db);
        let (c000, c111) = (v(0, 0, 0), v(1, 1, 1));
        // Walk from c000 to c111 along the axes in order of largest fraction.
        let mut order = [0usize, 1, 2];
        order.sort_by(|&a, &b| f[b].total_cmp(&f[a]));
        let mut d = [0usize; 3];
        d[order[0]] = 1;
        let c1 = v(d[0], d[1], d[2]);
        d[order[1]] = 1;
        let c2 = v(d[0], d[1], d[2]);
        let (f0, f1, f2) = (f[order[0]], f[order[1]], f[order[2]]);
        let mut out = [0.0; 3];
        for i in 0..3 {
            out[i] = c000[i] * (1.0 - f0) + c1[i] * (f0 - f1) + c2[i] * (f1 - f2) + c111[i] * f2;
        }
        out
    }

    /// Writes an Adobe `.cube`, which RapidRAW's LUT loader reads.
    pub fn write_cube(&self, path: &Path, title: &str) -> Result<()> {
        let mut text = String::with_capacity(self.data.len() * 28);
        writeln!(text, "TITLE \"{title}\"")?;
        writeln!(text, "LUT_3D_SIZE {}", self.size)?;
        for c in &self.data {
            writeln!(text, "{:.6} {:.6} {:.6}", c[0], c[1], c[2])?;
        }
        crate::file_management::write_file_atomic(path, text.as_bytes())?;
        Ok(())
    }
}

/// Per-channel tone curves: mean camera value for each neutral value, made monotonic.
fn fit_curves(samples: &[(Rgb, Rgb)]) -> [Vec<f32>; 3] {
    std::array::from_fn(|ch| {
        let mut sum = vec![0f64; CURVE_BINS];
        let mut count = vec![0f64; CURVE_BINS];
        for (n, c) in samples {
            let bin = (n[ch].clamp(0.0, 1.0) * (CURVE_BINS - 1) as f32).round() as usize;
            sum[bin] += c[ch] as f64;
            count[bin] += 1.0;
        }
        // Pool-adjacent-violators on the bin means: the best monotonic curve.
        let mut blocks: Vec<(f64, f64, usize)> = Vec::new();
        for i in 0..CURVE_BINS {
            // Empty bins borrow a whisper of the identity, so the ends stay defined.
            let prior = 1e-3;
            let (mut s, mut w, mut len) = (
                sum[i] + prior * i as f64 / (CURVE_BINS - 1) as f64,
                count[i] + prior,
                1usize,
            );
            while let Some(&(ps, pw, pl)) = blocks.last() {
                if ps / pw <= s / w {
                    break;
                }
                s += ps;
                w += pw;
                len += pl;
                blocks.pop();
            }
            blocks.push((s, w, len));
        }
        let mut curve = Vec::with_capacity(CURVE_BINS);
        for (s, w, len) in blocks {
            curve.extend(std::iter::repeat_n((s / w) as f32, len));
        }
        curve
    })
}

fn curve_at(curves: &[Vec<f32>; 3], c: Rgb) -> Rgb {
    std::array::from_fn(|ch| {
        let x = c[ch].clamp(0.0, 1.0) * (CURVE_BINS - 1) as f32;
        let lo = (x.floor() as usize).min(CURVE_BINS - 2);
        let f = x - lo as f32;
        curves[ch][lo] * (1.0 - f) + curves[ch][lo + 1] * f
    })
}

/// Solves `A x = b` for a symmetric positive-definite 4×4 (Gaussian elimination with pivoting).
fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let pivot = (col..4).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, pivot);
        b.swap(col, pivot);
        for row in col + 1..4 {
            let m = a[row][col] / a[col][col];
            for k in col..4 {
                a[row][k] -= m * a[col][k];
            }
            b[row] -= m * b[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let mut v = b[row];
        for k in row + 1..4 {
            v -= a[row][k] * x[k];
        }
        x[row] = v / a[row][row];
    }
    Some(x)
}

/// Fits the look: tone curves, then a local-linear 3D correction of what they leave.
pub fn fit(samples: &[(Rgb, Rgb)]) -> Lut3d {
    let curves = fit_curves(samples);
    let size = LUT_SIZE;
    let cells = size * size * size;
    // Moments per bin of what the curves leave over.
    #[derive(Clone, Copy, Default)]
    struct Bin {
        w: f64,
        x: [f64; 3],
        xx: [[f64; 3]; 3],
        y: [f64; 3],
        xy: [[f64; 3]; 3],
    }
    let mut bins = vec![Bin::default(); cells];
    let scale = (size - 1) as f32;
    for (n, c) in samples {
        let prior = curve_at(&curves, *n);
        let g: [f64; 3] = std::array::from_fn(|i| (n[i].clamp(0.0, 1.0) * scale) as f64);
        let idx = g.map(|v| (v.round() as usize).min(size - 1));
        let b = &mut bins[idx[0] + idx[1] * size + idx[2] * size * size];
        b.w += 1.0;
        for i in 0..3 {
            let r = (c[i] - prior[i]) as f64;
            b.x[i] += g[i];
            b.y[i] += r;
            for j in 0..3 {
                b.xx[i][j] += g[i] * g[j];
                b.xy[i][j] += g[i] * (c[j] - prior[j]) as f64;
            }
        }
    }
    // Precomputed kernel offsets.
    let mut kernel = Vec::new();
    for dz in -KERNEL_RADIUS..=KERNEL_RADIUS {
        for dy in -KERNEL_RADIUS..=KERNEL_RADIUS {
            for dx in -KERNEL_RADIUS..=KERNEL_RADIUS {
                let d2 = (dx * dx + dy * dy + dz * dz) as f64;
                kernel.push((
                    dx,
                    dy,
                    dz,
                    (-d2 / (2.0 * KERNEL_SIGMA * KERNEL_SIGMA)).exp(),
                ));
            }
        }
    }
    let mut lut = Lut3d::identity(size);
    for bz in 0..size as i32 {
        for by in 0..size as i32 {
            for bx in 0..size as i32 {
                let node = [bx as f64, by as f64, bz as f64];
                // Weighted normal equations in coordinates centred on the node.
                let mut a = [[0f64; 4]; 4];
                let mut rhs = [[0f64; 4]; 3];
                let mut weight = 0f64;
                for &(dx, dy, dz, k) in &kernel {
                    let (x, y, z) = (bx + dx, by + dy, bz + dz);
                    let top = size as i32;
                    if x < 0 || y < 0 || z < 0 || x >= top || y >= top || z >= top {
                        continue;
                    }
                    let b = &bins[x as usize + y as usize * size + z as usize * size * size];
                    if b.w == 0.0 {
                        continue;
                    }
                    weight += k * b.w;
                    let mean: [f64; 3] = std::array::from_fn(|i| b.x[i] - node[i] * b.w);
                    a[0][0] += k * b.w;
                    for i in 0..3 {
                        a[0][i + 1] += k * mean[i];
                        a[i + 1][0] += k * mean[i];
                        for j in 0..3 {
                            a[i + 1][j + 1] += k
                                * (b.xx[i][j] - node[i] * b.x[j] - node[j] * b.x[i]
                                    + node[i] * node[j] * b.w);
                        }
                    }
                    for ch in 0..3 {
                        rhs[ch][0] += k * b.y[ch];
                        for i in 0..3 {
                            rhs[ch][i + 1] += k * (b.xy[i][ch] - node[i] * b.y[ch]);
                        }
                    }
                }
                let g = [bx as f32 / scale, by as f32 / scale, bz as f32 / scale];
                let prior = curve_at(&curves, g);
                let mut value = prior;
                if weight > 0.0 {
                    // A little ridge keeps thin data from tilting the local plane.
                    for (i, row) in a.iter_mut().enumerate().skip(1) {
                        row[i] += 1e-2 * weight;
                    }
                    a[0][0] += 1e-9;
                    let confidence = (weight / (weight + SHRINK_WEIGHT)) as f32;
                    for ch in 0..3 {
                        if let Some(x) = solve4(a, rhs[ch]) {
                            value[ch] = (prior[ch] + confidence * x[0] as f32).clamp(0.0, 1.0);
                        }
                    }
                }
                lut.data[bx as usize + by as usize * size + bz as usize * size * size] = value;
            }
        }
    }
    lut
}

// ---- Quality metric: CIEDE2000 ----------------------------------------------------------------

fn srgb_to_lab(c: Rgb) -> [f64; 3] {
    let lin = c.map(|v| {
        let v = v.clamp(0.0, 1.0) as f64;
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    });
    let x = (0.4124564 * lin[0] + 0.3575761 * lin[1] + 0.1804375 * lin[2]) / 0.95047;
    let y = 0.2126729 * lin[0] + 0.7151522 * lin[1] + 0.0721750 * lin[2];
    let z = (0.0193339 * lin[0] + 0.1191920 * lin[1] + 0.9503041 * lin[2]) / 1.08883;
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let (fx, fy, fz) = (f(x), f(y), f(z));
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn delta_e_2000(l1: [f64; 3], l2: [f64; 3]) -> f64 {
    use std::f64::consts::PI;
    let rad = |d: f64| d * PI / 180.0;
    let c1 = (l1[1] * l1[1] + l1[2] * l1[2]).sqrt();
    let c2 = (l2[1] * l2[1] + l2[2] * l2[2]).sqrt();
    let cbar7 = ((c1 + c2) / 2.0).powi(7);
    let g = 0.5 * (1.0 - (cbar7 / (cbar7 + 25f64.powi(7))).sqrt());
    let (a1, a2) = ((1.0 + g) * l1[1], (1.0 + g) * l2[1]);
    let (cp1, cp2) = (
        (a1 * a1 + l1[2] * l1[2]).sqrt(),
        (a2 * a2 + l2[2] * l2[2]).sqrt(),
    );
    let hue = |b: f64, a: f64| {
        if a == 0.0 && b == 0.0 {
            0.0
        } else {
            let h = b.atan2(a);
            if h < 0.0 { h + 2.0 * PI } else { h }
        }
    };
    let (h1, h2) = (hue(l1[2], a1), hue(l2[2], a2));
    let dl = l2[0] - l1[0];
    let dc = cp2 - cp1;
    let mut dh = h2 - h1;
    if cp1 * cp2 == 0.0 {
        dh = 0.0;
    } else if dh > PI {
        dh -= 2.0 * PI;
    } else if dh < -PI {
        dh += 2.0 * PI;
    }
    let dhh = 2.0 * (cp1 * cp2).sqrt() * (dh / 2.0).sin();
    let lbar = (l1[0] + l2[0]) / 2.0;
    let cbar = (cp1 + cp2) / 2.0;
    let hbar = if cp1 * cp2 == 0.0 {
        h1 + h2
    } else if (h1 - h2).abs() <= PI {
        (h1 + h2) / 2.0
    } else if h1 + h2 < 2.0 * PI {
        (h1 + h2 + 2.0 * PI) / 2.0
    } else {
        (h1 + h2 - 2.0 * PI) / 2.0
    };
    let t = 1.0 - 0.17 * (hbar - rad(30.0)).cos()
        + 0.24 * (2.0 * hbar).cos()
        + 0.32 * (3.0 * hbar + rad(6.0)).cos()
        - 0.20 * (4.0 * hbar - rad(63.0)).cos();
    let d_theta = rad(30.0) * (-((hbar - rad(275.0)) / rad(25.0)).powi(2)).exp();
    let rc = 2.0 * (cbar.powi(7) / (cbar.powi(7) + 25f64.powi(7))).sqrt();
    let sl = 1.0 + 0.015 * (lbar - 50.0).powi(2) / (20.0 + (lbar - 50.0).powi(2)).sqrt();
    let sc = 1.0 + 0.045 * cbar;
    let sh = 1.0 + 0.015 * cbar * t;
    let rt = -(2.0 * d_theta).sin() * rc;
    ((dl / sl).powi(2) + (dc / sc).powi(2) + (dhh / sh).powi(2) + rt * (dc / sc) * (dhh / sh))
        .sqrt()
}

/// Colour difference between two sRGB-encoded values. Around 1 is barely visible side by side;
/// around 2–3 is noticeable if you look for it.
pub fn delta_e(a: Rgb, b: Rgb) -> f64 {
    delta_e_2000(srgb_to_lab(a), srgb_to_lab(b))
}

/// `(median, 95th percentile)` of the colour difference, optionally through a LUT.
pub fn score(samples: &[(Rgb, Rgb)], lut: Option<&Lut3d>) -> (f64, f64) {
    let mut d: Vec<f64> = samples
        .iter()
        .map(|(n, c)| delta_e(lut.map_or(*n, |l| l.apply(*n)), *c))
        .collect();
    if d.is_empty() {
        return (0.0, 0.0);
    }
    d.sort_by(f64::total_cmp);
    (d[d.len() / 2], d[d.len() * 95 / 100])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plausible "camera": contrast curve plus a colour twist that per-channel curves can't do.
    fn camera(n: Rgb) -> Rgb {
        let s = |v: f32| v * v * (3.0 - 2.0 * v);
        let [r, g, b] = n.map(s);
        [
            (r * 0.95 + g * 0.05).clamp(0.0, 1.0),
            (g * 0.90 + b * 0.10).clamp(0.0, 1.0),
            (b * 0.92 + r * 0.08).clamp(0.0, 1.0),
        ]
    }

    /// Deterministic low-discrepancy points, denser around neutral like real photos.
    fn points(count: usize) -> Vec<Rgb> {
        (0..count)
            .map(|i| {
                let v = |k: f64| ((i as f64 + 0.5) * k).fract() as f32;
                let (l, a, b) = (v(0.618034), v(0.754878), v(0.569840));
                let chroma = 0.35;
                [
                    (l + (a - 0.5) * chroma).clamp(0.0, 1.0),
                    l,
                    (l + (b - 0.5) * chroma).clamp(0.0, 1.0),
                ]
            })
            .collect()
    }

    #[test]
    fn identity_lut_changes_nothing() {
        let lut = Lut3d::identity(9);
        for c in [[0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.21, 0.67, 0.93]] {
            let out = lut.apply(c);
            for i in 0..3 {
                assert!((out[i] - c[i]).abs() < 1e-5, "{c:?} -> {out:?}");
            }
        }
    }

    #[test]
    fn fit_recovers_a_known_look_on_points_it_never_saw() {
        let train: Samples = points(200_000)
            .into_iter()
            .map(|n| (n, camera(n)))
            .collect();
        let lut = fit(&train);
        let test: Samples = points(20_000)
            .into_iter()
            .map(|n| {
                let n = [
                    (n[0] * 0.97 + 0.011).min(1.0),
                    n[1] * 0.99,
                    (n[2] * 0.98 + 0.007).min(1.0),
                ];
                (n, camera(n))
            })
            .collect();
        let (before, _) = score(&test, None);
        let (after, p95) = score(&test, Some(&lut));
        assert!(before > 3.0, "the test look should be visible: {before}");
        assert!(after < 0.6, "median ΔE {after} (was {before})");
        assert!(p95 < 1.5, "95th percentile ΔE {p95}");
    }

    #[test]
    fn an_identity_look_stays_identity() {
        let train: Samples = points(100_000).into_iter().map(|n| (n, n)).collect();
        let lut = fit(&train);
        let (median, p95) = score(&train, Some(&lut));
        assert!(median < 0.3 && p95 < 0.8, "{median} {p95}");
    }

    #[test]
    fn colours_it_never_saw_follow_the_curves_not_noise() {
        // Only near-neutral training data; a pure red must still come out sane.
        let train: Samples = (0..50_000)
            .map(|i| {
                let v = (i as f32 + 0.5) / 50_000.0;
                ([v, v, v], [v * 0.9, v * 0.9, v * 0.9])
            })
            .collect();
        let lut = fit(&train);
        let red = lut.apply([1.0, 0.0, 0.0]);
        assert!((red[0] - 0.9).abs() < 0.03, "{red:?}");
        assert!(red[1] < 0.03 && red[2] < 0.03, "{red:?}");
    }

    #[test]
    fn delta_e_matches_published_values() {
        // Sharma, Wu & Dalal (2005) CIEDE2000 test pairs.
        for (a, b, want) in [
            ([50.0, 2.6772, -79.7751], [50.0, 0.0, -82.7485], 2.0425),
            ([50.0, 2.5, 0.0], [50.0, 0.0, -2.5], 4.3065),
            ([50.0, 2.5, 0.0], [73.0, 25.0, -18.0], 27.1492),
            (
                [2.0776, 0.0795, -1.1350],
                [0.9033, -0.0636, -0.5514],
                0.9082,
            ),
        ] {
            let got = delta_e_2000(a, b);
            assert!((got - want).abs() < 1e-3, "{got} vs {want}");
        }
    }

    #[test]
    fn the_cube_file_loads_in_rapidraw() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("My Sony.cube");
        let train: Samples = points(20_000).into_iter().map(|n| (n, camera(n))).collect();
        let lut = fit(&train);
        lut.write_cube(&path, "My Sony").unwrap();
        let loaded = crate::lut_processing::parse_lut_file(path.to_str().unwrap()).unwrap();
        assert_eq!(loaded.size as usize, LUT_SIZE);
        assert_eq!(loaded.data.len(), LUT_SIZE * LUT_SIZE * LUT_SIZE * 3);
        // Red varies fastest: the second entry is a step along red.
        assert!((loaded.data[3] - lut.data[1][0]).abs() < 1e-5);
    }
}
