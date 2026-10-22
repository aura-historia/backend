use crate::network::policy::{
    CrawlerClientError, PublicTargetError, classify_reqwest_error, error_with_sources,
    public_http_client, redirect_target, resolve_public_http_target,
};
use crate::scraper::css_selector::rule::split_image_candidate_group;
use regex::regex;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use url::Url;

const MIN_LONGEST_SIDE: usize = 400;
const MIN_SHORTEST_SIDE: usize = 250;
const IMAGE_PROBE_BYTES: &str = "bytes=0-32767";
const IMAGE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const IMAGE_PROBE_MAX_REDIRECTS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageValidation {
    Valid,
    Invalid,
    Unknown,
}

#[async_trait::async_trait]
pub(crate) trait ImageValidator: Send + Sync {
    async fn validate(&self, url: &Url) -> ImageValidation;
}

pub(crate) struct ReqwestImageValidator {
    cache: Mutex<HashMap<String, ImageValidation>>,
}

impl ReqwestImageValidator {
    pub(crate) fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl Default for ReqwestImageValidator {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ImageValidator for ReqwestImageValidator {
    async fn validate(&self, url: &Url) -> ImageValidation {
        // Quality may be cached, but target safety is checked for every acceptance.
        // DNS answers can change after a prior successful image probe.
        if let Err(error) = resolve_public_http_target(url, IMAGE_PROBE_TIMEOUT).await {
            return image_validation_for_target_error(error);
        }

        let key = url.as_str().to_owned();
        if let Some(cached) = self.cache.lock().expect("cache lock").get(&key).copied() {
            return cached;
        }

        let validation = match ImageUrlHint::of(url) {
            ImageUrlHint::Dimensions(validation) => validation,
            ImageUrlHint::ThumbnailName | ImageUrlHint::NoHint => probe_image_dimensions(url).await,
        };

        self.cache
            .lock()
            .expect("cache lock")
            .insert(key, validation);
        validation
    }
}

/// Resolves each ordered image candidate group against `base_url` and keeps the
/// first candidate of each group that passes validation, without duplicates. An
/// empty result means the page has no usable image; it is not a failure.
pub(crate) async fn filter_valid_image_urls(
    raw_images: Vec<String>,
    base_url: &Url,
    validator: &dyn ImageValidator,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut valid = Vec::new();

    for raw in raw_images {
        for raw_candidate in split_image_candidate_group(&raw) {
            let Some(resolved) = resolve_image_candidate(raw_candidate, base_url) else {
                tracing::debug!("Discarding invalid image URL candidate");
                continue;
            };

            // Security validation always runs first. The URL hint may then
            // discard a target without replacing that result.
            let validation =
                ImageUrlHint::of(&resolved).settle(validator.validate(&resolved).await);
            match validation {
                ImageValidation::Invalid => {}
                ImageValidation::Valid | ImageValidation::Unknown => {
                    let normalized = resolved.to_string();
                    if seen.insert(normalized.clone()) {
                        valid.push(normalized);
                    }
                    break;
                }
            }
        }
    }

    valid
}

/// Resolves one raw image candidate against the page URL. `None` means the
/// candidate is not a usable URL.
pub(crate) fn resolve_image_candidate(raw_candidate: &str, base_url: &Url) -> Option<Url> {
    let trimmed = raw_candidate.trim();
    if trimmed.is_empty() {
        return None;
    }
    Url::parse(trimmed).or_else(|_| base_url.join(trimmed)).ok()
}

/// What an image URL's text says about the image before any probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageUrlHint {
    /// Query `w`/`h` or `width`/`height`, or path dimensions such as
    /// `-300x200.`, give the image's size.
    Dimensions(ImageValidation),
    /// The URL gives no size and names a thumbnail. Shops also name full-size
    /// uploads this way, so only a probe that reads the size decides.
    ThumbnailName,
    /// The URL gives no size and no other hint.
    NoHint,
}

impl ImageUrlHint {
    fn of(url: &Url) -> Self {
        if let Some((width, height)) =
            dimensions_from_query(url).or_else(|| dimensions_from_path(url.path()))
        {
            return Self::Dimensions(validation_for_dimensions(width, height));
        }
        if names_thumbnail(url) {
            return Self::ThumbnailName;
        }
        Self::NoHint
    }

