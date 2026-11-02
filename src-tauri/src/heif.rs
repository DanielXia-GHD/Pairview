//! Pairview: HEIF / Sony HIF support.
//!
//! On macOS the decode goes through Apple ImageIO, which uses the hardware HEVC
//! decoder and understands Sony's 10-bit 4:2:2 HIF. Other platforms are not
//! supported yet (see docs/PRD.md §10.1).

use std::path::Path;

pub const HEIF_EXTENSIONS: &[&str] = &["hif", "heif", "heic"];

pub fn is_heif_file<P: AsRef<Path>>(path: P) -> bool {
    path.as_ref()
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| HEIF_EXTENSIONS.iter().any(|h| h.eq_ignore_ascii_case(ext)))
}

#[cfg(not(target_os = "macos"))]
pub fn decode_heif(_bytes: &[u8]) -> anyhow::Result<image::DynamicImage> {
    Err(anyhow::anyhow!("HEIF decoding is only available on macOS"))
}

#[cfg(not(target_os = "macos"))]
pub fn heif_thumbnail(_bytes: &[u8], _max_px: u32) -> anyhow::Result<image::DynamicImage> {
    Err(anyhow::anyhow!("HEIF decoding is only available on macOS"))
}

#[cfg(target_os = "macos")]
pub use imp::{decode_heif, heif_thumbnail};

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{Result, anyhow};
    use image::{DynamicImage, ImageBuffer, Rgba};
    use objc2_core_foundation::{
        CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
        CGSize,
    };
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapInfo, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
        kCGColorSpaceSRGB,
    };
    use objc2_image_io::{
        CGImageSource, kCGImagePropertyOrientation, kCGImageSourceCreateThumbnailFromImageIfAbsent,
        kCGImageSourceCreateThumbnailWithTransform, kCGImageSourceShouldCache,
        kCGImageSourceThumbnailMaxPixelSize,
    };
    use rawler::Orientation;
    use std::time::Instant;

    use crate::image_processing::apply_orientation;

    fn options(pairs: &[(&CFString, &CFType)]) -> CFRetained<CFDictionary<CFString, CFType>> {
        let keys: Vec<&CFString> = pairs.iter().map(|(k, _)| *k).collect();
        let values: Vec<&CFType> = pairs.iter().map(|(_, v)| *v).collect();
        CFDictionary::from_slices(&keys, &values)
    }

    fn open_source(bytes: &[u8]) -> Result<CFRetained<CGImageSource>> {
        let data = CFData::from_bytes(bytes);
        let no_cache = options(&[(unsafe { kCGImageSourceShouldCache }, CFBoolean::new(false))]);
        unsafe { CGImageSource::with_data(&data, Some(no_cache.as_opaque())) }
            .ok_or_else(|| anyhow!("ImageIO could not open the HEIF data"))
    }

    /// Draws a CGImage into a 16-bit RGBA sRGB buffer, preserving the 10-bit HIF precision.
    fn draw_rgba16(image: &CGImage) -> Result<ImageBuffer<Rgba<u16>, Vec<u16>>> {
        let width = CGImage::width(Some(image));
        let height = CGImage::height(Some(image));
        if width == 0 || height == 0 {
            return Err(anyhow!("HEIF image has an empty size"));
        }
        let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
            .ok_or_else(|| anyhow!("Could not create sRGB color space"))?;

        let mut buf = vec![0u16; width * height * 4];
        #[allow(deprecated)]
        let bitmap_info =
            CGImageAlphaInfo::PremultipliedLast.0 | CGBitmapInfo::ByteOrder16Little.bits();
        let context = unsafe {
            CGBitmapContextCreate(
                buf.as_mut_ptr().cast(),
                width,
                height,
                16,
                width * 4 * std::mem::size_of::<u16>(),
                Some(&color_space),
                bitmap_info,
            )
        }
        .ok_or_else(|| anyhow!("Could not create a 16-bit bitmap context"))?;

        let rect = CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width as f64, height as f64),
        );
        CGContext::draw_image(Some(&context), rect, Some(image));
        drop(context);

        ImageBuffer::from_raw(width as u32, height as u32, buf)
            .ok_or_else(|| anyhow!("HEIF buffer size mismatch"))
    }

    /// HEIF stores rotation in the container (`irot`/`imir`), not in EXIF.
    /// ImageIO folds both into the standard orientation property.
    fn orientation(source: &CGImageSource) -> Option<i32> {
        let props = unsafe { source.properties_at_index(0, None) }?;
        let props: CFRetained<CFDictionary<CFString, CFType>> =
            unsafe { CFRetained::cast_unchecked(props) };
        props
            .get(unsafe { kCGImagePropertyOrientation })?
            .downcast::<CFNumber>()
            .ok()?
            .as_i32()
    }

    /// Full-resolution decode, returned in the same shape as other non-RAW images
    /// (`ImageRgb32F`, sRGB-encoded, orientation applied).
    pub fn decode_heif(bytes: &[u8]) -> Result<DynamicImage> {
        let start = Instant::now();
        let source = open_source(bytes)?;
        let image = unsafe { source.image_at_index(0, None) }
            .ok_or_else(|| anyhow!("ImageIO could not decode the HEIF image"))?;
        let rgba16 = draw_rgba16(&image)?;
        let decoded = DynamicImage::ImageRgb32F(DynamicImage::ImageRgba16(rgba16).to_rgb32f());

        let oriented = match orientation(&source) {
            Some(o) if o > 1 => apply_orientation(decoded, Orientation::from_u16(o as u16)),
            _ => decoded,
        };
        log::info!(
            "HEIF decoded via ImageIO ({}x{}) in {:?}",
            oriented.width(),
            oriented.height(),
            start.elapsed()
        );
        Ok(oriented)
    }

    /// Fast thumbnail: uses the embedded preview when it is large enough, orientation applied.
    pub fn heif_thumbnail(bytes: &[u8], max_px: u32) -> Result<DynamicImage> {
        let source = open_source(bytes)?;
        let max = CFNumber::new_i32(max_px as i32);
        let opts = options(&[
            (
                unsafe { kCGImageSourceCreateThumbnailFromImageIfAbsent },
                CFBoolean::new(true),
            ),
            (unsafe { kCGImageSourceCreateThumbnailWithTransform }, CFBoolean::new(true)),
            (unsafe { kCGImageSourceThumbnailMaxPixelSize }, &max),
        ]);
        let image = unsafe { source.thumbnail_at_index(0, Some(opts.as_opaque())) }
            .ok_or_else(|| anyhow!("ImageIO could not create a HEIF thumbnail"))?;
        let rgba16 = draw_rgba16(&image)?;
        Ok(DynamicImage::ImageRgb8(DynamicImage::ImageRgba16(rgba16).to_rgb8()))
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// Run with a real Sony file: `PV_HIF_SAMPLE=/path/DSC0001.HIF cargo test --lib heif -- --nocapture`
    #[test]
    fn decodes_sample_hif() {
        let Ok(path) = std::env::var("PV_HIF_SAMPLE") else {
            eprintln!("PV_HIF_SAMPLE not set; skipping");
            return;
        };
        let bytes = std::fs::read(&path).unwrap();
        assert!(is_heif_file(&path));

        let start = std::time::Instant::now();
        let full = decode_heif(&bytes).unwrap();
        eprintln!("full {}x{} in {:?}", full.width(), full.height(), start.elapsed());
        assert!(matches!(full, image::DynamicImage::ImageRgb32F(_)));
        assert_eq!(full.width().max(full.height()), 7008);

        let start = std::time::Instant::now();
        let thumb = heif_thumbnail(&bytes, 1280).unwrap();
        eprintln!("thumb {}x{} in {:?}", thumb.width(), thumb.height(), start.elapsed());
        assert_eq!(thumb.width().max(thumb.height()), 1280);
        // Orientation must agree between the full decode and the thumbnail.
        assert_eq!(full.width() > full.height(), thumb.width() > thumb.height());
    }
}
