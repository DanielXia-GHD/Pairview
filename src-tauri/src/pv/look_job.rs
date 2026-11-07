//! M3-06 job: renders ARW+HIF pairs headless and fits "My Sony" (`pv::look`). Local copies only;
//! never touches the NAS. Skipped unless `PV_LOOK_DIR` is set.
//!
//! ```sh
//! PV_LOOK_DIR=/path/to/lookfit cargo test --lib pv::look_job -- --nocapture
//! ```
//! `$PV_LOOK_DIR/pairs/<name>/{raw.ARW,cam.HIF}` are the pairs, `neutral.json` the default recipe.
//! Renders are cached as PNGs next to them, so re-fitting needs no GPU.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use image::{DynamicImage, ImageBuffer, Rgb as Px, imageops::FilterType};
use serde_json::Value;
use tauri::Manager;

use crate::AppState;
use crate::gpu_processing::RenderOutputPrecision;
use crate::image_processing::GpuContext;
use crate::pv::look::{self, Lut3d, Rgb, Samples};

type Image16 = ImageBuffer<Px<u16>, Vec<u16>>;

const WORK_PX: u32 = 768;
const BLOCK: u32 = 4;
/// Blocks busier than this are edges, where a pixel of misalignment would teach nonsense.
const MAX_BLOCK_SPREAD: f32 = 0.035;
/// Every 5th pair never trains the look: the score comes from these.
const HOLDOUT_EVERY: usize = 5;
/// A pair this far off after a first fit isn't this look (a Creative Look, a filter): left out.
const OUTLIER_DELTA_E: f64 = 8.0;

fn gpu() -> GpuContext {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        ..Default::default()
    }))
    .expect("no GPU");
    let adapter_info = adapter.get_info();
    let mut required_features = wgpu::Features::empty();
    if adapter
        .features()
        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
    {
        required_features |= wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    }
    let limits = adapter.limits();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("Look fitting"),
        required_features,
        required_limits: limits.clone(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("no GPU device");
    GpuContext {
        instance,
        adapter,
        device: Arc::new(device),
        queue: Arc::new(queue),
        limits,
        adapter_info,
        display: Arc::new(std::sync::Mutex::new(None)),
        viewer_display: Arc::new(std::sync::Mutex::new(None)),
        pipelines: Arc::new(std::sync::OnceLock::new()),
        compute: Arc::new(std::sync::OnceLock::new()),
    }
}

/// The owner's settings, read only: tone mapper and RAW options decide what "neutral" looks like.
fn settings() -> crate::AppSettings {
    let path = std::env::var("HOME").unwrap_or_default()
        + "/Library/Application Support/com.danielxia.pairview/settings.json";
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

struct Renderer<'a> {
    context: GpuContext,
    state: tauri::State<'a, AppState>,
    settings: crate::AppSettings,
}

impl Renderer<'_> {
    fn render(&self, raw: &Path, recipe: &Value) -> Result<DynamicImage, String> {
        let path = raw.to_string_lossy().to_string();
        let mut adjustments = recipe.clone();
        crate::hydrate_adjustments(&self.state, &mut adjustments);
        let bytes = crate::file_management::read_file_mapped(raw).map_err(|e| e.to_string())?;
        let base = crate::image_loader::load_and_composite(
            &bytes,
            &path,
            &adjustments,
            false,
            &self.settings,
            None,
        )
        .map_err(|e| e.to_string())?;
        let is_raw = crate::formats::is_raw_file(&path);
        crate::export_processing::process_image_with_render_options(
            &path,
            &base,
            &adjustments,
            &self.context,
            &self.state,
            "pv_look",
            crate::image_processing::resolve_render_options(&self.settings, is_raw),
            RenderOutputPrecision::SixteenBit,
        )
    }
}

/// Centre-crops `image` to `aspect` (width / height), then reduces it to the working size.
fn to_work(image: &DynamicImage, aspect: f64) -> Image16 {
    let (w, h) = (image.width() as f64, image.height() as f64);
    let (cw, ch) = if w / h > aspect {
        (h * aspect, h)
    } else {
        (w, w / aspect)
    };
    let cropped = image.crop_imm(
        ((w - cw) / 2.0) as u32,
        ((h - ch) / 2.0) as u32,
        cw as u32,
        ch as u32,
    );
    let long = cw.max(ch);
    let (tw, th) = (
        (cw * WORK_PX as f64 / long).round() as u32,
        (ch * WORK_PX as f64 / long).round() as u32,
    );
    image::imageops::resize(&cropped.to_rgb16(), tw, th, FilterType::Triangle)
}

/// Aligned block means of a pair, skipping edges and fully clipped camera blocks.
fn samples_of(neutral: &Image16, camera: &Image16) -> Samples {
    let (w, h) = (
        neutral.width().min(camera.width()),
        neutral.height().min(camera.height()),
    );
    let mut out = Vec::new();
    for by in 0..h / BLOCK {
        'block: for bx in 0..w / BLOCK {
            let (mut sum_n, mut sum_c): (Rgb, Rgb) = ([0.0; 3], [0.0; 3]);
            let (mut min_n, mut max_n) = ([f32::MAX; 3], [f32::MIN; 3]);
            let (mut min_c, mut max_c) = ([f32::MAX; 3], [f32::MIN; 3]);
            for y in by * BLOCK..(by + 1) * BLOCK {
                for x in bx * BLOCK..(bx + 1) * BLOCK {
                    let n = neutral.get_pixel(x, y).0.map(|v| v as f32 / 65535.0);
                    let c = camera.get_pixel(x, y).0.map(|v| v as f32 / 65535.0);
                    for i in 0..3 {
                        sum_n[i] += n[i];
                        sum_c[i] += c[i];
                        min_n[i] = min_n[i].min(n[i]);
                        max_n[i] = max_n[i].max(n[i]);
                        min_c[i] = min_c[i].min(c[i]);
                        max_c[i] = max_c[i].max(c[i]);
                    }
                }
            }
            for i in 0..3 {
                if max_n[i] - min_n[i] > MAX_BLOCK_SPREAD || max_c[i] - min_c[i] > MAX_BLOCK_SPREAD
                {
                    continue 'block;
                }
            }
            let count = (BLOCK * BLOCK) as f32;
            out.push((sum_n.map(|v| v / count), sum_c.map(|v| v / count)));
        }
    }
    out
}