    /// Combines the hint with the validator's result. The hint can only
    /// reject an image, never accept one the validator rejected.
    fn settle(self, validation: ImageValidation) -> ImageValidation {
        match (self, validation) {
            (Self::Dimensions(ImageValidation::Invalid), _) => ImageValidation::Invalid,
            // An image of unknown size is normally kept, but a thumbnail-named
            // one may really be a thumbnail. Rejecting it lets a later
            // candidate of the same group take its place.
            (Self::ThumbnailName, ImageValidation::Unknown) => ImageValidation::Invalid,
            (_, validation) => validation,
        }
    }
}

fn names_thumbnail(url: &Url) -> bool {
    url.as_str().to_ascii_lowercase().contains("thumb")
}

fn validation_for_dimensions(width: usize, height: usize) -> ImageValidation {
    let longest = width.max(height);
    let shortest = width.min(height);
    if longest >= MIN_LONGEST_SIDE && shortest >= MIN_SHORTEST_SIDE {
        ImageValidation::Valid
    } else {
        ImageValidation::Invalid
    }
}

fn dimensions_from_query(url: &Url) -> Option<(usize, usize)> {
    let mut width = None;
    let mut height = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "w" | "width" => width = value.parse::<usize>().ok(),
            "h" | "height" => height = value.parse::<usize>().ok(),
            _ => {}
        }
    }
    Some((width?, height?))
}

fn dimensions_from_path(path: &str) -> Option<(usize, usize)> {
    let captures = regex!(r"(?i)[-_/](\d{2,5})x(\d{2,5})(?:[._/?-]|$)").captures(path)?;
    let width = captures.get(1)?.as_str().parse::<usize>().ok()?;
    let height = captures.get(2)?.as_str().parse::<usize>().ok()?;
    Some((width, height))
}

async fn probe_image_dimensions(url: &Url) -> ImageValidation {
    let mut current_url = url.clone();
    let mut redirect_count = 0;
    let response = loop {
        let client = match public_http_client(&current_url, IMAGE_PROBE_TIMEOUT).await {
            Ok(client) => client,
            Err(CrawlerClientError::Target(error)) => {
                tracing::debug!("Image dimension probe rejected target");
                return image_validation_for_target_error(error);
            }
            Err(error) => {
                tracing::debug!(
                    error = %error_with_sources(&error),
                    "Image dimension probe client unavailable"
                );
                return ImageValidation::Unknown;
            }
        };
        let response = match client
            .get(current_url.clone())
            .header(reqwest::header::RANGE, IMAGE_PROBE_BYTES)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                tracing::debug!(
                    kind = ?classify_reqwest_error(&err),
                    error = %error_with_sources(&err),
                    "Image dimension probe failed"
                );
                return ImageValidation::Unknown;
            }
        };
        if !response.status().is_redirection() {
            break response;
        }
        let next_url = match redirect_target(&current_url, &response) {
            Ok(url) => url,
            Err(error) => {
                tracing::debug!("Image dimension probe rejected redirect");
                return image_validation_for_target_error(error);
            }
        };
        if redirect_count == IMAGE_PROBE_MAX_REDIRECTS || current_url == next_url {
            return ImageValidation::Unknown;
        }
        redirect_count += 1;
        current_url = next_url;
    };

    let response = match response.error_for_status() {
        Ok(response) => response,
        Err(err) => {
            tracing::debug!(
                kind = ?classify_reqwest_error(&err),
                "Image dimension probe returned non-success status"
            );
            return ImageValidation::Unknown;
        }
    };

    let bytes = match read_probe_bytes(response).await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::debug!(
                kind = ?classify_reqwest_error(&err),
                "Image dimension probe body read failed"
            );
            return ImageValidation::Unknown;
        }
    };

    match imagesize::blob_size(&bytes) {
        Ok(size) => validation_for_dimensions(size.width, size.height),
        Err(_) => {
            tracing::debug!("Image dimension parser could not read probed bytes");
            ImageValidation::Unknown
        }
    }
}

fn image_validation_for_target_error(error: PublicTargetError) -> ImageValidation {
    match error {
        // A URL is accepted only after current DNS resolution has established a
        // safe public target. Quality may remain unknown after that point, but
        // unresolved targets must never reach canonical ProductListing state.
        PublicTargetError::InvalidUrl
        | PublicTargetError::IpLiteral
        | PublicTargetError::InvalidPort
        | PublicTargetError::ResolutionTimeout
        | PublicTargetError::Resolution
        | PublicTargetError::UnsafeResolution
        | PublicTargetError::InvalidRedirect => ImageValidation::Invalid,
    }
}

