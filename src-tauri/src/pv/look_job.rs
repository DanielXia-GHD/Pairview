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
