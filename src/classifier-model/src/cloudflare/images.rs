//! Images as Workers AI receives them: validated, decodable and within the request limits.

use super::jpeg;
use crate::ImageOmissionReason;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{
    DynamicImage, GenericImageView, ImageDecoder, ImageFormat, ImageReader, Limits, Rgb, RgbImage,
};
use image_fetcher::FetchedImage;
use std::io::Cursor;

pub(super) const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// Clef scales every image down to about this many pixels, keeping its aspect ratio; larger
/// images cost no more tokens and show the model nothing more. Sending images at this budget
/// keeps everything the model sees while keeping four of them far below the request limits.
const MODEL_PIXEL_BUDGET: u64 = 1024 * 1024;
/// Images within the pixel budget are re-encoded only when their file is larger than this.
const REENCODE_ABOVE_BYTES: usize = 1024 * 1024;
const JPEG_QUALITY: u8 = 90;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PreparedCloudflareImage {
    pub(super) mime_type: &'static str,
    pub(super) base64_data: String,
}

impl PreparedCloudflareImage {
    pub(super) fn decoded_bytes(&self) -> usize {
        self.base64_data.len() / 4 * 3
            - self
                .base64_data
                .bytes()
                .rev()
                .take_while(|byte| *byte == b'=')
                .count()
    }
    pub(super) fn mime_type(&self) -> &'static str {
        self.mime_type
    }

    pub(super) fn base64_data(&self) -> &str {
        &self.base64_data
    }
}

pub(super) fn cloudflare_image(
    image: FetchedImage,
) -> Result<PreparedCloudflareImage, ImageOmissionReason> {
    cloudflare_image_parts(image.mime_type(), image.base64_data())
}

pub(super) fn cloudflare_image_parts(
    mime_type: &str,
    base64_data: &str,
) -> Result<PreparedCloudflareImage, ImageOmissionReason> {
    let mime_type = supported_mime_type(mime_type)?;
    let data = BASE64
        .decode(base64_data)
        .map_err(|_| ImageOmissionReason::InvalidOrOversized)?;
    if data.len() > MAX_IMAGE_BYTES {
        return Err(ImageOmissionReason::InvalidOrOversized);
    }
    let provider_jpeg = (mime_type == "image/jpeg")
        .then(|| jpeg::without_multi_picture_index(&data))
        .flatten();
    let data = provider_jpeg.as_deref().unwrap_or(&data);
    let decoded = decode(mime_type, data).ok_or(ImageOmissionReason::InvalidOrOversized)?;
    if let Some(jpeg) = reencoded_for_model(decoded, data.len()) {
        return Ok(PreparedCloudflareImage {
            mime_type: "image/jpeg",
            base64_data: BASE64.encode(jpeg),
        });
    }
    Ok(PreparedCloudflareImage {
        mime_type,
        base64_data: provider_jpeg
            .map_or_else(|| base64_data.to_owned(), |jpeg| BASE64.encode(jpeg)),
    })
}

fn supported_mime_type(mime_type: &str) -> Result<&'static str, ImageOmissionReason> {
    match mime_type {
        "image/jpeg" => Ok("image/jpeg"),
        "image/png" => Ok("image/png"),
        "image/webp" => Ok("image/webp"),
        _ => Err(ImageOmissionReason::UnsupportedFormat),
    }
}

struct DecodedImage {
    image: DynamicImage,
    orientation: Orientation,
}

fn decode(mime_type: &str, bytes: &[u8]) -> Option<DecodedImage> {
    let format = match mime_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => return None,
    };
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

/// A JPEG at the model's pixel budget, upright and opaque, when the image exceeds the budget or
/// its file is large; `None` keeps the original file. A failed encode keeps it too: the original
/// is a valid image, only larger.
fn reencoded_for_model(decoded: DecodedImage, file_bytes: usize) -> Option<Vec<u8>> {
    let (width, height) = decoded.image.dimensions();
    let over_budget = u64::from(width) * u64::from(height) > MODEL_PIXEL_BUDGET;
    if !over_budget && file_bytes <= REENCODE_ABOVE_BYTES {
        return None;
    }
    let mut image = decoded.image;
    image.apply_orientation(decoded.orientation);
    let (width, height) = within_pixel_budget(image.width(), image.height());
    if (width, height) != image.dimensions() {
        image = image.resize_exact(width, height, FilterType::CatmullRom);
    }
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&opaque_rgb(image))
        .ok()?;
    (over_budget || jpeg.len() < file_bytes).then_some(jpeg)
}

