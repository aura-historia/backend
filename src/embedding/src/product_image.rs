//! The image a product embedding sends. `gemini-embedding-2` resizes every image to 768 × 768
//! pixels itself and bills a flat 258 tokens per image, so an image is scaled down to about that
//! many pixels: the model sees the same, and each staged image stays small.

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image_fetcher::{DownscaleError, Downscaled, PixelBudget, downscale};

/// The model's input resolution in pixels. Images within it are re-encoded only when their file
/// is larger than 512 KiB, which a JPEG of that size never needs.
const EMBEDDING_PIXEL_BUDGET: PixelBudget = PixelBudget::new(768 * 768, 512 * 1024);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddingImage {
    pub(crate) mime_type: &'static str,
    pub(crate) bytes: Vec<u8>,
}

/// Why an embedding is computed without its listing's image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageOmission {
    /// The model does not read the format, such as GIF.
    UnsupportedFormat,
    InvalidOrOversized,
}

impl ImageOmission {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "unsupported_format",
            Self::InvalidOrOversized => "invalid_or_oversized",
        }
    }
}

/// The fetched image at the model's resolution. HEIC and HEIF, which the model reads but this
/// adapter cannot decode, are sent as fetched; the fetcher bounds their size.
pub(crate) fn embedding_image(
    mime_type: &'static str,
    base64_data: &str,
) -> Result<EmbeddingImage, ImageOmission> {
    let bytes = BASE64
        .decode(base64_data)
        .map_err(|_| ImageOmission::InvalidOrOversized)?;
    if matches!(mime_type, "image/heic" | "image/heif") {
        return Ok(EmbeddingImage { mime_type, bytes });
    }
    match downscale(mime_type, &bytes, EMBEDDING_PIXEL_BUDGET) {
        Ok(Downscaled::Original) => Ok(EmbeddingImage { mime_type, bytes }),
        Ok(Downscaled::Jpeg(jpeg)) => Ok(EmbeddingImage {
            mime_type: "image/jpeg",
            bytes: jpeg,
        }),
        Err(DownscaleError::UnsupportedFormat) => Err(ImageOmission::UnsupportedFormat),
        Err(DownscaleError::Undecodable) => Err(ImageOmission::InvalidOrOversized),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::io::Cursor;

    fn encoded(width: u32, height: u32, format: ImageFormat) -> String {
        let image = DynamicImage::ImageRgb8(RgbImage::from_fn(width, height, |x, y| {
            Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        }));
        let mut output = Cursor::new(Vec::new());
        image.write_to(&mut output, format).unwrap();
        BASE64.encode(output.into_inner())
    }

    #[test]
    fn should_scale_a_larger_image_down_to_the_models_resolution() {
        let image = embedding_image("image/png", &encoded(2400, 1600, ImageFormat::Png)).unwrap();

        assert_eq!("image/jpeg", image.mime_type);
        let sent = image::load_from_memory_with_format(&image.bytes, ImageFormat::Jpeg).unwrap();
        let pixels = u64::from(sent.width()) * u64::from(sent.height());
        assert!((768 * 768 * 99 / 100..=768 * 768).contains(&pixels));
    }

    #[test]
    fn should_send_an_image_within_the_models_resolution_as_fetched() {
        let png = encoded(640, 480, ImageFormat::Png);

        let image = embedding_image("image/png", &png).unwrap();

        assert_eq!("image/png", image.mime_type);
        assert_eq!(BASE64.decode(&png).unwrap(), image.bytes);
    }

    #[test]
    fn should_send_heic_as_fetched_and_omit_formats_the_model_does_not_read() {
        let heic = BASE64.encode(b"heic bytes");

        assert_eq!(
            b"heic bytes".to_vec(),
            embedding_image("image/heic", &heic).unwrap().bytes
        );
        assert_eq!(
            Err(ImageOmission::UnsupportedFormat),
            embedding_image("image/gif", &BASE64.encode(b"GIF87a"))
        );
        assert_eq!(
            Err(ImageOmission::InvalidOrOversized),
            embedding_image("image/png", &BASE64.encode(b"not a png"))
        );
    }
}
