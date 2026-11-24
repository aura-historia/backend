//! Images as Workers AI receives them: validated, decodable and within the request limits.

use super::jpeg;
use crate::ImageOmissionReason;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image_fetcher::{Downscaled, FetchedImage, PixelBudget, downscale};

pub(super) const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
/// Clef scales every image down to about this many pixels, keeping its aspect ratio; larger
/// images cost no more tokens and show the model nothing more. Sending images at this budget
/// keeps everything the model sees while keeping four of them far below the request limits.
/// Images within it are re-encoded only when their file is larger than 1 MiB.
const MODEL_BUDGET: PixelBudget = PixelBudget::new(1024 * 1024, 1024 * 1024);

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
    match downscale(mime_type, data, MODEL_BUDGET) {
        Ok(Downscaled::Jpeg(jpeg)) => Ok(PreparedCloudflareImage {
            mime_type: "image/jpeg",
            base64_data: BASE64.encode(jpeg),
        }),
        Ok(Downscaled::Original) => Ok(PreparedCloudflareImage {
            mime_type,
            base64_data: provider_jpeg
                .map_or_else(|| base64_data.to_owned(), |jpeg| BASE64.encode(jpeg)),
        }),
        Err(_) => Err(ImageOmissionReason::InvalidOrOversized),
    }
}

fn supported_mime_type(mime_type: &str) -> Result<&'static str, ImageOmissionReason> {
    match mime_type {
        "image/jpeg" => Ok("image/jpeg"),
        "image/png" => Ok("image/png"),
        "image/webp" => Ok("image/webp"),
        _ => Err(ImageOmissionReason::UnsupportedFormat),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

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

    #[test]
    fn should_send_an_image_within_the_pixel_budget_unchanged() {
        let png = encode(&gradient(640, 480), ImageFormat::Png);

        let prepared = prepare("image/png", &png);

        assert_eq!("image/png", prepared.mime_type());
        assert_eq!(BASE64.encode(&png), prepared.base64_data());
    }

    #[test]
    fn should_send_a_larger_image_as_a_jpeg_at_clefs_pixel_budget() {
        let jpeg = encode(&gradient(3000, 2000), ImageFormat::Jpeg);

        let prepared = prepare("image/jpeg", &jpeg);

        assert_eq!("image/jpeg", prepared.mime_type());
        let sent = image::load_from_memory_with_format(
            &BASE64.decode(prepared.base64_data()).unwrap(),
            ImageFormat::Jpeg,
        )
        .unwrap();
        let pixels = u64::from(sent.width()) * u64::from(sent.height());
        assert!((1024 * 1024 * 99 / 100..=1024 * 1024).contains(&pixels));
    }

    #[test]
    fn should_omit_an_image_it_cannot_decode() {
        assert_eq!(
            Err(ImageOmissionReason::InvalidOrOversized),
            cloudflare_image_parts("image/png", &BASE64.encode(b"not a png"))
        );
    }
}
