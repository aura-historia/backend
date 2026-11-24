//! The image a product embedding sends, on the batch and the synchronous path alike.
//! `gemini-embedding-2` resizes every image to 768 × 768 pixels itself and bills a flat 258
//! tokens per image, so an image is scaled down to about that many pixels: the model sees the
//! same, and each request stays small.

use crate::EmbeddingImageUrl;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image_fetcher::{DownscaleError, Downscaled, ImageFetcher, ImageHash, PixelBudget, decode};

/// The model's input resolution in pixels. Images within it are re-encoded only when their file
/// is larger than 512 KiB, which a JPEG of that size never needs.
const EMBEDDING_PIXEL_BUDGET: PixelBudget = PixelBudget::new(768 * 768, 512 * 1024);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddingImage {
    pub(crate) mime_type: &'static str,
    pub(crate) bytes: Vec<u8>,
    /// `None` for a format this adapter cannot decode.
    pub(crate) hash: Option<ImageHash>,
}

/// Why an embedding is computed without its listing's image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageOmission {
    /// The model does not read the format, such as GIF.
    UnsupportedFormat,
    InvalidOrOversized,
}

impl ImageOmission {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedFormat => "unsupported_format",
            Self::InvalidOrOversized => "invalid_or_oversized",
        }
    }
}

/// The listing's image at the model's resolution, with its perceptual hash. An image that cannot
/// be fetched or sent is left out of the embedding; its bounded cause is logged.
pub(crate) async fn fetch_embedding_image(
    fetcher: &ImageFetcher,
    image_url: &EmbeddingImageUrl,
) -> Option<EmbeddingImage> {
    prepare_fetched(
        fetcher,
        image_url,
        embedding_image,
        "EMBEDDING_IMAGE_OMITTED",
    )
    .await
}

/// The perceptual hash of the image the embedding would send, or `None` when it would send none
/// or this adapter cannot decode it.
pub(crate) async fn fetch_embedding_image_hash(
    fetcher: &ImageFetcher,
    image_url: &EmbeddingImageUrl,
) -> Option<ImageHash> {
    prepare_fetched(
        fetcher,
        image_url,
        embedding_image_hash,
        "EMBEDDING_IMAGE_UNHASHED",
    )
    .await
    .flatten()
}

/// Fetches the image and prepares it with `prepare`; a failure is logged as `event_type` with its
/// bounded cause.
async fn prepare_fetched<T: Send + 'static>(
    fetcher: &ImageFetcher,
    image_url: &EmbeddingImageUrl,
    prepare: fn(&'static str, &str) -> Result<T, ImageOmission>,
    event_type: &'static str,
) -> Option<T> {
    let fetched = fetcher
        .fetch(image_url.as_url())
        .await
        .inspect_err(|error| {
            let fetch_failure = error.diagnostics();
            tracing::info!(
                eventType = event_type,
                imageOmissionReason = "fetch_failed",
                imageFetchFailure = fetch_failure.category,
                imageFetchHttpStatus = fetch_failure.http_status,
                imageFetchAttempts = fetch_failure.attempts,
                "Embedding image omitted."
            );
        })
        .ok()?;
    // Decoding and resizing are CPU-bound; they run off the async workers. A panic in the
    // decoder leaves only this image out.
    let prepared =
        tokio::task::spawn_blocking(move || prepare(fetched.mime_type(), fetched.base64_data()))
            .await
            .unwrap_or(Err(ImageOmission::InvalidOrOversized));
    prepared
        .inspect_err(|omission| {
            tracing::info!(
                eventType = event_type,
                imageOmissionReason = omission.as_str(),
                "Embedding image omitted."
            );
        })
        .ok()
}

/// The fetched image at the model's resolution. HEIC and HEIF, which the model reads but this
/// adapter cannot decode, are sent as fetched and without a hash; the fetcher bounds their size.
fn embedding_image(
    mime_type: &'static str,
    base64_data: &str,
) -> Result<EmbeddingImage, ImageOmission> {
    let bytes = BASE64
        .decode(base64_data)
        .map_err(|_| ImageOmission::InvalidOrOversized)?;
    if is_undecodable_but_read_by_the_model(mime_type) {
        return Ok(EmbeddingImage {
            mime_type,
            bytes,
            hash: None,
        });
    }
    let decoded = decode(mime_type, &bytes).map_err(ImageOmission::from)?;
    let hash = Some(decoded.perceptual_hash());
    match decoded.downscale(bytes.len(), EMBEDDING_PIXEL_BUDGET) {
        Downscaled::Original => Ok(EmbeddingImage {
            mime_type,
            bytes,
            hash,
        }),
        Downscaled::Jpeg(jpeg) => Ok(EmbeddingImage {
            mime_type: "image/jpeg",
            bytes: jpeg,
            hash,
        }),
    }
}

/// The hash [`embedding_image`] records for the fetched image, without re-encoding it.
fn embedding_image_hash(
    mime_type: &'static str,
    base64_data: &str,
) -> Result<Option<ImageHash>, ImageOmission> {
    if is_undecodable_but_read_by_the_model(mime_type) {
        return Ok(None);
    }
    let bytes = BASE64
        .decode(base64_data)
        .map_err(|_| ImageOmission::InvalidOrOversized)?;
    let decoded = decode(mime_type, &bytes).map_err(ImageOmission::from)?;
    Ok(Some(decoded.perceptual_hash()))
}

fn is_undecodable_but_read_by_the_model(mime_type: &str) -> bool {
    matches!(mime_type, "image/heic" | "image/heif")
}

impl From<DownscaleError> for ImageOmission {
    fn from(error: DownscaleError) -> Self {
        match error {
            DownscaleError::UnsupportedFormat => Self::UnsupportedFormat,
            DownscaleError::Undecodable => Self::InvalidOrOversized,
        }
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
    fn should_hash_the_sent_image_as_the_comparison_does() {
        for png in [
            encoded(2400, 1600, ImageFormat::Png),
            encoded(640, 480, ImageFormat::Png),
        ] {
            let image = embedding_image("image/png", &png).unwrap();

            assert!(image.hash.is_some());
            assert_eq!(image.hash, embedding_image_hash("image/png", &png).unwrap());
        }
    }

    #[test]
    fn should_send_heic_as_fetched_and_omit_formats_the_model_does_not_read() {
        let heic = BASE64.encode(b"heic bytes");

        let sent = embedding_image("image/heic", &heic).unwrap();
        assert_eq!((b"heic bytes".to_vec(), None), (sent.bytes, sent.hash));
        assert_eq!(Ok(None), embedding_image_hash("image/heic", &heic));
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
