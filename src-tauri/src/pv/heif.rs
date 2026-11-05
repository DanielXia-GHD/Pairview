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

#[cfg(not(target_os = "macos"))]
pub fn encode_heif(
    _image: &image::DynamicImage,
    _metadata_from: Option<&[u8]>,
    _quality: f32,
) -> anyhow::Result<Vec<u8>> {
    Err(anyhow::anyhow!("HEIF encoding is only available on macOS"))
}

#[cfg(target_os = "macos")]
pub use imp::{decode_heif, heif_thumbnail, properties};

// Used by the edit-overwrite flow in M3 (docs/IMPLEMENTATION_PLAN.md, M3-02).
#[cfg(target_os = "macos")]
#[allow(unused_imports)]
pub use imp::encode_heif;

#[cfg(target_os = "macos")]
mod imp {
    use anyhow::{Result, anyhow};
    use image::{DynamicImage, ImageBuffer, Rgba};
    use objc2_core_foundation::{
        CFBoolean, CFData, CFDictionary, CFMutableData, CFMutableDictionary, CFNumber, CFRetained,
        CFString, CFType, CGPoint, CGRect, CGSize,
    };
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGBitmapInfo, CGColorSpace, CGContext,
        CGImage, CGImageAlphaInfo, kCGColorSpaceSRGB,
    };
    use objc2_image_io::{
        CGImageDestination, CGImageSource, kCGImageDestinationLossyCompressionQuality,
        kCGImagePropertyDepth, kCGImagePropertyOrientation,
        kCGImageSourceCreateThumbnailFromImageAlways,
        kCGImageSourceCreateThumbnailFromImageIfAbsent, kCGImageSourceCreateThumbnailWithTransform,
        kCGImageSourceShouldCache, kCGImageSourceThumbnailMaxPixelSize,
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

    /// A 16-bit RGBA sRGB bitmap context drawing into `buf` (which must outlive the context).
    fn rgba16_context(
        buf: &mut [u16],
        width: usize,
        height: usize,
    ) -> Result<CFRetained<CGContext>> {
        assert_eq!(buf.len(), width * height * 4);
        let color_space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
            .ok_or_else(|| anyhow!("Could not create sRGB color space"))?;
        #[allow(deprecated)]
        let bitmap_info =
            CGImageAlphaInfo::PremultipliedLast.0 | CGBitmapInfo::ByteOrder16Little.bits();
        unsafe {
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
        .ok_or_else(|| anyhow!("Could not create a 16-bit bitmap context"))
    }

    /// Draws a CGImage into a 16-bit RGBA sRGB buffer, preserving the 10-bit HIF precision.
    fn draw_rgba16(image: &CGImage) -> Result<ImageBuffer<Rgba<u16>, Vec<u16>>> {
        let width = CGImage::width(Some(image));
        let height = CGImage::height(Some(image));
        if width == 0 || height == 0 {
            return Err(anyhow!("HEIF image has an empty size"));
        }
        let mut buf = vec![0u16; width * height * 4];
        let context = rgba16_context(&mut buf, width, height)?;

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

    /// All ImageIO properties of the first image (EXIF, TIFF, GPS, … as nested dictionaries).
    pub fn properties(bytes: &[u8]) -> Option<CFRetained<CFDictionary<CFString, CFType>>> {
        let props = unsafe { open_source(bytes).ok()?.properties_at_index(0, None) }?;
        Some(unsafe { CFRetained::cast_unchecked(props) })
    }

    #[cfg(test)]
    pub(super) fn depth(bytes: &[u8]) -> Option<i32> {
        properties(bytes)?
            .get(unsafe { kCGImagePropertyDepth })?
            .downcast::<CFNumber>()
            .ok()?
            .as_i32()
    }

    /// (DateTimeOriginal, Model) as ImageIO sees them. Sony HIF EXIF is not readable by kamadak-exif.
    #[cfg(test)]
    pub(super) fn date_and_model(bytes: &[u8]) -> (Option<String>, Option<String>) {
        use objc2_image_io::{
            kCGImagePropertyExifDateTimeOriginal, kCGImagePropertyExifDictionary,
            kCGImagePropertyTIFFDictionary, kCGImagePropertyTIFFModel,
        };
        let Some(props) = properties(bytes) else {
            return (None, None);
        };
        let nested = |dict: &CFString, key: &CFString| -> Option<String> {
            let sub = props.get(dict)?.downcast::<CFDictionary>().ok()?;
            let sub: CFRetained<CFDictionary<CFString, CFType>> =
                unsafe { CFRetained::cast_unchecked(sub) };
            Some(sub.get(key)?.downcast::<CFString>().ok()?.to_string())
        };
        unsafe {
            (
                nested(
                    kCGImagePropertyExifDictionary,
                    kCGImagePropertyExifDateTimeOriginal,
                ),
                nested(kCGImagePropertyTIFFDictionary, kCGImagePropertyTIFFModel),
            )
        }
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

    /// Fast thumbnail, orientation applied: from the embedded preview when that is at least
    /// `max_px` on the long edge (Sony: ~1620 px), otherwise rendered from the full image.
    pub fn heif_thumbnail(bytes: &[u8], max_px: u32) -> Result<DynamicImage> {
        let source = open_source(bytes)?;
        let max = CFNumber::new_i32(max_px as i32);
        let thumbnail = |from_image: &CFString| {
            let opts = options(&[
                (from_image, CFBoolean::new(true)),
                (
                    unsafe { kCGImageSourceCreateThumbnailWithTransform },
                    CFBoolean::new(true),
                ),
                (unsafe { kCGImageSourceThumbnailMaxPixelSize }, &max),
            ]);
            unsafe { source.thumbnail_at_index(0, Some(opts.as_opaque())) }
        };
        let mut image = thumbnail(unsafe { kCGImageSourceCreateThumbnailFromImageIfAbsent })
            .ok_or_else(|| anyhow!("ImageIO could not create a HEIF thumbnail"))?;
        if CGImage::width(Some(&image)).max(CGImage::height(Some(&image))) < max_px as usize {
            // The embedded preview is too small: ImageIO renders from the full image instead.
            if let Some(full) = thumbnail(unsafe { kCGImageSourceCreateThumbnailFromImageAlways }) {
                image = full;
            }
        }
        let rgba16 = draw_rgba16(&image)?;
        Ok(DynamicImage::ImageRgb8(
            DynamicImage::ImageRgba16(rgba16).to_rgb8(),
        ))
    }

    /// Encodes an (already upright) image as 10-bit HEIF. Metadata (EXIF, GPS, color profile) is
    /// copied from `metadata_from`, the original file, with orientation reset to 1.
    #[allow(dead_code)]
    pub fn encode_heif(
        image: &DynamicImage,
        metadata_from: Option<&[u8]>,
        quality: f32,
    ) -> Result<Vec<u8>> {
        let start = Instant::now();
        let mut rgba16 = image.to_rgba16();
        let (width, height) = (rgba16.width() as usize, rgba16.height() as usize);
        let context = rgba16_context(&mut rgba16, width, height)?;
        // Copies the pixels, so `rgba16` may be dropped afterwards.
        let cg_image = CGBitmapContextCreateImage(Some(&context))
            .ok_or_else(|| anyhow!("Could not create a CGImage from the edited pixels"))?;
        drop(context);

        let base = match metadata_from {
            Some(bytes) => unsafe { open_source(bytes)?.properties_at_index(0, None) },
            None => None,
        }
        .unwrap_or_else(|| unsafe { CFRetained::cast_unchecked(options(&[])) });
        let props = unsafe { CFMutableDictionary::new_copy(None, 0, Some(&base)) }
            .ok_or_else(|| anyhow!("Could not copy image properties"))?;
        let props: CFRetained<CFMutableDictionary<CFString, CFType>> =
            unsafe { CFRetained::cast_unchecked(props) };
        props.set(unsafe { kCGImagePropertyDepth }, &CFNumber::new_i32(10));
        props.set(
            unsafe { kCGImagePropertyOrientation },
            &CFNumber::new_i32(1),
        );
        props.set(
            unsafe { kCGImageDestinationLossyCompressionQuality },
            &CFNumber::new_f64(quality.clamp(0.0, 1.0) as f64),
        );

        let out =
            CFMutableData::new(None, 0).ok_or_else(|| anyhow!("Could not allocate output"))?;
        let heic = CFString::from_static_str("public.heic");
        let destination = unsafe { CGImageDestination::with_data(&out, &heic, 1, None) }
            .ok_or_else(|| anyhow!("ImageIO has no HEIC encoder"))?;
        unsafe { destination.add_image(&cg_image, Some(props.as_opaque())) };
        if !unsafe { destination.finalize() } {
            return Err(anyhow!("ImageIO failed to write the HEIF image"));
        }
        let bytes = out.to_vec();
        log::info!(
            "HEIF encoded ({}x{}, {} KB) in {:?}",
            width,
            height,
            bytes.len() / 1024,
            start.elapsed()
        );
        Ok(bytes)
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
        eprintln!(
            "full {}x{} in {:?}",
            full.width(),
            full.height(),
            start.elapsed()
        );
        assert!(matches!(full, image::DynamicImage::ImageRgb32F(_)));
        assert_eq!(full.width().max(full.height()), 7008);

        let start = std::time::Instant::now();
        let thumb = heif_thumbnail(&bytes, 1280).unwrap();
        eprintln!(
            "thumb {}x{} in {:?}",
            thumb.width(),
            thumb.height(),
            start.elapsed()
        );
        assert_eq!(thumb.width().max(thumb.height()), 1280);
        // Bigger than the embedded preview: rendered from the full image (viewer JPEG fallback).
        let big = heif_thumbnail(&bytes, 4096).unwrap();
        assert_eq!(big.width().max(big.height()), 4096);
        // Orientation must agree between the full decode and the thumbnail.
        assert_eq!(full.width() > full.height(), thumb.width() > thumb.height());
    }

    /// Decode → encode → decode must keep size, orientation, 10-bit depth and camera metadata.
    #[test]
    fn encode_round_trip() {
        let Ok(path) = std::env::var("PV_HIF_SAMPLE") else {
            eprintln!("PV_HIF_SAMPLE not set; skipping");
            return;
        };
        let original = std::fs::read(&path).unwrap();
        let decoded = decode_heif(&original).unwrap();

        let start = std::time::Instant::now();
        let encoded = encode_heif(&decoded, Some(&original), 0.9).unwrap();
        eprintln!(
            "encoded {} KB in {:?}",
            encoded.len() / 1024,
            start.elapsed()
        );

        assert_eq!(imp::depth(&encoded), Some(10));
        let again = decode_heif(&encoded).unwrap();
        assert_eq!(
            (again.width(), again.height()),
            (decoded.width(), decoded.height())
        );
        let (date, model) = imp::date_and_model(&original);
        eprintln!("original date={date:?} model={model:?}");
        assert!(
            date.is_some() && model.is_some(),
            "ImageIO should read Sony HIF metadata"
        );
        assert_eq!(imp::date_and_model(&encoded), (date, model));
    }
}
