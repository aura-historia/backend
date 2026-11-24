//! Fetched images at the size a model looks at. A model scales every image down to its own input
//! resolution, so sending more pixels costs transfer and storage without showing it anything more.

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits, Rgb, RgbImage,
};
use std::io::Cursor;

use crate::ImageHash;

/// Images with more pixels are not decoded.
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
const JPEG_QUALITY: u8 = 90;

/// How large an image a model needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelBudget {
    max_pixels: u64,
    reencode_above_bytes: usize,
}

impl PixelBudget {
    /// Images are scaled down to at most `max_pixels`, keeping their aspect ratio. An image
    /// within the budget is re-encoded only when its file is larger than `reencode_above_bytes`.
    pub const fn new(max_pixels: u64, reencode_above_bytes: usize) -> Self {
        Self {
            max_pixels,
            reencode_above_bytes,
        }
    }
}

/// What to send for an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Downscaled {
    /// The original file, which already fits the budget.
    Original,
    /// The image as a JPEG at the budget, upright and opaque.
    Jpeg(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DownscaleError {
    #[error("image format cannot be decoded here")]
    UnsupportedFormat,
    #[error("image is invalid or too large to decode")]
    Undecodable,
}

/// Decodes `bytes`, a JPEG, PNG or WebP file, within fixed memory limits, and re-encodes it as a
/// JPEG at `budget` when it has more pixels than the budget or its file is large. A failed
/// encode keeps the original, which is a valid image, only larger.
pub fn downscale(
    mime_type: &str,
    bytes: &[u8],
    budget: PixelBudget,
) -> Result<Downscaled, DownscaleError> {
    Ok(decode(mime_type, bytes)?.downscale(bytes.len(), budget))
}

/// A JPEG, PNG or WebP file decoded within fixed memory limits.
pub struct DecodedImage {
    image: DynamicImage,
    orientation: Orientation,
}

impl DecodedImage {
    /// The hash of the upright picture, which matches rescaled and re-encoded copies.
    pub fn perceptual_hash(&self) -> ImageHash {
        ImageHash::of(&self.image, self.orientation)
    }

    /// What to send for the image within `budget`, given its file's size in bytes.
    pub fn downscale(self, file_bytes: usize, budget: PixelBudget) -> Downscaled {
        reencoded(self, file_bytes, budget).map_or(Downscaled::Original, Downscaled::Jpeg)
    }
}

/// Decodes `bytes`, a JPEG, PNG or WebP file, within fixed memory limits.
pub fn decode(mime_type: &str, bytes: &[u8]) -> Result<DecodedImage, DownscaleError> {
    let format = match mime_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => return Err(DownscaleError::UnsupportedFormat),
    };
    decode_format(format, bytes).ok_or(DownscaleError::Undecodable)
}

fn decode_format(format: ImageFormat, bytes: &[u8]) -> Option<DecodedImage> {
    let mut decoder = ImageReader::with_format(Cursor::new(bytes), format)
        .into_decoder()
        .ok()?;
    let (width, height) = decoder.dimensions();
    if u64::from(width)
        .checked_mul(u64::from(height))
        .is_none_or(|pixels| pixels == 0 || pixels > MAX_IMAGE_PIXELS)
    {
        return None;
    }
    if decoder.total_bytes() > MAX_DECODED_IMAGE_BYTES {
        return None;
    }
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    decoder.set_limits(limits).ok()?;
    // Orientation only matters for an image that is re-encoded without its metadata; an
    // unreadable tag must not reject an otherwise valid image.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let image = DynamicImage::from_decoder(decoder).ok()?;
    Some(DecodedImage { image, orientation })
}

/// A JPEG at the budget when the image exceeds it or its file is large; `None` keeps the
/// original file.
fn reencoded(decoded: DecodedImage, file_bytes: usize, budget: PixelBudget) -> Option<Vec<u8>> {
    let (width, height) = decoded.image.dimensions();
    let over_budget = u64::from(width) * u64::from(height) > budget.max_pixels;
    if !over_budget && file_bytes <= budget.reencode_above_bytes {
        return None;
    }
    let mut image = decoded.image;
    image.apply_orientation(decoded.orientation);
    let (width, height) = within_pixel_budget(image.width(), image.height(), budget.max_pixels);
    if (width, height) != image.dimensions() {
        image = image.resize_exact(width, height, FilterType::CatmullRom);
    }
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&opaque_rgb(image))
        .ok()?;
    (over_budget || jpeg.len() < file_bytes).then_some(jpeg)
}

/// The largest size with the same aspect ratio whose pixel count fits `max_pixels`.
fn within_pixel_budget(width: u32, height: u32, max_pixels: u64) -> (u32, u32) {
    let pixels = u64::from(width) * u64::from(height);
    if pixels <= max_pixels {
        return (width, height);
    }
    let scale = (max_pixels as f64 / pixels as f64).sqrt();
    let scaled = |side: u32| ((f64::from(side) * scale).floor() as u32).max(1);
    let (width, height) = (scaled(width), scaled(height));
    // A side clamped to one pixel can push the other past the budget; shorten the longer one.
    let longest_within =
        |other: u32| u32::try_from(max_pixels / u64::from(other)).unwrap_or(u32::MAX);
    if width >= height {
        (width.min(longest_within(height)), height)
    } else {
        (width, height.min(longest_within(width)))
    }
}

