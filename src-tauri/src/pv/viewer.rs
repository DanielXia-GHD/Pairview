//! `pvimg://` URL scheme for the viewer (docs/IMPLEMENTATION_PLAN.md M1-14, A5).
//!
//! `pvimg://localhost/photo/<id>` serves a Photo's display copy as is (HIF/JPG; WebKit decodes HEIF
//! through ImageIO on macOS). `…?jpeg` serves a JPEG rendering instead, for when the webview can't
//! decode the file. A Photo without a display copy gets its RAW's embedded preview.
//!
//! Only files of Photos in the catalog can be served; the URL carries a Photo id, never a path.

use std::io::Cursor;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use image::DynamicImage;
use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, UriSchemeResponder};

use crate::pv::commands::PvState;

pub const SCHEME: &str = "pvimg";

/// Long edge of the JPEG rendering: sharp on a 5K screen, quick to encode.
const JPEG_MAX_PX: u32 = 4096;

pub fn handle(app: AppHandle, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    std::thread::spawn(move || {
        let response = match serve(&app, &request) {
            Ok((bytes, mime)) => Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "no-cache")
                .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
                .body(bytes),
            Err(e) => {
                log::warn!("pvimg {}: {e:#}", request.uri());
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .header(header::CONTENT_TYPE, "text/plain")
                    .body(format!("{e:#}").into_bytes())
            }
        };
        responder.respond(response.unwrap_or_else(|_| Response::new(Vec::new())));
    });
}

#[derive(Debug, PartialEq, Eq)]
struct Target {
    photo_id: i64,
    jpeg: bool,
}

fn parse(path: &str, query: Option<&str>) -> Option<Target> {
    let id = path.trim_start_matches('/').strip_prefix("photo/")?;
    Some(Target {
        photo_id: id.trim_end_matches('/').parse().ok()?,
        jpeg: query.is_some_and(|q| q.split('&').any(|p| p == "jpeg")),
    })
}

fn serve(app: &AppHandle, request: &Request<Vec<u8>>) -> Result<(Vec<u8>, &'static str)> {
    let target = parse(request.uri().path(), request.uri().query())
        .ok_or_else(|| anyhow!("expected /photo/<id>"))?;
    let state = app.state::<PvState>();
    let (display, raw) = state.read(|cat| files_of(cat, target.photo_id))?;

    match (display, raw) {
        (Some(display), _) if !target.jpeg => {
            let mime = mime_for(&display);
            Ok((std::fs::read(&display)?, mime))
        }
        (Some(display), _) => {
            let bytes = std::fs::read(&display)?;
            let image = if crate::pv::heif::is_heif_file(&display) {
                crate::pv::heif::heif_thumbnail(&bytes, JPEG_MAX_PX)?
            } else {
                image::load_from_memory(&bytes)?
            };
            Ok((jpeg(&image)?, "image/jpeg"))
        }
        (None, Some(raw)) => {
            let preview = crate::pv::thumbs::quick_thumbnail(&raw, 1024)
                .context("this RAW has no embedded preview")?;
            Ok((jpeg(&preview)?, "image/jpeg"))
        }
        (None, None) => Err(anyhow!("Photo {} has no files", target.photo_id)),
    }
}

/// Long edge of the image the histogram is computed from.
const HISTOGRAM_PX: u32 = 512;

/// RGB + luma histogram of what the viewer shows, 256 bins each, scaled 0..=1.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Histogram {
    pub red: Vec<f32>,
    pub green: Vec<f32>,
    pub blue: Vec<f32>,
    pub luma: Vec<f32>,
}

/// Computes a Photo's histogram with RapidRAW's own `calculate_histogram`, from a small
/// rendering of its display copy (or the RAW's embedded preview). The catalog lock is only
/// held for the path lookup, not while decoding.
pub fn histogram(state: &PvState, photo_id: i64) -> Result<Histogram> {
    let (display, raw) = state.read(|cat| files_of(cat, photo_id))?;
    let source = display
        .or(raw)
        .ok_or_else(|| anyhow!("Photo {photo_id} has no files"))?;
    let image = match crate::pv::thumbs::quick_thumbnail(&source, HISTOGRAM_PX) {
        Some(image) => image,
        None => image::open(&source)
            .with_context(|| format!("can't decode {}", source.display()))?
            .thumbnail(HISTOGRAM_PX, HISTOGRAM_PX),
    };
    histogram_of(&image)
}

fn histogram_of(image: &DynamicImage) -> Result<Histogram> {
    use crate::image_processing::{AnalyticsFrame, AnalyticsPixels, calculate_histogram};
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let frame = AnalyticsFrame {
        data: AnalyticsPixels::Owned(rgba.into_raw()),
        width,
        height,
        row_stride: width as usize * 4,
    };
    let data = calculate_histogram(&frame).map_err(|e| anyhow!(e))?;
    Ok(Histogram {
        red: data.red,
        green: data.green,
        blue: data.blue,
        luma: data.luma,
    })
}

/// (display copy, RAW) paths of a Photo.
fn files_of(
    cat: &crate::pv::catalog::Catalog,
    photo_id: i64,
) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
    let (display, raw): (Option<i64>, Option<i64>) = cat.conn().query_row(
        "SELECT p.display_file_id,
                (SELECT m.file_id FROM photo_members m WHERE m.photo_id = p.id AND m.role = 'raw')
         FROM photos p WHERE p.id = ?1",
        [photo_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let path = |f: Option<i64>| f.map(|f| crate::pv::assets::file_path(cat, f)).transpose();
    Ok((path(display)?, path(raw)?))
}

fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("hif" | "heif" | "heic") => "image/heic",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("tif" | "tiff") => "image/tiff",
        _ => "application/octet-stream",
    }
}

fn jpeg(image: &DynamicImage) -> Result<Vec<u8>> {
    let image = if image.width().max(image.height()) > JPEG_MAX_PX {
        image.resize(
            JPEG_MAX_PX,
            JPEG_MAX_PX,
            image::imageops::FilterType::Triangle,
        )
    } else {
        image.clone()
    };
    let mut out = Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 90)
        .encode_image(&image.to_rgb8())?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            parse("/photo/42", None),
            Some(Target {
                photo_id: 42,
                jpeg: false
            })
        );
        assert_eq!(
            parse("/photo/42/", Some("jpeg")),
            Some(Target {
                photo_id: 42,
                jpeg: true
            })
        );
        assert_eq!(
            parse("photo/7", Some("v=3&jpeg")).map(|t| t.jpeg),
            Some(true)
        );
        for bad in [
            "/",
            "/photo/",
            "/photo/x",
            "/file/1",
            "/photo/../etc/passwd",
        ] {
            assert_eq!(parse(bad, None), None, "{bad}");
        }
    }

    #[test]
    fn histogram_of_a_flat_image() {
        let image = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            64,
            48,
            image::Rgb([200, 20, 20]),
        ));
        let h = histogram_of(&image).unwrap();
        assert_eq!(h.red.len(), 256);
        let peak = |v: &[f32]| {
            v.iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0
        };
        // RapidRAW smooths and clips the curve, so the peak can move by a bin or two.
        assert!(peak(&h.red).abs_diff(200) <= 2, "{}", peak(&h.red));
        assert!(peak(&h.green).abs_diff(20) <= 2, "{}", peak(&h.green));
        assert!(h.luma.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    #[test]
    fn mime_types() {
        assert_eq!(mime_for(std::path::Path::new("/a/DSC1.HIF")), "image/heic");
        assert_eq!(mime_for(std::path::Path::new("/a/DSC1.JPG")), "image/jpeg");
    }
}
