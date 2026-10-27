//! Images of a matched element: its ordered image URL candidates, joined into one group as
//! an `image_url` rule captures them, whether it carries image markup at all, and whether
//! it is an empty gallery slot.

use super::IMAGE_CANDIDATE_SEPARATOR;
use super::visible_text::has_visible_text;
use std::borrow::Cow;
use std::collections::HashSet;
use url::Url;

/// The element's image URL candidates joined into one group, or `None` when it has none.
pub(super) fn image_url_candidate_group(element: &scraper::ElementRef<'_>) -> Option<String> {
    let candidates = image_url_candidates_for_element(element);
    if candidates.is_empty() {
        return None;
    }

    Some(candidates.join(IMAGE_CANDIDATE_SEPARATOR.encode_utf8(&mut [0; 4])))
}

/// Whether `element` is a gallery slot without an image, as galleries render for photos a
/// product does not have (`<a rel="lightbox" href="#"></a>`, also with a hidden
/// `<img src="//:0">` inside): its own attributes name an image context, and it holds no
/// image URL candidate, no image markup, no attribute that names an image file and no
/// visible text, where a URL without a host names no image. An element that holds an image
/// the rule cannot read, such as a gallery wrapper around its `<img>` or a slot whose
/// `data-largeimg` names the photo, is no placeholder, and neither is a control with a label
/// or a layout element such as `<div class="w-full"></div>`.
pub(super) fn is_gallery_placeholder(element: &scraper::ElementRef<'_>) -> bool {
    element_attrs_name_image_context(element)
        && image_url_candidate_group(element).is_none()
        && !element_has_image_like_evidence(element)
        && !element_names_image_file(element)
        && !has_visible_text(element)
}

/// Whether an attribute of `element` or of a descendant names an image file, such as a
/// lazy loader's `data-bg` that neither the image URL candidates nor the image markup
/// check read.
fn element_names_image_file(element: &scraper::ElementRef<'_>) -> bool {
    element
        .descendants()
        .filter_map(scraper::ElementRef::wrap)
        .any(|node| {
            node.value().attrs().any(|(_, value)| {
                has_known_image_extension(value) && !holds_only_hostless_urls(value)
            })
        })
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

/// Terms that name an image context in an element's class, id, `rel` or gallery data
/// attribute.
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
];

/// Whether a link's own attributes mark it as an image link: they name an image context or
/// a full-size image (`full`).
fn element_attrs_have_image_context(element: &scraper::ElementRef<'_>) -> bool {
    let attr_text = image_context_attr_text(element);
    attr_text.contains("full") || names_image_context(&attr_text)
}

/// Whether `element`'s own attributes name an image context. `full` alone does not: it
/// also names layout utilities such as `w-full` or `rounded-full`.
fn element_attrs_name_image_context(element: &scraper::ElementRef<'_>) -> bool {
    names_image_context(&image_context_attr_text(element))
}