/// The largest size with the same aspect ratio whose pixel count fits the model's budget.
fn within_pixel_budget(width: u32, height: u32) -> (u32, u32) {
    let pixels = u64::from(width) * u64::from(height);
    if pixels <= MODEL_PIXEL_BUDGET {
        return (width, height);
    }
    let scale = (MODEL_PIXEL_BUDGET as f64 / pixels as f64).sqrt();
    let scaled = |side: u32| ((f64::from(side) * scale).floor() as u32).max(1);
    let (width, height) = (scaled(width), scaled(height));
    // A side clamped to one pixel can push the other past the budget; shorten the longer one.
    let longest_within =
        |other: u32| u32::try_from(MODEL_PIXEL_BUDGET / u64::from(other)).unwrap_or(u32::MAX);
    if width >= height {
        (width.min(longest_within(height)), height)
    } else {
        (width, height.min(longest_within(width)))
    }
}

/// JPEG has no transparency: transparent pixels become white, as product photos usually show.
fn opaque_rgb(image: DynamicImage) -> RgbImage {
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

    fn prepare(mime_type: &str, bytes: &[u8]) -> PreparedCloudflareImage {
        cloudflare_image_parts(mime_type, &BASE64.encode(bytes))
            .unwrap_or_else(|reason| panic!("image is prepared: {reason:?}"))
    }

    fn sent(image: &PreparedCloudflareImage) -> DynamicImage {
        let bytes = BASE64
            .decode(image.base64_data())
            .unwrap_or_else(|_| panic!("prepared image is base64"));
        image::load_from_memory_with_format(&bytes, ImageFormat::Jpeg)
            .unwrap_or_else(|_| panic!("prepared image is a JPEG"))
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
    fn should_send_an_image_within_the_pixel_budget_unchanged() {
        let png = encode(&gradient(640, 480), ImageFormat::Png);

        let prepared = prepare("image/png", &png);

        assert_eq!("image/png", prepared.mime_type());
        assert_eq!(BASE64.encode(&png), prepared.base64_data());
    }

    #[test]
    fn should_downscale_an_image_to_the_pixel_budget_when_it_is_larger() {
        for (width, height) in [(2048, 1024), (1500, 1500), (3000, 2000), (600, 2400)] {
            let jpeg = encode(&gradient(width, height), ImageFormat::Jpeg);

            let sent = sent(&prepare("image/jpeg", &jpeg));

            let pixels = u64::from(sent.width()) * u64::from(sent.height());
            assert!(pixels <= MODEL_PIXEL_BUDGET, "{width}x{height}");
            assert!(pixels > MODEL_PIXEL_BUDGET * 99 / 100, "{width}x{height}");
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
            let (fitted_width, fitted_height) = within_pixel_budget(width, height);

            assert!(fitted_width >= 1 && fitted_height >= 1, "{width}x{height}");
            assert!(
                u64::from(fitted_width) * u64::from(fitted_height) <= MODEL_PIXEL_BUDGET,
                "{width}x{height}"
            );
        }
    }

    #[test]
    fn should_turn_transparent_pixels_white_when_reencoding() {
        let transparent =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(1500, 1500, Rgba([0, 0, 0, 0])));
        let png = encode(&transparent, ImageFormat::Png);

        let sent = sent(&prepare("image/png", &png)).into_rgb8();

        assert!(sent.pixels().all(|pixel| pixel.0.iter().all(|c| *c >= 250)));
    }

    #[test]
    fn should_send_the_image_upright_when_its_exif_orientation_rotates_it() {
        let jpeg = with_exif_orientation(&encode(&gradient(2000, 1000), ImageFormat::Jpeg), 6);

        let sent = sent(&prepare("image/jpeg", &jpeg));

        assert!(sent.height() > sent.width());
    }

    #[test]
    fn should_reencode_a_large_file_within_the_pixel_budget() {
        let mut state = 0x2545_f491_u32;
        let noise = DynamicImage::ImageRgb8(RgbImage::from_fn(1000, 1000, |_, _| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let [red, green, blue, _] = state.to_le_bytes();
            Rgb([red, green, blue])
        }));
        let png = encode(&noise, ImageFormat::Png);
        assert!(png.len() > REENCODE_ABOVE_BYTES && png.len() <= MAX_IMAGE_BYTES);

        let prepared = prepare("image/png", &png);

        assert_eq!("image/jpeg", prepared.mime_type());
        assert!(prepared.decoded_bytes() < png.len());
        assert_eq!((1000, 1000), sent(&prepared).dimensions());
    }
}