/// JPEG has no transparency: transparent pixels become white, as product photos usually show.
pub(crate) fn opaque_rgb(image: DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.into_rgb8();
    }
    let rgba = image.into_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [red, green, blue, alpha] = rgba.get_pixel(x, y).0;
        let over_white = |channel: u8| {
            let alpha = u32::from(alpha);
            let blended = (u32::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255;
            u8::try_from(blended).unwrap_or(u8::MAX)
        };
        Rgb([over_white(red), over_white(green), over_white(blue)])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    const BUDGET: PixelBudget = PixelBudget::new(1024 * 1024, 1024 * 1024);

    fn encode(image: &DynamicImage, format: ImageFormat) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image
            .write_to(&mut output, format)
            .unwrap_or_else(|_| panic!("encode test image"));
        output.into_inner()
    }

    fn gradient(width: u32, height: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        }))
    }

    fn sent(downscaled: Downscaled) -> DynamicImage {
        let Downscaled::Jpeg(jpeg) = downscaled else {
            panic!("the image is re-encoded");
        };
        image::load_from_memory_with_format(&jpeg, ImageFormat::Jpeg)
            .unwrap_or_else(|_| panic!("the re-encoded image is a JPEG"))
    }

    /// Inserts an EXIF APP1 segment with the given orientation tag after SOI.
    fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let tiff = [
            &b"MM\0\x2a\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01"[..],
            &orientation.to_be_bytes(),
            &[0, 0, 0, 0, 0, 0],
        ]
        .concat();
        let payload = [&b"Exif\0\0"[..], &tiff].concat();
        let length = u16::try_from(payload.len() + 2).unwrap_or_else(|_| panic!("short APP1"));
        [
            &jpeg[..2],
            &[0xFF, 0xE1],
            &length.to_be_bytes(),
            &payload,
            &jpeg[2..],
        ]
        .concat()
    }

    #[test]
    fn should_keep_an_image_within_the_budget() {
        let png = encode(&gradient(640, 480), ImageFormat::Png);

        assert_eq!(
            Ok(Downscaled::Original),
            downscale("image/png", &png, BUDGET)
        );
    }

    #[test]
    fn should_downscale_an_image_to_the_budget_when_it_is_larger() {
        for (width, height) in [(2048, 1024), (1500, 1500), (3000, 2000), (600, 2400)] {
            let jpeg = encode(&gradient(width, height), ImageFormat::Jpeg);

            let sent = sent(downscale("image/jpeg", &jpeg, BUDGET).unwrap());

            let pixels = u64::from(sent.width()) * u64::from(sent.height());
            assert!(pixels <= BUDGET.max_pixels, "{width}x{height}");
            assert!(pixels > BUDGET.max_pixels * 99 / 100, "{width}x{height}");
            let ratio = f64::from(width) / f64::from(height);
            let sent_ratio = f64::from(sent.width()) / f64::from(sent.height());
            assert!(
                (ratio - sent_ratio).abs() / ratio < 0.01,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn should_keep_every_side_when_an_extreme_aspect_ratio_exceeds_the_budget() {
        for (width, height) in [(16_000_000, 1), (1, 16_000_000), (8_000_000, 2)] {
            let (fitted_width, fitted_height) =
                within_pixel_budget(width, height, BUDGET.max_pixels);

            assert!(fitted_width >= 1 && fitted_height >= 1, "{width}x{height}");
            assert!(
                u64::from(fitted_width) * u64::from(fitted_height) <= BUDGET.max_pixels,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn should_turn_transparent_pixels_white_when_reencoding() {
        let transparent =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1500, 1500, Rgba([0, 0, 0, 0])));
        let png = encode(&transparent, ImageFormat::Png);

        let sent = sent(downscale("image/png", &png, BUDGET).unwrap()).into_rgb8();

        assert!(sent.pixels().all(|pixel| pixel.0.iter().all(|c| *c >= 250)));
    }

    #[test]
    fn should_reencode_the_image_upright_when_its_exif_orientation_rotates_it() {
        let jpeg = with_exif_orientation(&encode(&gradient(2000, 1000), ImageFormat::Jpeg), 6);

        let sent = sent(downscale("image/jpeg", &jpeg, BUDGET).unwrap());

        assert!(sent.height() > sent.width());
    }

    #[test]
    fn should_reencode_a_large_file_within_the_budget() {
        let mut state = 0x2545_f491_u32;
        let noise = DynamicImage::ImageRgb8(RgbImage::from_fn(1000, 1000, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [red, green, blue, _] = state.to_le_bytes();
            Rgb([red, green, blue])
        }));
        let png = encode(&noise, ImageFormat::Png);
        assert!(png.len() > BUDGET.reencode_above_bytes);

        let downscaled = downscale("image/png", &png, BUDGET).unwrap();

        let Downscaled::Jpeg(jpeg) = &downscaled else {
            panic!("the image is re-encoded");
        };
        assert!(jpeg.len() < png.len());
        assert_eq!((1000, 1000), sent(downscaled).dimensions());
    }

    #[test]
    fn should_reject_formats_it_cannot_decode_and_invalid_files() {
        assert_eq!(
            Err(DownscaleError::UnsupportedFormat),
            downscale("image/gif", b"GIF87a", BUDGET)
        );
        assert_eq!(
            Err(DownscaleError::Undecodable),
            downscale("image/png", b"not a png", BUDGET)
        );
    }
}