/// The class, id, `rel` and gallery data attributes of `element`, lowercased.
fn image_context_attr_text(element: &scraper::ElementRef<'_>) -> String {
    [
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
    .to_ascii_lowercase()
}

fn names_image_context(attr_text: &str) -> bool {
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

/// Whether `url`, read from `source`, can name an image. A URL without a host never does
/// (see [`is_hostless_url`]).
fn is_probably_image_url(url: &str, source: ImageCandidateSource) -> bool {
    if is_hostless_url(url) {
        return false;
    }
    match source {
        ImageCandidateSource::ImageSpecific | ImageCandidateSource::ImageLikeLink => {
            is_url_like(url) || has_known_image_extension(url)
        }
        ImageCandidateSource::GenericLink => has_known_image_extension(url),
    }
}

fn is_url_like(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with('/')
}

/// Whether `url` names an authority without a host, such as the `//:0` some galleries give
/// the hidden image of an empty slot. Such a URL can never be fetched: capture resolves
/// image URLs with the same parser and discards it. A relative URL names no authority and
/// takes the page's host.
fn is_hostless_url(url: &str) -> bool {
    let absolute = match url.strip_prefix("//") {
        Some(scheme_relative) => Cow::Owned(format!("https://{scheme_relative}")),
        None => Cow::Borrowed(url),
    };
    matches!(Url::parse(&absolute), Err(url::ParseError::EmptyHost))
}

/// Whether the attribute `value` holds at least one URL, one per candidate of a `srcset`,
/// and none of them has a host (see [`is_hostless_url`]).
fn holds_only_hostless_urls(value: &str) -> bool {
    let mut urls = value
        .split(',')
        .filter_map(|candidate| candidate.split_whitespace().next())
        .peekable();
    urls.peek().is_some() && urls.all(is_hostless_url)
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

/// Elements that are image markup.
const IMAGE_ELEMENTS: [&str; 3] = ["img", "picture", "source"];

/// Attributes that hold an image source, as image elements and lazy loaders write them.
const IMAGE_SOURCE_ATTRIBUTES: [&str; 10] = [
    "src",
    "srcset",
    "data-src",
    "data-srcset",
    "data-lazy",
    "data-lazy-src",
    "data-large_image",
    "data-full",
    "data-original",
    "data-zoom-image",
];

/// Whether `element` or a descendant carries image markup: an `img`, `picture` or `source`
/// element, an image source attribute or a CSS background image. A host-less image (see
/// [`is_hostless_image`]) is none, because it can never be fetched. A hidden image with a
/// real URL still is: lazy-loading galleries hide their images until a script runs.
pub(crate) fn element_has_image_like_evidence(element: &scraper::ElementRef<'_>) -> bool {
    let markup = element
        .descendants()
        .filter_map(scraper::ElementRef::wrap)
        .filter(is_hostless_image)
        // `img` and `source` are void elements: their markup holds nothing else.
        .fold(element.html(), |markup, image| {
            markup.replace(&image.html(), "")
        });
    markup_has_image_evidence(&markup)
}

/// Whether `element` is an `img` or `source` element whose every image source attribute
/// holds only URLs without a host (see [`holds_only_hostless_urls`]) and whose style names
/// no background image, such as the hidden `<img src="//:0">` of an empty gallery slot. An
/// image element without an image source is not: a script may still fill it in.
fn is_hostless_image(element: &scraper::ElementRef<'_>) -> bool {
    let element = element.value();
    let mut sources = IMAGE_SOURCE_ATTRIBUTES
        .iter()
        .filter_map(|attribute| element.attr(attribute))
        .peekable();
    matches!(element.name(), "img" | "source")
        && !element
            .attr("style")
            .is_some_and(|style| style.to_ascii_lowercase().contains("background-image"))
        && sources.peek().is_some()
        && sources.all(holds_only_hostless_urls)
}

/// Whether serialized `markup` names an image element, an image source attribute or a CSS
/// background image anywhere, case-insensitively.
fn markup_has_image_evidence(markup: &str) -> bool {
    let markup = markup.to_ascii_lowercase();
    IMAGE_ELEMENTS
        .iter()
        .any(|name| markup.contains(&format!("<{name}")))
        || IMAGE_SOURCE_ATTRIBUTES
            .iter()
            .any(|attribute| markup.contains(&format!(" {attribute}=")))
        || markup.contains(" background-image")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;
    use scraper::Html;

    const ALL_SOURCES: [ImageCandidateSource; 3] = [
        ImageCandidateSource::ImageSpecific,
        ImageCandidateSource::ImageLikeLink,
        ImageCandidateSource::GenericLink,
    ];

    /// A URL whose authority names no host, which capture cannot resolve either, is no
    /// image URL candidate wherever it is read from, even with an image file name.
    #[rstest]
    #[case::scheme_relative("//:0")]
    #[case::scheme_relative_image_file("//:0/blank.gif")]
    #[case::absolute("https://:0/photo.jpg")]
    #[case::uppercase_scheme("HTTP://:8080")]
    #[case::userinfo_without_a_host("//user@:0")]
    fn should_reject_a_url_without_a_host_as_an_image_url_candidate(#[case] url: &str) {
        for source in ALL_SOURCES {
            assert!(!is_probably_image_url(url, source), "{source:?} took {url}");
        }
    }

    #[rstest]
    #[case::scheme_relative("//cdn.example.com/photo.jpg")]
    #[case::absolute_with_a_port("https://cdn.example.com:8443/photo.jpg")]
    #[case::ipv6_host("//[::1]/photo.jpg")]
    #[case::root_relative("/uploads/photo.jpg")]
    #[case::relative("uploads/photo.jpg")]
    fn should_keep_a_url_with_a_host_or_a_relative_url_as_an_image_url_candidate(
        #[case] url: &str,
    ) {
        assert!(is_probably_image_url(
            url,
            ImageCandidateSource::ImageSpecific
        ));
    }

    /// Markup of harrisonantiquefurniture.co.uk: an empty slot of its lightbox gallery.
    const HARRISON_EMPTY_SLOT: &str = r##"<a href="#" id="_EKM_PRODUCTIMAGE_LINK_9_2" rel="lightbox[1] mousezoom[1]" data-thumbnailimg="" data-largeimg="" data-video="" target="_blank"/>
<img itemprop="image" src="//:0" alt="variant attributes" width="200" height="200" id="_EKM_PRODUCTIMAGE_9_2" border="0" style="display:none;" /></a>"##;

    fn has_image_markup(markup: &str) -> bool {
        element_has_image_like_evidence(&Html::parse_fragment(markup).root_element())
    }

    #[rstest]
    #[case::hidden_hostless_image(HARRISON_EMPTY_SLOT)]
    #[case::hostless_srcset(r#"<source srcset="//:0 1x, //:0 2x">"#)]
    #[case::several_hostless_images(r#"<img src="//:0"><img src="//:0">"#)]
    #[case::padded_hostless_src(r#"<img src=" //:0 ">"#)]
    fn should_find_no_image_markup_when_every_image_source_has_no_host(#[case] markup: &str) {
        assert!(!has_image_markup(markup));
    }

    #[rstest]
    #[case::hidden_image_with_a_real_url(
        r##"<a href="#" rel="lightbox"><img src="/uploads/photo.jpg" style="display:none"></a>"##
    )]
    #[case::lazy_image_behind_a_hostless_src(r#"<img src="//:0" data-src="/uploads/photo">"#)]
    #[case::srcset_with_a_real_candidate(r#"<source srcset="//:0 1x, /uploads/photo.jpg 2x">"#)]
    #[case::hostless_image_with_a_background(
        r#"<img src="//:0" style="background-image: url(/uploads/photo.jpg)">"#
    )]
    #[case::image_without_a_source(r#"<img class="lazy">"#)]
    #[case::real_image_next_to_a_hostless_one(r#"<img src="//:0"><img src="/uploads/photo">"#)]
    #[case::real_image_inside_an_element_with_a_hostless_source(
        r#"<div data-src="//:0"><img src="/uploads/photo"></div>"#
    )]
    fn should_find_image_markup_when_an_image_may_have_a_source(#[case] markup: &str) {
        assert!(has_image_markup(markup));
    }

    /// A host-less URL names no image file, even with an image file name.
    #[test]
    fn should_take_a_slot_whose_hidden_image_file_has_no_host_for_a_placeholder() {
        let html =
            Html::parse_fragment(r##"<a href="#" rel="lightbox"><img src="//:0/blank.gif"></a>"##);
        let selector = scraper::Selector::parse("a").unwrap();
        let slot = html.select(&selector).next().unwrap();

        assert!(is_gallery_placeholder(&slot));
    }
}