async fn read_probe_bytes(mut response: reqwest::Response) -> Result<Vec<u8>, reqwest::Error> {
    let mut bytes = Vec::with_capacity(32 * 1024);
    while bytes.len() < 32 * 1024 {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        let remaining = 32 * 1024 - bytes.len();
        bytes.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::css_selector::rule::IMAGE_CANDIDATE_SEPARATOR;

    struct StaticValidator {
        validation: ImageValidation,
    }

    #[async_trait::async_trait]
    impl ImageValidator for StaticValidator {
        async fn validate(&self, _: &Url) -> ImageValidation {
            self.validation
        }
    }

    #[test]
    fn rejects_small_dimensions_from_path() {
        let url = Url::parse("https://example.com/photo-100x100.jpg").unwrap();
        assert_eq!(
            ImageUrlHint::of(&url),
            ImageUrlHint::Dimensions(ImageValidation::Invalid)
        );
    }

    #[test]
    fn accepts_large_dimensions_from_query() {
        let url = Url::parse("https://example.com/photo.jpg?width=640&height=640").unwrap();
        assert_eq!(
            ImageUrlHint::of(&url),
            ImageUrlHint::Dimensions(ImageValidation::Valid)
        );
    }

    #[tokio::test]
    async fn rejects_private_image_url_even_when_the_path_has_large_dimensions() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec!["http://127.0.0.1/image-800x600.jpg".to_string()],
            &base,
            &ReqwestImageValidator::new(),
        )
        .await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn rejects_metadata_image_url_even_when_query_has_large_dimensions() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec!["http://169.254.169.254/photo.jpg?width=800&height=600".to_string()],
            &base,
            &ReqwestImageValidator::new(),
        )
        .await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn filters_invalid_images_but_keeps_valid_ones() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![
                "/image-100x100.jpg".to_string(),
                "/image-800x600.jpg".to_string(),
            ],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await;

        assert_eq!(result, vec!["https://example.com/image-800x600.jpg"]);
    }

    #[tokio::test]
    async fn accepts_first_valid_candidate_from_each_ordered_image_group() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![
                format!("/image-100x100.jpg{IMAGE_CANDIDATE_SEPARATOR}/image-800x600.jpg"),
                format!("/other-80x80.jpg{IMAGE_CANDIDATE_SEPARATOR}/other-640x480.jpg"),
            ],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await;

        assert_eq!(
            result,
            vec![
                "https://example.com/image-800x600.jpg",
                "https://example.com/other-640x480.jpg"
            ]
        );
    }

    #[tokio::test]
    async fn returns_no_image_when_all_images_are_invalid() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![format!(
                "/image-100x100.jpg{IMAGE_CANDIDATE_SEPARATOR}/image-120x120.jpg"
            )],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn treats_malformed_image_url_candidates_as_invalid() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec!["//".to_string()],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await;

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn keeps_valid_fallback_candidate_after_malformed_candidate_in_same_group() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let result = filter_valid_image_urls(
            vec![format!("//{IMAGE_CANDIDATE_SEPARATOR}/image-800x600.jpg")],
            &base,
            &StaticValidator {
                validation: ImageValidation::Unknown,
            },
        )
        .await;

        assert_eq!(result, vec!["https://example.com/image-800x600.jpg"]);
    }

    /// A shop's full-size 768x768 upload whose file name starts with `thumbnail_`.
    const THUMBNAIL_NAMED_UPLOAD: &str =
        "https://example.com/wp-content/uploads/2018/02/thumbnail_IMG_5849_org_l.jpg?fit=768%2C768";

    #[tokio::test]
    async fn should_keep_thumbnail_named_upload_when_its_probe_reads_a_full_size() {
        let base = Url::parse("https://example.com/product/wall-light").unwrap();
        let result = filter_valid_image_urls(
            vec![THUMBNAIL_NAMED_UPLOAD.to_string()],
            &base,
            &StaticValidator {
                validation: ImageValidation::Valid,
            },
        )
        .await;

        assert_eq!(
            ImageUrlHint::of(&Url::parse(THUMBNAIL_NAMED_UPLOAD).unwrap()),
            ImageUrlHint::ThumbnailName,
            "the URL text leaves the decision to the probe"
        );
        assert_eq!(result, vec![THUMBNAIL_NAMED_UPLOAD]);
    }

    #[tokio::test]
    async fn should_fall_back_from_thumbnail_named_candidate_when_its_probe_cannot_read_the_size() {
        let base = Url::parse("https://example.com/product/wall-light").unwrap();
        let unknown_size = StaticValidator {
            validation: ImageValidation::Unknown,
        };

        let with_fallback = filter_valid_image_urls(
            vec![format!(
                "{THUMBNAIL_NAMED_UPLOAD}{IMAGE_CANDIDATE_SEPARATOR}/images/wall-light.jpg"
            )],
            &base,
            &unknown_size,
        )
        .await;
        let alone = filter_valid_image_urls(
            vec![THUMBNAIL_NAMED_UPLOAD.to_string()],
            &base,
            &unknown_size,
        )
        .await;

        assert_eq!(
            with_fallback,
            vec!["https://example.com/images/wall-light.jpg"]
        );
        assert!(alone.is_empty());
    }

    #[tokio::test]
    async fn should_still_reject_resize_query_with_a_small_height() {
        let resized =
            "https://example.com/Object/636610351541085812.jpg?width=1000&height=150&mode=max";
        let base = Url::parse("https://example.com/objects/1").unwrap();
        let result = filter_valid_image_urls(
            vec![resized.to_string()],
            &base,
            &StaticValidator {
                validation: ImageValidation::Valid,
            },
        )
        .await;

        assert_eq!(
            ImageUrlHint::of(&Url::parse(resized).unwrap()),
            ImageUrlHint::Dimensions(ImageValidation::Invalid)
        );
        assert!(result.is_empty());
    }
}
