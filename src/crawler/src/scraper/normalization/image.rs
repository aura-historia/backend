use super::error::NormalizationError;
use crate::scraper::css_selector::rule::split_image_candidate_group;
use product_core::{product_image::ProductImage, prohibited_content::ProhibitedContent};
use url::Url;

/// Converts a list of raw image URL strings into [`ProductImage`] values.
///
/// Each string is first trimmed. Absolute URLs are parsed directly; relative
/// paths are resolved against `base_url`. All images start with
/// [`ProhibitedContent::Unknown`] — content moderation runs separately.
pub(super) fn normalize_images(
    raw: Vec<String>,
    base_url: &Url,
) -> Result<Vec<ProductImage>, NormalizationError> {
    raw.into_iter()
        .map(|s| {
            let s = s.trim().to_owned();
            let image_url = split_image_candidate_group(&s)
                .into_iter()
                .next()
                .unwrap_or(s.as_str())
                .to_owned();
            let url = Url::parse(&image_url)
                .or_else(|_| base_url.join(&image_url))
                .map_err(|source| NormalizationError::InvalidImageUrl {
                    raw: image_url.clone(),
                    source,
                })?;
            Ok(ProductImage {
                url,
                prohibited_content: ProhibitedContent::Unknown,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use rstest::rstest;
    use url::Url;

    use product_core::prohibited_content::ProhibitedContent;

    use super::normalize_images;
    use crate::scraper::normalization::error::NormalizationError;

    fn base_url() -> Url {
        Url::parse("https://example.com/products/123").unwrap()
    }

    // -----------------------------------------------------------------------
    // Successful cases
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_empty_vec_when_no_images_provided() {
        let result = normalize_images(vec![], &base_url()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn should_normalize_images_when_absolute_urls_provided() {
        let result =
            normalize_images(vec!["https://cdn.example.com/img.jpg".into()], &base_url()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].url.as_str(), "https://cdn.example.com/img.jpg");
    }

    #[test]
    fn should_normalize_multiple_images_preserving_order() {
        let result = normalize_images(
            vec![
                "https://cdn.example.com/img1.jpg".into(),
                "https://cdn.example.com/img2.jpg".into(),
                "https://cdn.example.com/img3.jpg".into(),
            ],
            &base_url(),
        )
        .unwrap();
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].url.as_str(), "https://cdn.example.com/img1.jpg");
        assert_eq!(result[1].url.as_str(), "https://cdn.example.com/img2.jpg");
        assert_eq!(result[2].url.as_str(), "https://cdn.example.com/img3.jpg");
    }

    #[rstest]
    #[case("/images/item.jpg", "https://example.com/images/item.jpg")]
    #[case("images/item.jpg", "https://example.com/products/images/item.jpg")]
    fn should_resolve_relative_urls_against_base_url(#[case] input: &str, #[case] expected: &str) {
        let result = normalize_images(vec![input.into()], &base_url()).unwrap();
        assert_eq!(result[0].url.as_str(), expected);
    }

    #[test]
    fn should_trim_whitespace_from_image_urls_when_normalizing() {
        let result = normalize_images(
            vec!["  https://cdn.example.com/img.jpg  ".into()],
            &base_url(),
        )
        .unwrap();
        assert_eq!(result[0].url.as_str(), "https://cdn.example.com/img.jpg");
    }

    #[test]
    fn should_set_prohibited_content_to_unknown_for_all_images() {
        let result = normalize_images(
            vec![
                "https://cdn.example.com/img1.jpg".into(),
                "https://cdn.example.com/img2.jpg".into(),
            ],
            &base_url(),
        )
        .unwrap();
        for image in &result {
            assert_eq!(image.prohibited_content, ProhibitedContent::Unknown);
        }
    }

    #[test]
    fn should_handle_mixed_absolute_and_relative_urls() {
        let result = normalize_images(
            vec![
                "https://cdn.example.com/img1.jpg".into(),
                "/images/img2.jpg".into(),
            ],
            &base_url(),
        )
        .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].url.as_str(), "https://cdn.example.com/img1.jpg");
        assert_eq!(
            result[1].url.as_str(),
            "https://example.com/images/img2.jpg"
        );
    }

    #[test]
    fn should_use_different_scheme_from_base_url_when_absolute_url_provided() {
        let http_base = Url::parse("http://example.com/products/123").unwrap();
        let result =
            normalize_images(vec!["https://cdn.example.com/img.jpg".into()], &http_base).unwrap();
        // Absolute URL keeps its own scheme, not the base's.
        assert!(result[0].url.as_str().starts_with("https://"));
    }

    // -----------------------------------------------------------------------
    // Error cases
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_error_when_image_url_is_invalid() {
        // "//" is invalid as an absolute URL and fails as a relative join too
        // because it results in an empty authority.
        let err = normalize_images(vec!["//".into()], &base_url()).unwrap_err();
        assert!(matches!(err, NormalizationError::InvalidImageUrl { .. }));
    }

    #[test]
    fn should_include_raw_url_string_in_error_when_image_url_is_invalid() {
        let err = normalize_images(vec!["//".into()], &base_url()).unwrap_err();
        if let NormalizationError::InvalidImageUrl { raw, .. } = err {
            assert_eq!(raw, "//");
        } else {
            panic!("expected InvalidImageUrl");
        }
    }

    #[test]
    fn should_fail_fast_on_first_invalid_url_when_multiple_images_provided() {
        // The invalid URL is first; the valid one should never be processed.
        let err = normalize_images(
            vec!["//".into(), "https://cdn.example.com/img.jpg".into()],
            &base_url(),
        )
        .unwrap_err();
        assert!(matches!(err, NormalizationError::InvalidImageUrl { .. }));
    }
}
