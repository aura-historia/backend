//! Image URLs of a matched element, as an `image_url` rule captures them: the element's
//! ordered image URL candidates, joined into one group.

use super::IMAGE_CANDIDATE_SEPARATOR;
use std::collections::HashSet;

/// The element's image URL candidates joined into one group, or `None` when it has none.
pub(super) fn image_url_candidate_group(element: &scraper::ElementRef<'_>) -> Option<String> {
    let candidates = image_url_candidates_for_element(element);
    if candidates.is_empty() {
        return None;
    }

    Some(candidates.join(IMAGE_CANDIDATE_SEPARATOR.encode_utf8(&mut [0; 4])))
}

fn image_url_candidates_for_element(element: &scraper::ElementRef<'_>) -> Vec<String> {
    let mut candidates = OrderedImageCandidates::default();

    for attr in [
        "data-large_image",
        "data-full",
        "data-original",
        "data-zoom-image",
    ] {
        push_attr_candidate(
            &mut candidates,
            element,
            attr,
            ImageCandidateSource::ImageSpecific,
        );
    }
    for attr in ["data-src", "data-lazy-src", "content"] {
        push_attr_candidate(
            &mut candidates,
            element,
            attr,
            ImageCandidateSource::ImageSpecific,
        );
    }
    push_attr_candidate(
        &mut candidates,
        element,
        "href",
        image_link_source_for_element(element, element.value().attr("href")),
    );

    if let Some((href, source)) = nearest_anchor_href(element) {
        candidates.push(href, source);
    }

    for srcset in picture_source_srcsets(element) {
        if let Some((url, _)) = largest_srcset_candidate(Some(srcset)) {
            candidates.push(url, ImageCandidateSource::ImageSpecific);
        }
    }
    if let Some((url, _)) = largest_srcset_candidate(element.value().attr("srcset")) {
        candidates.push(url, ImageCandidateSource::ImageSpecific);
    }

    push_attr_candidate(
        &mut candidates,
        element,
        "src",
        ImageCandidateSource::ImageSpecific,
    );

    candidates.into_vec()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageCandidateSource {
    ImageSpecific,
    ImageLikeLink,
    GenericLink,
}

#[derive(Default)]
struct OrderedImageCandidates {
    seen: HashSet<String>,
    values: Vec<String>,
}

impl OrderedImageCandidates {
    fn push(&mut self, value: String, source: ImageCandidateSource) {
        let trimmed = value.trim();
        if trimmed.is_empty() || !is_probably_image_url(trimmed, source) {
            return;
        }

        let normalized = trimmed.to_owned();
        if self.seen.insert(normalized.clone()) {
            self.values.push(normalized);
        }
    }

    fn into_vec(self) -> Vec<String> {
        self.values
    }
}

fn push_attr_candidate(
    candidates: &mut OrderedImageCandidates,
    element: &scraper::ElementRef<'_>,
    attr: &str,
    source: ImageCandidateSource,
) {
    if let Some(value) = element
        .value()
        .attr(attr)
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        candidates.push(value.to_owned(), source);
    }
}

fn nearest_anchor_href(
    element: &scraper::ElementRef<'_>,
) -> Option<(String, ImageCandidateSource)> {
    for ancestor in element.ancestors() {
        let Some(ancestor) = scraper::ElementRef::wrap(ancestor) else {
            continue;
        };
        if ancestor.value().name() == "a" {
            let href = ancestor.value().attr("href")?;
            return Some((
                href.to_owned(),
                image_link_source_for_element(&ancestor, Some(href)),
            ));
        }
    }
    None
}

fn image_link_source_for_element(
    element: &scraper::ElementRef<'_>,
    href: Option<&str>,
) -> ImageCandidateSource {
    if href.is_some_and(path_has_image_context) || element_attrs_have_image_context(element) {
        ImageCandidateSource::ImageLikeLink
    } else {
        ImageCandidateSource::GenericLink
    }
}

fn element_attrs_have_image_context(element: &scraper::ElementRef<'_>) -> bool {
    const IMAGE_CONTEXT_TERMS: &[&str] = &[
        "image",
        "images",
        "img",
        "photo",
        "photos",
        "gallery",
        "thumbnail",
        "thumb",
        "zoom",
        "lightbox",
        "fancybox",
        "prettyphoto",
        "full",
    ];

    let attr_text = [
        element.value().attr("class"),
        element.value().attr("id"),
        element.value().attr("rel"),
        element.value().attr("data-gallery"),
        element.value().attr("data-lightbox"),
        element.value().attr("data-fancybox"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();

    IMAGE_CONTEXT_TERMS
        .iter()
        .any(|term| attr_text.contains(term))
}

fn path_has_image_context(href: &str) -> bool {
    let lower = href.to_ascii_lowercase();
    [
        "/photo", "/photos", "/image", "/images", "/media", "/uploads", "/cdn/", "cdn.",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

fn picture_source_srcsets<'a>(element: &scraper::ElementRef<'a>) -> Vec<&'a str> {
    let Some(parent_node) = element.parent() else {
        return Vec::new();
    };
    let Some(parent) = scraper::ElementRef::wrap(parent_node) else {
        return Vec::new();
    };
    if parent.value().name() != "picture" {
        return Vec::new();
    }

    parent
        .children()
        .filter_map(scraper::ElementRef::wrap)
        .filter(|child| child.value().name() == "source")
        .filter_map(|child| child.value().attr("srcset"))
        .collect()
}

fn largest_srcset_candidate(srcset: Option<&str>) -> Option<(String, usize)> {
    srcset?
        .split(',')
        .filter_map(|candidate| {
            let mut parts = candidate.split_whitespace();
            let url = parts.next()?.trim();
            let width = parts
                .find_map(|part| part.strip_suffix('w'))
                .and_then(|raw| raw.parse::<usize>().ok())
                .unwrap_or_default();
            if url.is_empty() {
                None
            } else {
                Some((url.to_owned(), width))
            }
        })
        .max_by_key(|(_, width)| *width)
}

fn is_probably_image_url(url: &str, source: ImageCandidateSource) -> bool {
    match source {
        ImageCandidateSource::ImageSpecific => is_url_like(url) || has_known_image_extension(url),
        ImageCandidateSource::ImageLikeLink => is_url_like(url) || has_known_image_extension(url),
        ImageCandidateSource::GenericLink => has_known_image_extension(url),
    }
}

fn is_url_like(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with('/')
}

/// Whether `url` mentions a known image file extension anywhere, case-insensitively.
pub(crate) fn has_known_image_extension(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains(".jpg")
        || lower.contains(".jpeg")
        || lower.contains(".png")
        || lower.contains(".webp")
        || lower.contains(".gif")
        || lower.contains(".avif")
}
