//! Images as Workers AI receives them: validated, decodable and within the request limits.

use super::jpeg;
use crate::ImageOmissionReason;
use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use image_fetcher::FetchedImage;
use std::io::Cursor;

pub(super) const MAX_IMAGE_BYTES: usize = 4 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 16 * 1024 * 1024;
const MAX_DECODED_IMAGE_BYTES: u64 = 64 * 1024 * 1024;

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
    if !matches!(mime_type, "image/jpeg" | "image/png" | "image/webp") {
        return Err(ImageOmissionReason::UnsupportedFormat);
    }
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
    if !image_is_decodable(mime_type, data) {
        return Err(ImageOmissionReason::InvalidOrOversized);
    }
    Ok(PreparedCloudflareImage {
        mime_type: match mime_type {
            "image/jpeg" => "image/jpeg",
            "image/png" => "image/png",
            "image/webp" => "image/webp",
            _ => return Err(ImageOmissionReason::UnsupportedFormat),
        },
        base64_data: provider_jpeg
            .map_or_else(|| base64_data.to_owned(), |jpeg| BASE64.encode(jpeg)),
    })
}

fn image_is_decodable(mime_type: &str, bytes: &[u8]) -> bool {
    let format = match mime_type {
        "image/jpeg" => ImageFormat::Jpeg,
        "image/png" => ImageFormat::Png,
        "image/webp" => ImageFormat::WebP,
        _ => return false,
    };
    let Ok(mut decoder) = ImageReader::with_format(Cursor::new(bytes), format).into_decoder()
    else {
        return false;
    };
    let (width, height) = decoder.dimensions();
    if u64::from(width)
        .checked_mul(u64::from(height))
        .is_none_or(|pixels| pixels == 0 || pixels > MAX_IMAGE_PIXELS)
    {
        return false;
    }
    let total_bytes = decoder.total_bytes();
    if total_bytes > MAX_DECODED_IMAGE_BYTES {
        return false;
    }
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODED_IMAGE_BYTES);
    if decoder.set_limits(limits).is_err() {
        return false;
    }
    let Ok(output_len) = usize::try_from(total_bytes) else {
        return false;
    };
    let mut output = Vec::new();
    if output.try_reserve_exact(output_len).is_err() {
        return false;
    }
    output.resize(output_len, 0);
    decoder.read_image(&mut output).is_ok()
}
