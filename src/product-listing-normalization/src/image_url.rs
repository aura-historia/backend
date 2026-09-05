use product_listing_core::product_listing_image::ProductListingImage;
use std::collections::BTreeSet;
use url::Url;

const IMAGE_CANDIDATE_SEPARATOR: char = '\u{1f}';

#[derive(Debug, thiserror::Error)]
pub enum ImageUrlNormalizationError {
    #[error("image URL is invalid")]
    InvalidUrl(#[source] url::ParseError),
}

/// Parses, resolves, preserves first-seen order, and de-duplicates image URLs.
///
/// A unit-separator grouped value is accepted for compatibility with crawler image
/// extraction. It is a generic candidate encoding, not a provider DTO format.
pub fn normalize_image_urls(
    raw_values: impl IntoIterator<Item = String>,
    base_url: &Url,
) -> Result<Vec<ProductListingImage>, ImageUrlNormalizationError> {
    let mut seen = BTreeSet::new();
    let mut images = Vec::new();

    for raw in raw_values {
        let candidate = raw
            .split(IMAGE_CANDIDATE_SEPARATOR)
            .find(|candidate| !candidate.trim().is_empty())
            .unwrap_or_default()
            .trim();
        if candidate.is_empty() {
            continue;
        }
        let url = Url::parse(candidate)
            .or_else(|_| base_url.join(candidate))
            .map_err(ImageUrlNormalizationError::InvalidUrl)?;
        if seen.insert(url.as_str().to_owned()) {
            images.push(ProductListingImage::new(url));
        }
    }

    Ok(images)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_url() -> Url {
        Url::parse("https://example.com/products/123").unwrap_or_else(|error| {
            panic!("test URL must parse: {error}");
        })
    }

    #[test]
    fn should_resolve_relative_urls_and_preserve_order() {
        let images = normalize_image_urls(
            vec!["/images/one.jpg".into(), "images/two.jpg".into()],
            &base_url(),
        )
        .unwrap_or_else(|error| panic!("URLs must normalize: {error}"));
        assert_eq!(
            images[0].url().as_str(),
            "https://example.com/images/one.jpg"
        );
        assert_eq!(
            images[1].url().as_str(),
            "https://example.com/products/images/two.jpg"
        );
    }

    #[test]
    fn should_deduplicate_urls_without_reordering() {
        let images = normalize_image_urls(
            vec![
                "https://cdn.example.com/one.jpg".into(),
                "https://cdn.example.com/one.jpg".into(),
                "https://cdn.example.com/two.jpg".into(),
            ],
            &base_url(),
        )
        .unwrap_or_else(|error| panic!("URLs must normalize: {error}"));
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].url().as_str(), "https://cdn.example.com/one.jpg");
        assert_eq!(images[1].url().as_str(), "https://cdn.example.com/two.jpg");
    }

    #[test]
    fn should_reject_invalid_url() {
        assert!(matches!(
            normalize_image_urls(vec!["//".into()], &base_url()),
            Err(ImageUrlNormalizationError::InvalidUrl(_))
        ));
    }
}