#[test]
fn fit_my_sony() {
    let Ok(dir) = std::env::var("PV_LOOK_DIR") else {
        return;
    };
    let dir = Path::new(&dir);
    assert!(!dir.starts_with("/Volumes"), "local copies only");
    let recipe: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("neutral.json")).unwrap()).unwrap();
    let mut pairs: Vec<_> = std::fs::read_dir(dir.join("pairs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("raw.ARW").exists() && p.join("cam.HIF").exists())
        .collect();
    pairs.sort();
    println!("{} pairs", pairs.len());

    let app = tauri::test::mock_app();
    app.manage(crate::initial_app_state());
    let renderer = Renderer {
        context: gpu(),
        state: app.state::<AppState>(),
        settings: settings(),
    };

    // Render once; everything after this reads the PNGs.
    let t = Instant::now();
    let mut data: Vec<(String, Samples)> = Vec::new();
    for (i, pair) in pairs.iter().enumerate() {
        let (np, cp) = (pair.join("neutral.png"), pair.join("camera.png"));
        if !np.exists() || !cp.exists() {
            let cam_bytes = std::fs::read(pair.join("cam.HIF")).unwrap();
            let camera = match crate::pv::heif::decode_heif(&cam_bytes) {
                Ok(c) => c,
                Err(e) => {
                    println!("skip {}: {e:#}", pair.display());
                    continue;
                }
            };
            let neutral = match renderer.render(&pair.join("raw.ARW"), &recipe) {
                Ok(n) => n,
                Err(e) => {
                    println!("skip {}: {e}", pair.display());
                    continue;
                }
            };
            if i == 0 {
                println!(
                    "first pair: render {}x{}, camera {}x{}",
                    neutral.width(),
                    neutral.height(),
                    camera.width(),
                    camera.height()
                );
            }
            let aspect = camera.width() as f64 / camera.height() as f64;
            to_work(&neutral, aspect).save(&np).unwrap();
            to_work(&camera, aspect).save(&cp).unwrap();
            println!(
                "{}/{} rendered, {:.0} s",
                i + 1,
                pairs.len(),
                t.elapsed().as_secs_f64()
            );
        }
        let n = image::open(&np).unwrap().to_rgb16();
        let c = image::open(&cp).unwrap().to_rgb16();
        let name = pair.file_name().unwrap().to_string_lossy().into_owned();
        data.push((name, samples_of(&n, &c)));
    }

    let is_holdout = |i: usize| i.is_multiple_of(HOLDOUT_EVERY);
    let train = |skip: &[String]| -> Samples {
        data.iter()
            .enumerate()
            .filter(|(i, (name, _))| !is_holdout(*i) && !skip.contains(name))
            .flat_map(|(_, (_, s))| s.iter().copied())
            .collect()
    };
    let first = look::fit(&train(&[]));
    let outliers: Vec<String> = data
        .iter()
        .enumerate()
        .filter(|(i, (_, s))| !is_holdout(*i) && look::score(s, Some(&first)).0 > OUTLIER_DELTA_E)
        .map(|(_, (n, _))| n.clone())
        .collect();
    println!("training pairs left out as outliers: {outliers:?}");
    let lut: Lut3d = look::fit(&train(&outliers));
    let cube = dir.join("My Sony.cube");
    lut.write_cube(&cube, "My Sony").unwrap();

    println!("pair | samples | before median | after median | after p95 (held-out pairs)");
    let (mut before, mut after) = (Vec::new(), Vec::new());
    let mut all: Samples = Vec::new();
    for (i, (name, s)) in data.iter().enumerate() {
        if !is_holdout(i) {
            continue;
        }
        let (b, _) = look::score(s, None);
        let (a, p) = look::score(s, Some(&lut));
        println!("{name} | {} | {b:.2} | {a:.2} | {p:.2}", s.len());
        before.push(b);
        after.push(a);
        all.extend(s.iter().copied());
    }
    let mid = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    let (ab, ap) = look::score(&all, Some(&lut));
    println!(
        "HELD OUT: median per pair {:.2} -> {:.2}; all pixels median {ab:.2}, p95 {ap:.2}; neutral all pixels {:.2}",
        mid(&mut before),
        mid(&mut after),
        look::score(&all, None).0
    );

    // Through the real GPU path: the same LUT as the app would apply it.
    let mut with_lut = recipe.clone();
    with_lut["lutPath"] = Value::String(cube.to_string_lossy().into_owned());
    with_lut["lutIntensity"] = Value::from(100);
    with_lut["lutIsSceneReferred"] = Value::Bool(false);
    if let Some(v) = with_lut
        .get_mut("sectionVisibility")
        .and_then(Value::as_object_mut)
    {
        v.insert("effects".into(), Value::Bool(true));
    }
    for (i, pair) in pairs
        .iter()
        .enumerate()
        .filter(|(i, _)| is_holdout(*i))
        .take(3)
    {
        let _ = i;
        let rendered = renderer.render(&pair.join("raw.ARW"), &with_lut).unwrap();
        let c = image::open(pair.join("camera.png")).unwrap().to_rgb16();
        let aspect = c.width() as f64 / c.height() as f64;
        let r = to_work(&rendered, aspect);
        r.save(pair.join("lut_gpu.png")).unwrap();
        let (m, p) = look::score(&samples_of(&r, &c), None);
        println!(
            "GPU with look {}: median {m:.2}, p95 {p:.2}",
            pair.file_name().unwrap().to_string_lossy()
        );
    }
}

// ---- Camera styles (M3-13) --------------------------------------------------------------------

/// Held-out frames a style needs before its score means anything, and the score it must reach.
const STYLE_MIN_HELD_OUT: usize = 5;
const STYLE_BAR: f64 = 3.0;
/// So one foggy frame, all smooth blocks, can't outvote a varied one.
const STYLE_MAX_SAMPLES: usize = 12_000;
/// Below this, the frames don't show the same picture (crop, double exposure, wrong orientation).
const STYLE_MIN_CORRELATION: f64 = 0.92;
/// Below this a frame is nearly all edges or clipping, and its score would mean little.
const STYLE_MIN_SAMPLES: usize = 1_000;
/// Passes of `Lut3d::smoothed`: removes the stray nodes thin data leaves, at no real cost in score.
const STYLE_SMOOTHING: usize = 1;

/// The camera's own JPEG inside a RAF: the header gives its offset and length, big-endian.
fn embedded_jpeg(raf: &[u8]) -> Option<&[u8]> {
    if !raf.starts_with(b"FUJIFILMCCD-RAW") {
        return None;
    }
    let word = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(raf.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let (start, len) = (word(84)?, word(88)?);
    raf.get(start..start.checked_add(len)?)
}

fn decode_oriented_jpeg(bytes: &[u8]) -> Result<DynamicImage, String> {
    use image::ImageDecoder;
    let mut decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().map_err(|e| e.to_string())?;
    let mut image = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// Adobe RGB values to sRGB. Only the green primary differs. Colours outside sRGB clip, as they
/// would on the way to the sRGB image the fit wants.
fn adobe_to_srgb(image: &Image16) -> Image16 {
    let encode = |v: f64| {
        let v = v.clamp(0.0, 1.0);
        let e = if v <= 0.0031308 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (e * 65535.0).round() as u16
    };
    let mut out = image.clone();
    for p in out.pixels_mut() {
        let [r, g, b] = p.0.map(|v| (v as f64 / 65535.0).powf(563.0 / 256.0));
        p.0 = [
            encode(1.39836 * r - 0.39836 * g),
            encode(g),
            encode(-0.04293 * g + 1.04293 * b),
        ];
    }
    out
}

/// `image` zoomed about its centre by `zoom` and shifted by a fraction of its size.
fn warped(image: &Image16, zoom: f64, shift: (f64, f64)) -> Image16 {
    let (w, h) = (image.width(), image.height());
    let (fw, fh) = (w as f64, h as f64);
    ImageBuffer::from_fn(w, h, |x, y| {
        let sx = (fw / 2.0 + (x as f64 - fw / 2.0) / zoom + shift.0 * fw).clamp(0.0, fw - 1.001);
        let sy = (fh / 2.0 + (y as f64 - fh / 2.0) / zoom + shift.1 * fh).clamp(0.0, fh - 1.001);
        let (x0, y0) = (sx as u32, sy as u32);
        let (fx, fy) = (sx - x0 as f64, sy - y0 as f64);
        let (a, b, c, d) = (
            image.get_pixel(x0, y0).0,
            image.get_pixel(x0 + 1, y0).0,
            image.get_pixel(x0, y0 + 1).0,
            image.get_pixel(x0 + 1, y0 + 1).0,
        );
        Px(std::array::from_fn(|i| {
            let top = a[i] as f64 * (1.0 - fx) + b[i] as f64 * fx;
            let bottom = c[i] as f64 * (1.0 - fx) + d[i] as f64 * fx;
            (top * (1.0 - fy) + bottom * fy) as u16
        }))
    })
}

fn correlation(a: &Image16, b: &Image16) -> f64 {
    let luma =
        |p: &Px<u16>| 0.2126 * p.0[0] as f64 + 0.7152 * p.0[1] as f64 + 0.0722 * p.0[2] as f64;
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        let (x, y) = (luma(pa), luma(pb));
        sa += x;
        sb += y;
        saa += x * x;
        sbb += y * y;
        sab += x * y;
        n += 1.0;
    }
    let denom = ((saa - sa * sa / n) * (sbb - sb * sb / n)).sqrt();
    if denom > 0.0 {
        (sab - sa * sb / n) / denom
    } else {
        0.0
    }
}

/// Lines the neutral render up with the camera's JPEG, which is lens-corrected and so shows a
/// slightly different field of view. Returns the aligned pair, the zoom and the correlation.
fn aligned(neutral: &Image16, camera: &Image16) -> (Image16, Image16, f64, f64) {
    let small = |i: &Image16| {
        image::imageops::resize(i, 192, 192 * i.height() / i.width(), FilterType::Triangle)
    };
    let (sn, sc) = (small(neutral), small(camera));
    let mut best = (1.0, (0.0, 0.0), f64::MIN);
    for step in -10..=30 {
        let zoom = 1.0 + step as f64 * 0.004;
        let c = correlation(&warped(&sn, zoom, (0.0, 0.0)), &sc);
        if c > best.2 {
            best = (zoom, (0.0, 0.0), c);
        }
    }
    let zoom = best.0;
    for dy in -6..=6 {
        for dx in -6..=6 {
            let shift = (dx as f64 * 0.0025, dy as f64 * 0.0025);
            let c = correlation(&warped(&sn, zoom, shift), &sc);
            if c > best.2 {
                best = (zoom, shift, c);
            }
        }
    }
    let n = warped(neutral, best.0, best.1);
    // Leaves out the border, where shifting and lens correction leave no real match.
    let (w, h) = (n.width(), n.height());
    let (mx, my) = (w / 25, h / 25);
    let crop =
        |i: &Image16| image::imageops::crop_imm(i, mx, my, w - 2 * mx, h - 2 * my).to_image();
    (crop(&n), crop(camera), best.0, best.2)
}

/// The file name is the label in the Looks panel, so a look that missed the bar says so there.
fn style_name(brand: &str, look: &str, meets_the_bar: bool) -> String {
    if meets_the_bar {
        format!("{brand} {look}")
    } else {
        format!("{brand} {look} (approximate)")
    }
}

fn capped(samples: &[(Rgb, Rgb)]) -> Samples {
    let step = samples.len() / STYLE_MAX_SAMPLES + 1;
    samples.iter().step_by(step).copied().collect()
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        f64::NAN
    } else {
        v[v.len() / 2]
    }
}

struct StyleFrame {
    look: String,
    id: String,
    /// Who shot it, where that's known: a held-out frame's fit sees nothing else of theirs, or it
    /// could learn the photographer's habits and score itself on them.
    shooter: String,
    /// A frame with plain settings scores the look. Mild tone settings only help train it.
    scored: bool,
    samples: Samples,
}

/// The look from `train`, leaving out frames a first fit can't explain (a filter, a local edit).
fn fit_style_from(look_name: &str, train: &[&StyleFrame]) -> Lut3d {
    let all: Samples = train.iter().flat_map(|f| capped(&f.samples)).collect();
    let first = look::fit(&all);
    let kept: Samples = train
        .iter()
        .filter(|f| look::score(&f.samples, Some(&first)).0 <= OUTLIER_DELTA_E)
        .flat_map(|f| capped(&f.samples))
        .collect();
    let lut = look::fit(if kept.is_empty() { &all } else { &kept }).smoothed(STYLE_SMOOTHING);
    if look_name == "Acros" {
        lut.desaturated()
    } else {
        lut
    }
}

/// Fits one LUT per camera style from a brand's RAF files, which carry the camera's own JPEG.
/// `PV_STYLE_DIR=… [PV_LOOK_DIR=<Sony pairs>] cargo test --lib pv::look_job::fit_camera -- --nocapture`
///
/// `$PV_STYLE_DIR/manifest.tsv`: look, source, frame id, score|train|exclude, url, licence, topic,
/// model, bytes, colorspace (1 is sRGB), shooter (blank if unknown), orientation (DNG only). Frames are `frames/<source>/<frame id>.RAF`; renders are
/// cached in `work/`, so fitting again needs no GPU. Cubes land in `cubes/`.
///
/// The manifest holds one frame per scene, so every scored frame can be held out in turn: each is
/// scored by a fit that saw neither it nor its photographer, and the final look trains on all. Thin looks would
/// otherwise have to choose between a look worth having and a score worth trusting.
#[test]
fn fit_camera_styles() {
    let Ok(dir) = std::env::var("PV_STYLE_DIR") else {
        return;
    };
    let dir = Path::new(&dir);
    assert!(!dir.starts_with("/Volumes"), "local copies only");
    // The name in the panel, and the look whose tone stands in for "the camera's standard
    // rendering" when moving to the Sony.
    let brand = std::env::var("PV_STYLE_BRAND").unwrap_or_else(|_| "Fujifilm".into());
    let base_look = std::env::var("PV_STYLE_BASE").unwrap_or_else(|_| "Provia".into());
    let recipe: Value =
        serde_json::from_str(include_str!("../../../scripts/look-neutral.json")).unwrap();
    let manifest = std::fs::read_to_string(dir.join("manifest.tsv")).unwrap();
    let rows: Vec<Vec<&str>> = manifest.lines().map(|l| l.split('\t').collect()).collect();

    let app = tauri::test::mock_app();
    app.manage(crate::initial_app_state());
    let renderer = Renderer {
        context: gpu(),
        state: app.state::<AppState>(),
        settings: settings(),
    };

    let t = Instant::now();
    let mut frames: Vec<StyleFrame> = Vec::new();
    println!("look | frame | zoom | correlation | samples");
    for (i, row) in rows.iter().enumerate() {
        let (look, source, id, split) = (row[0], row[1], row[2], row[3]);
        let folder = dir.join("frames").join(source);
        let raf = ["RAF", "DNG"]
            .iter()
            .map(|ext| folder.join(format!("{id}.{ext}")))
            .find(|p| p.exists())
            .unwrap_or_else(|| folder.join(format!("{id}.RAF")));
        let work = dir.join("work").join(id);
        let (np, cp) = (work.join("neutral.png"), work.join("camera.png"));
        if !np.exists() || !cp.exists() {
            let made: Result<(), String> = (|| {
                // A DNG's JPEG is taken out beforehand (scripts/pv-dng-survey.py), and has no
                // EXIF of its own, so its orientation comes from the manifest.
                let extracted = folder.join(format!("{id}.cam.jpg"));
                let camera = if extracted.exists() {
                    let mut image = decode_oriented_jpeg(
                        &std::fs::read(&extracted).map_err(|e| e.to_string())?,
                    )?;
                    let exif_value = row.get(11).and_then(|v| v.parse().ok()).unwrap_or(1);
                    if let Some(o) = image::metadata::Orientation::from_exif(exif_value) {
                        image.apply_orientation(o);
                    }
                    image
                } else {
                    let bytes = std::fs::read(&raf).map_err(|e| e.to_string())?;
                    decode_oriented_jpeg(embedded_jpeg(&bytes).ok_or("no embedded JPEG")?)?
                };
                let neutral = renderer.render(&raf, &recipe)?;
                std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
                let aspect = camera.width() as f64 / camera.height() as f64;
                let mut c = to_work(&camera, aspect);
                if row.get(9).is_some_and(|s| *s != "1") {
                    c = adobe_to_srgb(&c);
                }
                to_work(&neutral, aspect)
                    .save(&np)
                    .map_err(|e| e.to_string())?;
                c.save(&cp).map_err(|e| e.to_string())?;
                println!(
                    "{}/{} rendered, {:.0} s",
                    i + 1,
                    rows.len(),
                    t.elapsed().as_secs_f64()
                );
                Ok(())
            })();
            if let Err(e) = made {
                println!("skip {id}: {e}");
                continue;
            }
        }
        if split == "exclude" {
            continue;
        }
        let n = image::open(&np).unwrap().to_rgb16();
        let c = image::open(&cp).unwrap().to_rgb16();
        if n.dimensions() != c.dimensions() {
            println!("skip {id}: sizes differ");
            continue;
        }
        let (an, ac, zoom, corr) = aligned(&n, &c);
        let samples = samples_of(&an, &ac);
        println!("{look} | {id} | {zoom:.3} | {corr:.3} | {}", samples.len());
        if corr < STYLE_MIN_CORRELATION || samples.len() < STYLE_MIN_SAMPLES {
            println!(
                "skip {id}: correlation {corr:.3}, {} samples",
                samples.len()
            );
            continue;
        }
        frames.push(StyleFrame {
            look: look.to_string(),
            id: id.to_string(),
            shooter: row
                .get(10)
                .filter(|s| !s.is_empty())
                .map_or_else(|| id.to_string(), |s| s.to_string()),
            scored: split == "score",
            samples,
        });
    }

    let mut looks: Vec<&str> = frames.iter().map(|f| f.look.as_str()).collect();
    looks.sort();
    looks.dedup();
    println!(
        "\nEACH FRAME HELD OUT IN TURN, on the brand's own frames (ΔE2000, per-frame medians)"
    );
    println!("look | frames | scored | neutral | with look | worst frame | meets the bar");
    let mut fitted: Vec<(&str, Lut3d, f64, bool)> = Vec::new();
    for look in looks {
        let of_look: Vec<&StyleFrame> = frames.iter().filter(|f| f.look == look).collect();
        let (mut before, mut after) = (Vec::new(), Vec::new());
        for held in of_look.iter().filter(|f| f.scored) {
            let others: Vec<&StyleFrame> = of_look
                .iter()
                .copied()
                .filter(|f| f.id != held.id && f.shooter != held.shooter)
                .collect();
            if others.is_empty() {
                continue;
            }
            let lut = fit_style_from(look, &others);
            let score = look::score(&held.samples, Some(&lut)).0;
            println!("  {look} {}: {score:.2}", held.id);
            before.push(look::score(&held.samples, None).0);
            after.push(score);
        }
        let worst = after.iter().copied().fold(f64::NAN, f64::max);
        let scored = after.len();
        let score = median(after);
        let passes = scored >= STYLE_MIN_HELD_OUT && score <= STYLE_BAR;
        println!(
            "{look} | {} | {scored} | {:.2} | {score:.2} | {worst:.2} | {passes}",
            of_look.len(),
            median(before)
        );
        // Too few frames to score means no look at all: there's nothing to say how good it is.
        if scored >= STYLE_MIN_HELD_OUT {
            fitted.push((look, fit_style_from(look, &of_look), score, passes));
        } else {
            println!("{look}: left out, {scored} scored frames");
        }
    }

    // Moving to the Sony: the same input curve for every style, so they keep their differences.
    let Ok(sony) = std::env::var("PV_LOOK_DIR") else {
        return;
    };
    let mut pairs: Vec<_> = std::fs::read_dir(Path::new(&sony).join("pairs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("neutral.png").exists() && p.join("camera.png").exists())
        .collect();
    pairs.sort();
    let (mut sony_train, mut sony_test) = (Samples::new(), Vec::new());
    for (i, pair) in pairs.iter().enumerate() {
        let n = image::open(pair.join("neutral.png")).unwrap().to_rgb16();
        let c = image::open(pair.join("camera.png")).unwrap().to_rgb16();
        if i.is_multiple_of(HOLDOUT_EVERY) {
            sony_test.push(samples_of(&n, &c));
        } else {
            sony_train.extend(capped(&samples_of(&n, &c)));
        }
    }
    let Some((_, base, ..)) = fitted.iter().find(|(l, ..)| *l == base_look) else {
        println!("no {base_look} fit, so nothing moved to the Sony");
        return;
    };
    let curve = look::input_curve_for_camera(&sony_train, base);
    println!(
        "\ninput curve at 0.1 0.25 0.5 0.75 0.9: {:.3} {:.3} {:.3} {:.3} {:.3}",
        curve[26], curve[64], curve[128], curve[191], curve[230]
    );
    // Only this run's looks: a stale file here could get installed by mistake.
    let _ = std::fs::remove_dir_all(dir.join("cubes"));
    std::fs::create_dir_all(dir.join("cubes")).unwrap();
    let luma = |c: Rgb| 0.2126 * c[0] as f64 + 0.7152 * c[1] as f64 + 0.0722 * c[2] as f64;
    let camera_luma = median(
        sony_test
            .iter()
            .map(|s| s.iter().map(|(_, c)| luma(*c)).sum::<f64>() / s.len().max(1) as f64)
            .collect(),
    );
    println!(
        "ON THE SONY, 12 held-out pairs. The camera's own smooth-block luma: {camera_luma:.3}"
    );
    println!("look | luma as fitted | luma moved | ΔE to the Sony file, as fitted | moved");
    for (look, lut, _, passes) in &fitted {
        let moved = lut.with_input_curve(&curve);
        let through = |l: &Lut3d| {
            median(
                sony_test
                    .iter()
                    .map(|s| {
                        s.iter().map(|(n, _)| luma(l.apply(*n))).sum::<f64>()
                            / s.len().max(1) as f64
                    })
                    .collect(),
            )
        };
        let de = |l: &Lut3d| {
            median(
                sony_test
                    .iter()
                    .map(|s| look::score(s, Some(l)).0)
                    .collect(),
            )
        };
        println!(
            "{look} | {:.3} | {:.3} | {:.2} | {:.2}",
            through(lut),
            through(&moved),
            de(lut),
            de(&moved)
        );
        let name = style_name(&brand, look, *passes);
        moved
            .write_cube(&dir.join("cubes").join(format!("{name}.cube")), &name)
            .unwrap();
    }

    // Do they stay different from each other on the Sony, and is anything broken in them?
    let moved: Vec<(String, Lut3d)> = fitted
        .iter()
        .map(|(l, lut, _, passes)| (style_name(&brand, l, *passes), lut.with_input_curve(&curve)))
        .collect();
    let test: Samples = sony_test.iter().flat_map(|s| capped(s)).collect();
    let chroma = |c: Rgb| {
        c.iter().copied().fold(f32::MIN, f32::max) - c.iter().copied().fold(f32::MAX, f32::min)
    };
    println!(
        "\nsaturation (mean max−min) on the Sony: its own file {:.3}, neutral {:.3}",
        test.iter().map(|(_, c)| chroma(*c) as f64).sum::<f64>() / test.len() as f64,
        test.iter().map(|(n, _)| chroma(*n) as f64).sum::<f64>() / test.len() as f64,
    );
    for (look, lut) in &moved {
        let sat = test
            .iter()
            .map(|(n, _)| chroma(lut.apply(*n)) as f64)
            .sum::<f64>()
            / test.len() as f64;
        let axis: Vec<f64> = (0..=32)
            .map(|i| {
                let v = i as f32 / 32.0;
                let [r, g, b] = lut.apply([v; 3]);
                0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64
            })
            .collect();
        let dips = axis.windows(2).filter(|w| w[1] < w[0] - 0.002).count();
        let tint = (0..=32)
            .map(|i| chroma(lut.apply([i as f32 / 32.0; 3])))
            .fold(0f32, f32::max);
        println!(
            "{look}: saturation {sat:.3}; grey axis 0.25→{:.3} 0.5→{:.3} 0.75→{:.3} 1→{:.3}, {dips} dips, largest tint {tint:.3}",
            axis[8], axis[16], axis[24], axis[32]
        );
    }
    println!("median ΔE between looks on the Sony");
    for (a, la) in &moved {
        for (b, lb) in &moved {
            if a < b {
                let mut d: Vec<f64> = test
                    .iter()
                    .map(|(n, _)| look::delta_e(la.apply(*n), lb.apply(*n)))
                    .collect();
                d.sort_by(f64::total_cmp);
                println!("{a} / {b}: {:.2}", d[d.len() / 2]);
            }
        }
    }

    // Through the real GPU path, as the app applies a look from the luts folder.
    if std::env::var("PV_STYLE_GPU").is_err() {
        return;
    }
    println!("GPU path against the CPU calculation (the same look, the same neutral render)");
    for pair in pairs.iter().step_by(HOLDOUT_EVERY).take(3) {
        let raw = pair.join("raw.ARW");
        let c = image::open(pair.join("camera.png")).unwrap().to_rgb16();
        let aspect = c.width() as f64 / c.height() as f64;
        let n = image::open(pair.join("neutral.png")).unwrap().to_rgb16();
        for (look, lut) in &moved {
            let mut with_lut = recipe.clone();
            let cube = dir.join("cubes").join(format!("{look}.cube"));
            with_lut["lutPath"] = Value::String(cube.to_string_lossy().into_owned());
            with_lut["lutIntensity"] = Value::from(100);
            with_lut["lutIsSceneReferred"] = Value::Bool(false);
            if let Some(v) = with_lut
                .get_mut("sectionVisibility")
                .and_then(Value::as_object_mut)
            {
                v.insert("effects".into(), Value::Bool(true));
            }
            let gpu = to_work(&renderer.render(&raw, &with_lut).unwrap(), aspect);
            let s = samples_of(&n, &gpu);
            let (m, p) = look::score(&s, Some(lut));
            println!("{look}: median {m:.2}, p95 {p:.2}");
        }
    }
}

#[test]
fn finds_the_jpeg_inside_a_raf() {
    let mut raf = vec![0u8; 100];
    raf[..15].copy_from_slice(b"FUJIFILMCCD-RAW");
    raf[84..88].copy_from_slice(&92u32.to_be_bytes());
    raf[88..92].copy_from_slice(&4u32.to_be_bytes());
    raf[92..96].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xD9]);
    assert_eq!(embedded_jpeg(&raf), Some(&[0xFF, 0xD8, 0xFF, 0xD9][..]));
    assert_eq!(embedded_jpeg(b"not a raf"), None);
    raf[88..92].copy_from_slice(&4000u32.to_be_bytes());
    assert_eq!(
        embedded_jpeg(&raf),
        None,
        "a length past the end of the file"
    );
}

#[test]
fn adobe_conversion_leaves_grey_and_widens_green() {
    let mut image: Image16 = ImageBuffer::new(2, 1);
    image.put_pixel(0, 0, Px([32768, 32768, 32768]));
    image.put_pixel(1, 0, Px([20000, 50000, 20000]));
    let out = adobe_to_srgb(&image);
    // Grey stays grey; only the gamma differs slightly (2.2 against the sRGB curve).
    let [r, g, b] = out.get_pixel(0, 0).0;
    assert!(r.abs_diff(g) < 4 && g.abs_diff(b) < 4);
    assert!(r.abs_diff(32768) < 1500);
    // Adobe's green is more saturated than sRGB's, so red falls as the green is expressed in sRGB.
    assert!(out.get_pixel(1, 0).0[0] < 20000);
}

/// Auto with a look on, computed the way `export_processing::auto_exposure_with_look` does it:
/// `PV_LOOK_DIR=… PV_LOOK_AUTO=1 cargo test --lib pv::look_job::auto -- --nocapture`
#[test]
fn auto_with_looks() {
    let (Ok(dir), Ok(_)) = (std::env::var("PV_LOOK_DIR"), std::env::var("PV_LOOK_AUTO")) else {
        return;
    };
    let dir = Path::new(&dir);
    let recipe: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("neutral.json")).unwrap()).unwrap();
    let film =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/pv_film_luts/K-Portra 400.cube");
    let mut pairs: Vec<_> = std::fs::read_dir(dir.join("pairs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    pairs.sort();
    let app = tauri::test::mock_app();
    app.manage(crate::initial_app_state());
    let r = Renderer {
        context: gpu(),
        state: app.state::<AppState>(),
        settings: settings(),
    };
    let opts = crate::image_processing::resolve_render_options(&r.settings, true);
    for (name, cube, scene) in [
        ("My Sony", dir.join("My Sony.cube"), false),
        ("Portra 400", film, true),
    ] {
        let mut with_look = recipe.clone();
        with_look["lutPath"] = Value::String(cube.to_string_lossy().into_owned());
        with_look["lutIntensity"] = Value::from(100);
        with_look["lutIsSceneReferred"] = Value::Bool(scene);
        println!("{name}: pair | camera luma, | look luma, dE | judged EV | auto+look luma, dE");
        for pair in pairs.iter().step_by(HOLDOUT_EVERY) {
            let raw = pair.join("raw.ARW");
            let c = image::open(pair.join("camera.png")).unwrap().to_rgb16();
            let aspect = c.width() as f64 / c.height() as f64;
            let path = raw.to_string_lossy().to_string();
            let bytes = crate::file_management::read_file_mapped(&raw).unwrap();
            let base = crate::image_loader::load_and_composite(
                &bytes,
                &path,
                &recipe,
                false,
                &r.settings,
                None,
            )
            .unwrap();
            let exposure = crate::image_processing::effective_baseline_exposure(
                &recipe,
                crate::as_shot::as_shot(&path),
                opts.baseline,
            );
            let results = crate::image_processing::perform_auto_analysis(&base, exposure);
            let auto = crate::image_processing::auto_results_to_json(&results, Default::default());
            // As the app judges it: a small input through the look, exposure 0.
            let mut judge = with_look.clone();
            judge["exposure"] = Value::from(0);
            crate::hydrate_adjustments(&r.state, &mut judge);
            let small = crate::pv::thumbs::shrink(&base, 1024);
            let probe = crate::export_processing::process_image_with_render_options(
                &path,
                &small,
                &judge,
                &r.context,
                &r.state,
                "pv_auto",
                opts,
                RenderOutputPrecision::EightBit,
            )
            .unwrap();
            let ev = look::auto_exposure_with_look(look::mean_luma(&probe));
            let mut both = with_look.clone();
            for (k, v) in auto.as_object().unwrap() {
                if k == "activeTools" {
                    if let (Some(to), Some(from)) = (
                        both.get_mut(k).and_then(Value::as_object_mut),
                        v.as_object(),
                    ) {
                        to.extend(from.clone());
                    } else {
                        both[k] = v.clone();
                    }
                } else {
                    both[k] = v.clone();
                }
            }
            both["exposure"] = Value::from(ev);
            let look_img = to_work(&r.render(&raw, &with_look).unwrap(), aspect);
            let auto_img = to_work(&r.render(&raw, &both).unwrap(), aspect);
            println!(
                "{} | {:.3} | {:.3} {:.2} | {ev:+.2} | {:.3} {:.2}",
                pair.file_name().unwrap().to_string_lossy(),
                look::mean_luma(&DynamicImage::ImageRgb16(c.clone())),
                look::mean_luma(&DynamicImage::ImageRgb16(look_img.clone())),
                look::score(&samples_of(&look_img, &c), None).0,
                look::mean_luma(&DynamicImage::ImageRgb16(auto_img.clone())),
                look::score(&samples_of(&auto_img, &c), None).0,
            );
        }
    }
}

/// Every film look through the real GPU path: a contact sheet from one photo, and how bright each
/// comes out against the camera on the held-out pairs (what `EV` in the bake script is tuned from).
/// `PV_LOOK_DIR=… PV_LOOK_FILM=1 cargo test --lib pv::look_job::film -- --nocapture`
#[test]
fn film_contact_sheet() {
    let (Ok(dir), Ok(_)) = (std::env::var("PV_LOOK_DIR"), std::env::var("PV_LOOK_FILM")) else {
        return;
    };
    let dir = Path::new(&dir);
    let recipe: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("neutral.json")).unwrap()).unwrap();
    let resources = Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/pv_film_luts");
    let mut cubes: Vec<_> = std::fs::read_dir(resources)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "cube"))
        .collect();
    cubes.sort();
    let mut pairs: Vec<_> = std::fs::read_dir(dir.join("pairs"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .collect();
    pairs.sort();
    let app = tauri::test::mock_app();
    app.manage(crate::initial_app_state());
    let r = Renderer {
        context: gpu(),
        state: app.state::<AppState>(),
        settings: settings(),
    };
    let opts = crate::image_processing::resolve_render_options(&r.settings, true);

    // Small developed bases, once, so every look only costs a small GPU render.
    let held: Vec<_> = pairs.iter().step_by(HOLDOUT_EVERY).collect();
    let bases: Vec<(String, DynamicImage, f64)> = held
        .iter()
        .map(|pair| {
            let raw = pair.join("raw.ARW");
            let path = raw.to_string_lossy().to_string();
            let bytes = crate::file_management::read_file_mapped(&raw).unwrap();
            let base = crate::image_loader::load_and_composite(
                &bytes,
                &path,
                &recipe,
                false,
                &r.settings,
                None,
            )
            .unwrap();
            let camera = look::mean_luma(&image::open(pair.join("camera.png")).unwrap());
            (path, crate::pv::thumbs::shrink(&base, 1024), camera)
        })
        .collect();
    let camera_median = {
        let mut v: Vec<f64> = bases.iter().map(|b| b.2).collect();
        v.sort_by(f64::total_cmp);
        v[v.len() / 2]
    };
    println!("camera median luma {camera_median:.3}");

    let render = |path: &str, base: &DynamicImage, cube: &Path| -> DynamicImage {
        let mut v = recipe.clone();
        v["lutPath"] = Value::String(cube.to_string_lossy().into_owned());
        v["lutIntensity"] = Value::from(100);
        v["lutIsSceneReferred"] = Value::Bool(true);
        crate::hydrate_adjustments(&r.state, &mut v);
        crate::export_processing::process_image_with_render_options(
            path,
            base,
            &v,
            &r.context,
            &r.state,
            "pv_film",
            opts,
            RenderOutputPrecision::EightBit,
        )
        .unwrap()
    };
    // A look that can't be loaded (moved, deleted) renders without it, not black.
    let (path, base, _) = &bases[2];
    let missing = render(path, base, Path::new("/nowhere/Gone.cube"));
    let no_look = {
        let mut v = recipe.clone();
        crate::hydrate_adjustments(&r.state, &mut v);
        crate::export_processing::process_image_with_render_options(
            path,
            base,
            &v,
            &r.context,
            &r.state,
            "pv_film",
            opts,
            RenderOutputPrecision::EightBit,
        )
        .unwrap()
    };
    let (a, b) = (look::mean_luma(&missing), look::mean_luma(&no_look));
    println!("missing look {a:.3}, no look {b:.3}");
    assert!(
        (a - b).abs() < 0.01,
        "a missing look must render as no look"
    );

    let (cell_w, cell_h, columns) = (300u32, 200u32, 5u32);
    let rows = (cubes.len() as u32).div_ceil(columns);
    let mut sheet = image::RgbImage::new(cell_w * columns, cell_h * rows);
    println!("look | median luma | range | EV change to reach the camera");
    for (i, cube) in cubes.iter().enumerate() {
        let mut lumas = Vec::new();
        for (path, base, _) in &bases {
            lumas.push(look::mean_luma(&render(path, base, cube)));
        }
        let small = render(&bases[2].0, &bases[2].1, cube)
            .thumbnail(cell_w, cell_h)
            .to_rgb8();
        let (x, y) = (i as u32 % columns * cell_w, i as u32 / columns * cell_h);
        image::imageops::overlay(&mut sheet, &small, x as i64, y as i64);
        let mut sorted = lumas.clone();
        sorted.sort_by(f64::total_cmp);
        let median = sorted[sorted.len() / 2];
        let lin = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        println!(
            "{i} {} | {median:.3} | {:.3}-{:.3} | {:+.2}",
            cube.file_stem().unwrap().to_string_lossy(),
            sorted[0],
            sorted[sorted.len() - 1],
            (lin(camera_median) / lin(median)).log2()
        );
    }
    sheet.save(dir.join("film_sheet.jpg")).unwrap();
}
