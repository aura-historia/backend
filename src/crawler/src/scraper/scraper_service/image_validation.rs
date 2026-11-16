use crate::network::policy::{
    CrawlerClientError, PublicTargetError, RequestTimeouts, classify_reqwest_error,
    error_with_sources, is_same_or_www_host, public_http_client, redirect_target,
    resolve_public_http_target, validate_public_http_url,
};
use crate::scraper::css_selector::rule::split_image_candidate_group;
use crate::scraper::scraper_service::domain::product::{
    count_image_probe, count_image_probe_capped,
};
use crate::scraper::scraper_service::html_fetcher::{HtmlFetcher, ImageProbeRobots};
use regex::regex;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use url::Url;

/// The smallest image capture keeps, about 200×150 in either orientation: small archive photos
/// such as a 228×200 product shot pass, while icons, badges, logo strips and 150×150 or smaller
/// thumbnails stay below it. Larger shop resizes, such as a 300×300 catalogue thumbnail, pass, so
/// the first candidate of a group that clears the floor is kept even when a later one is larger.
const MIN_LONGEST_SIDE: usize = 200;
const MIN_SHORTEST_SIDE: usize = 150;
const IMAGE_PROBE_BYTES: &str = "bytes=0-32767";
const IMAGE_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
const IMAGE_PROBE_MAX_REDIRECTS: usize = 3;
/// Above this `Crawl-delay`, a page probes at most [`MAX_PACED_PROBES_ABOVE_CAP_DELAY`] images
/// on the shop's own host, so a slow shop's page is not held for one spacing per image.
const PROBE_CAP_CRAWL_DELAY: std::time::Duration = std::time::Duration::from_secs(10);
/// The own-host images a page probes on a shop whose `Crawl-delay` is above
/// [`PROBE_CAP_CRAWL_DELAY`]: the first ones in the page's image order, its primary gallery.
const MAX_PACED_PROBES_ABOVE_CAP_DELAY: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageValidation {
    Valid,
    Invalid,
    Unknown,
}

#[async_trait::async_trait]
pub(crate) trait ImageValidator: Send + Sync {
    /// Decides `url`'s quality; a probe it sends goes through `admission`.
    async fn validate(&self, url: &Url, admission: &ProbeAdmission<'_>) -> ImageValidation;
}

/// Which image probes one page may send, and when (`docs/crawler/networking.md#robotstxt`). A
/// probe to the page's own host, bare or `www`, must be allowed by robots.txt. When the shop sets
/// a `Crawl-delay`, it waits for the shop's request spacing like a page request, and above
/// [`PROBE_CAP_CRAWL_DELAY`] only the page's first [`MAX_PACED_PROBES_ABOVE_CAP_DELAY`] images
/// are probed. A probe to any other host, such as an image CDN, is sent at once. One admission
/// serves every extraction of a fetched page, so each image is probed, paced and counted at most
/// once per page, and the cap counts each image once.
pub(crate) struct ProbeAdmission<'a> {
    fetcher: &'a dyn HtmlFetcher,
    page: &'a Url,
    /// The own-host images this page probes at a `Crawl-delay` above the cap delay.
    capped_slots: Mutex<HashSet<String>>,
    /// What each image probe of this page settled, an unknown size included, so another
    /// extraction of the page listing the same image sends no second probe.
    settled: Mutex<HashMap<String, ImageValidation>>,
}

/// Whether one probe request is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeVerdict {
    Send,
    /// robots.txt refuses the probe or cannot be read.
    Refused,
    /// The page already probes as many own-host images as a slow shop's `Crawl-delay` allows.
    Capped,
}

impl<'a> ProbeAdmission<'a> {
    pub(crate) fn new(fetcher: &'a dyn HtmlFetcher, page: &'a Url) -> Self {
        Self {
            fetcher,
            page,
            capped_slots: Mutex::new(HashSet::new()),
            settled: Mutex::new(HashMap::new()),
        }
    }

    /// Whether the probe of `image` may send its request to `target`, the image or a redirect
    /// hop of it; waits for the shop's spacing first when it may.
    async fn admit(&self, target: &Url, image: &Url) -> ProbeVerdict {
        if !is_same_or_www_host(self.page, target) {
            return ProbeVerdict::Send;
        }
        let crawl_delay = match self.fetcher.image_probe_robots(target).await {
            ImageProbeRobots::Refused => return ProbeVerdict::Refused,
            ImageProbeRobots::Allowed { crawl_delay } => crawl_delay,
        };
        let Some(crawl_delay) = crawl_delay.filter(|delay| !delay.is_zero()) else {
            return ProbeVerdict::Send;
        };
        if crawl_delay > PROBE_CAP_CRAWL_DELAY && !self.takes_capped_slot(image) {
            return ProbeVerdict::Capped;
        }
        self.fetcher.pace_image_probe(target, crawl_delay).await;
        ProbeVerdict::Send
    }

    /// Whether `image` is, or now becomes, one of the page's capped probes.
    fn takes_capped_slot(&self, image: &Url) -> bool {
        let mut slots = self.capped_slots.lock().expect("probe slots lock");
        if slots.contains(image.as_str()) {
            return true;
        }
        if slots.len() >= MAX_PACED_PROBES_ABOVE_CAP_DELAY {
            return false;
        }
        slots.insert(image.as_str().to_owned());
        true
    }
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
    async fn validate(&self, url: &Url, admission: &ProbeAdmission<'_>) -> ImageValidation {
        // Quality may be cached, but target safety is checked for every acceptance.
        // DNS answers can change after a prior successful image probe.
        if let Err(error) = resolve_public_http_target(url, IMAGE_PROBE_TIMEOUT).await {
            return image_validation_for_target_error(error);
        }

        self.cached_quality(url, || image_quality(url, admission))
            .await
    }
}

impl ReqwestImageValidator {
    /// Returns the cached quality of `url`, or runs `probe` and caches its
    /// result unless it is `Unknown`. An unknown size usually comes from a
    /// transient probe failure, and a thumbnail-named image of unknown size is
    /// discarded, so caching it would drop the image until the process restarts.
    async fn cached_quality<F, Fut>(&self, url: &Url, probe: F) -> ImageValidation
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = ImageValidation>,
    {
        if let Some(cached) = self.cache.lock().expect("cache lock").get(url.as_str()) {
            return *cached;
        }

        let validation = probe().await;
        if validation != ImageValidation::Unknown {
            self.cache
                .lock()
                .expect("cache lock")
                .insert(url.as_str().to_owned(), validation);
        }
        validation
    }
}

/// Decides an image's quality from the size its URL gives, or else from a probe.
async fn image_quality(url: &Url, admission: &ProbeAdmission<'_>) -> ImageValidation {
    match ImageUrlHint::of(url) {
        ImageUrlHint::Dimensions(validation) => validation,
        ImageUrlHint::ThumbnailName | ImageUrlHint::NoHint => {
            probe_image_dimensions(url, admission).await
        }
    }
}

/// Resolves each ordered image candidate group against `base_url` and keeps the
/// first candidate of each group that passes validation, without duplicates. An
/// empty result means the page has no usable image; it is not a failure.
pub(crate) async fn filter_valid_image_urls(
    raw_images: Vec<String>,
    base_url: &Url,
    validator: &dyn ImageValidator,
    admission: &ProbeAdmission<'_>,
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
                ImageUrlHint::of(&resolved).settle(validator.validate(&resolved, admission).await);
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

/// Keeps the images [`filter_valid_image_urls`] would keep if DNS resolution and
/// the probe accepted every image, for offline schema ranking and the schema
/// matrix, which have no network access: the first candidate of each ordered
/// image candidate group, without duplicates, that passes the checks a URL alone
/// decides. A URL that capture's target check rejects (scheme, credentials, IP
/// literal, port) or whose own dimensions show a small image is therefore
/// rejected as at capture, while a URL that gives no size counts, `thumb`-named
/// or not, because only the capture-time probe can read its size.
pub(crate) fn filter_image_urls_offline(raw_images: &[String], base_url: &Url) -> Vec<String> {
    let mut seen = HashSet::new();
    raw_images
        .iter()
        .filter_map(|raw_image| {
            split_image_candidate_group(raw_image)
                .into_iter()
                .filter_map(|raw_candidate| resolve_image_candidate(raw_candidate, base_url))
                .find(passes_url_checks)
                .map(String::from)
        })
        .filter(|image_url| seen.insert(image_url.clone()))
        .collect()
}

/// Whether `url` passes the image checks that need no network access: the
/// target check that capture runs before resolving the host, and the dimensions
/// the URL itself gives.
fn passes_url_checks(url: &Url) -> bool {
    validate_public_http_url(url).is_ok() && !ImageUrlHint::of(url).rejects_by_itself()
}

/// Resolves one raw image candidate against the page URL. `None` means the
/// candidate is not a usable URL.
fn resolve_image_candidate(raw_candidate: &str, base_url: &Url) -> Option<Url> {
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

    /// Whether the hint rejects the image even when the validator accepts it,
    /// so the decision needs no network access.
    fn rejects_by_itself(self) -> bool {
        self.settle(ImageValidation::Valid) == ImageValidation::Invalid
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

/// Probes `url` once per page: an image this page already probed, or left unprobed, answers as
/// it did then.
async fn probe_image_dimensions(url: &Url, admission: &ProbeAdmission<'_>) -> ImageValidation {
    let settled = admission
        .settled
        .lock()
        .expect("probe outcomes lock")
        .get(url.as_str())
        .copied();
    if let Some(settled) = settled {
        return settled;
    }
    let validation = send_image_probe(url, admission).await;
    admission
        .settled
        .lock()
        .expect("probe outcomes lock")
        .insert(url.as_str().to_owned(), validation);
    validation
}

/// A probe that robots.txt refuses, or cannot be read for, or that a slow shop's probe cap
/// leaves out, is not sent and leaves the size unknown, as a failed probe does.
async fn send_image_probe(url: &Url, admission: &ProbeAdmission<'_>) -> ImageValidation {
    let mut current_url = url.clone();
    let mut redirect_count = 0;
    let response = loop {
        match admission.admit(&current_url, url).await {
            ProbeVerdict::Send => {}
            ProbeVerdict::Refused => {
                tracing::debug!("Image dimension probe not admitted by robots.txt");
                return ImageValidation::Unknown;
            }
            ProbeVerdict::Capped => {
                count_image_probe_capped();
                tracing::debug!("Image dimension probe left out by the slow shop's probe cap");
                return ImageValidation::Unknown;
            }
        }
        if redirect_count == 0 {
            count_image_probe();
        }
        let client =
            match public_http_client(&current_url, RequestTimeouts::uniform(IMAGE_PROBE_TIMEOUT))
                .await
            {
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
        let response = match range_probe(&client, &current_url).send().await {
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

/// The request for the first bytes of an image. It asks for the identity encoding, overriding
/// the transfer encodings the crawler client names: a range applies to the encoded
/// representation, so the first bytes of a compressed image would be a truncated stream that
/// does not decode, not the image header the probe reads its dimensions from.
fn range_probe(client: &reqwest::Client, url: &Url) -> reqwest::RequestBuilder {
    client
        .get(url.clone())
        .header(reqwest::header::RANGE, IMAGE_PROBE_BYTES)
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
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
    use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml};
    use rstest::rstest;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A page fetcher with no robots.txt or pacing: it admits every probe.
    struct AdmitAll;

    #[async_trait::async_trait]
    impl HtmlFetcher for AdmitAll {
        async fn fetch(&self, _: &Url) -> Result<FetchedHtml, FetchError> {
            unreachable!("image validation fetches no page")
        }

        async fn image_probe_robots(&self, _: &Url) -> ImageProbeRobots {
            ImageProbeRobots::Allowed { crawl_delay: None }
        }
    }

    /// A page fetcher whose robots.txt answers every probe with `robots`, recording the probes
    /// it was asked about and the ones it paced.
    struct ShopRobots {
        robots: ImageProbeRobots,
        asked: Mutex<Vec<String>>,
        paced: Mutex<Vec<String>>,
    }

    impl ShopRobots {
        fn answering(robots: ImageProbeRobots) -> Self {
            Self {
                robots,
                asked: Mutex::new(Vec::new()),
                paced: Mutex::new(Vec::new()),
            }
        }

        fn with_crawl_delay(seconds: u64) -> Self {
            Self::answering(ImageProbeRobots::Allowed {
                crawl_delay: Some(std::time::Duration::from_secs(seconds)),
            })
        }

        fn paced(&self) -> Vec<String> {
            self.paced.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl HtmlFetcher for ShopRobots {
        async fn fetch(&self, _: &Url) -> Result<FetchedHtml, FetchError> {
            unreachable!("image validation fetches no page")
        }

        async fn image_probe_robots(&self, url: &Url) -> ImageProbeRobots {
            self.asked.lock().unwrap().push(url.to_string());
            self.robots
        }

        async fn pace_image_probe(&self, url: &Url, _: std::time::Duration) {
            self.paced.lock().unwrap().push(url.to_string());
        }
    }

    fn shop_page() -> Url {
        Url::parse("https://shop.example/product/1").unwrap()
    }

    fn own_image(index: usize) -> Url {
        Url::parse(&format!("https://shop.example/img/{index}.jpg")).unwrap()
    }

    async fn verdicts(admission: &ProbeAdmission<'_>, images: &[Url]) -> Vec<ProbeVerdict> {
        let mut verdicts = Vec::new();
        for image in images {
            verdicts.push(admission.admit(image, image).await);
        }
        verdicts
    }

    /// robots.txt is asked about probes to the page's own host, bare or `www`; a probe to an
    /// image CDN is sent without it.
    #[tokio::test]
    async fn should_ask_robots_txt_only_for_probes_to_the_shops_own_host() {
        let page = shop_page();
        let fetcher = ShopRobots::answering(ImageProbeRobots::Refused);
        let admission = ProbeAdmission::new(&fetcher, &page);
        let twin = Url::parse("https://www.shop.example/img/b.jpg").unwrap();
        let cdn = Url::parse("https://cdn.example.net/c.jpg").unwrap();

        let answers = verdicts(&admission, &[own_image(1), twin, cdn]).await;

        assert_eq!(
            answers,
            [
                ProbeVerdict::Refused,
                ProbeVerdict::Refused,
                ProbeVerdict::Send
            ]
        );
        assert_eq!(
            *fetcher.asked.lock().unwrap(),
            [
                "https://shop.example/img/1.jpg",
                "https://www.shop.example/img/b.jpg"
            ]
        );
    }

    /// A shop without a `Crawl-delay` is probed unpaced, as before, but its robots.txt still
    /// refuses what it disallows.
    #[tokio::test]
    async fn should_probe_a_shop_without_a_crawl_delay_unpaced() {
        let page = shop_page();
        for crawl_delay in [None, Some(std::time::Duration::ZERO)] {
            let fetcher = ShopRobots::answering(ImageProbeRobots::Allowed { crawl_delay });
            let admission = ProbeAdmission::new(&fetcher, &page);

            let answers = verdicts(&admission, &(1..=5).map(own_image).collect::<Vec<_>>()).await;

            assert_eq!(answers, [ProbeVerdict::Send; 5], "{crawl_delay:?}");
            assert!(fetcher.paced().is_empty(), "{crawl_delay:?}");
        }
    }

    /// A shop's `Crawl-delay` up to the cap delay paces every own-host probe, without a cap.
    #[tokio::test]
    async fn should_pace_every_probe_at_a_crawl_delay_up_to_the_cap_delay() {
        let page = shop_page();
        for seconds in [2, 10] {
            let fetcher = ShopRobots::with_crawl_delay(seconds);
            let admission = ProbeAdmission::new(&fetcher, &page);
            let images: Vec<Url> = (1..=5).map(own_image).collect();

            let answers = verdicts(&admission, &images).await;

            assert_eq!(answers, [ProbeVerdict::Send; 5], "{seconds}");
            assert_eq!(fetcher.paced().len(), 5, "{seconds}");
        }
    }

    /// Above the cap delay, a page probes its first three own-host images, paced, and leaves
    /// the rest unprobed; an image it already probes stays admitted, such as for a later
    /// extraction of the same page.
    #[tokio::test]
    async fn should_probe_only_the_first_images_of_a_page_above_the_cap_delay() {
        let page = shop_page();
        let fetcher = ShopRobots::with_crawl_delay(30);
        let admission = ProbeAdmission::new(&fetcher, &page);
        let images: Vec<Url> = (1..=5).map(own_image).collect();

        let answers = verdicts(&admission, &images).await;
        let again = admission.admit(&images[0], &images[0]).await;

        assert_eq!(
            answers,
            [
                ProbeVerdict::Send,
                ProbeVerdict::Send,
                ProbeVerdict::Send,
                ProbeVerdict::Capped,
                ProbeVerdict::Capped
            ]
        );
        assert_eq!(again, ProbeVerdict::Send);
        assert_eq!(
            fetcher.paced(),
            [
                "https://shop.example/img/1.jpg",
                "https://shop.example/img/2.jpg",
                "https://shop.example/img/3.jpg",
                "https://shop.example/img/1.jpg"
            ]
        );
    }

    /// A probe robots.txt refuses, or the cap leaves out, is not sent and leaves the size
    /// unknown, as a failed probe does: the image is kept unless its name says thumbnail. A
    /// capped probe is counted apart from the probes sent.
    #[tokio::test]
    async fn should_leave_a_refused_or_capped_image_unprobed() {
        let page = shop_page();
        let refusing = ShopRobots::answering(ImageProbeRobots::Refused);
        let slow = ShopRobots::with_crawl_delay(30);
        let slow_admission = ProbeAdmission::new(&slow, &page);
        for image in (1..=3).map(own_image) {
            assert_eq!(
                slow_admission.admit(&image, &image).await,
                ProbeVerdict::Send
            );
        }

        let ((refused, capped, telemetry), _) =
            crate::scraper::scraper_service::domain::product::with_scrape_observation(async {
                let refused =
                    probe_image_dimensions(&own_image(1), &ProbeAdmission::new(&refusing, &page))
                        .await;
                let capped = probe_image_dimensions(&own_image(4), &slow_admission).await;
                (
                    refused,
                    capped,
                    crate::scraper::scraper_service::domain::product::current_scrape_telemetry(),
                )
            })
            .await;

        assert_eq!(refused, ImageValidation::Unknown);
        assert_eq!(capped, ImageValidation::Unknown);
        assert_eq!(
            ImageUrlHint::NoHint.settle(capped),
            ImageValidation::Unknown
        );
        assert_eq!(telemetry.image_probes, 0);
        assert_eq!(telemetry.image_probes_capped, 1);
    }

    /// One page probes, paces and counts each image at most once, however many of its
    /// extractions list it: an image whose probe left the size unknown, was refused or was
    /// capped answers as it did before, without a second request, wait or count.
    #[tokio::test]
    async fn should_probe_each_image_of_a_page_at_most_once() {
        let page = Url::parse("https://shop.invalid/product/1").unwrap();
        let slow = ShopRobots::with_crawl_delay(30);
        let admission = ProbeAdmission::new(&slow, &page);
        let image =
            |index: usize| Url::parse(&format!("https://shop.invalid/img/{index}.jpg")).unwrap();
        let refusing = ShopRobots::answering(ImageProbeRobots::Refused);
        let refused_admission = ProbeAdmission::new(&refusing, &page);

        let (telemetry, _) =
            crate::scraper::scraper_service::domain::product::with_scrape_observation(async {
                for _extraction in 0..3 {
                    for index in 1..=5 {
                        probe_image_dimensions(&image(index), &admission).await;
                    }
                    probe_image_dimensions(&image(1), &refused_admission).await;
                }
                crate::scraper::scraper_service::domain::product::current_scrape_telemetry()
            })
            .await;

        // The host does not resolve, so each sent probe fails and leaves the size unknown.
        assert_eq!(
            slow.paced(),
            [
                "https://shop.invalid/img/1.jpg",
                "https://shop.invalid/img/2.jpg",
                "https://shop.invalid/img/3.jpg"
            ]
        );
        assert_eq!(telemetry.image_probes, 3);
        assert_eq!(telemetry.image_probes_capped, 2);
        assert_eq!(refusing.asked.lock().unwrap().len(), 1);
    }

    struct StaticValidator {
        validation: ImageValidation,
    }

    #[async_trait::async_trait]
    impl ImageValidator for StaticValidator {
        async fn validate(&self, _: &Url, _: &ProbeAdmission<'_>) -> ImageValidation {
            self.validation
        }
    }

    /// Looks `url` up in `validator`'s cache with a probe that returns `probed`
    /// and counts each probe it runs in `probes`.
    async fn cached_quality_with_probe(
        validator: &ReqwestImageValidator,
        url: &Url,
        probed: ImageValidation,
        probes: &AtomicUsize,
    ) -> ImageValidation {
        validator
            .cached_quality(url, || async {
                probes.fetch_add(1, Ordering::SeqCst);
                probed
            })
            .await
    }

    #[tokio::test]
    async fn should_probe_again_after_an_unknown_size_and_serve_decided_sizes_from_cache() {
        let validator = ReqwestImageValidator::new();
        let flaky = Url::parse(THUMBNAIL_NAMED_UPLOAD).unwrap();
        let small = Url::parse("https://example.com/images/badge.jpg").unwrap();
        let probes = AtomicUsize::new(0);
        let lookup = |url, probed| cached_quality_with_probe(&validator, url, probed, &probes);

        assert_eq!(
            lookup(&flaky, ImageValidation::Unknown).await,
            ImageValidation::Unknown
        );
        assert_eq!(
            lookup(&flaky, ImageValidation::Valid).await,
            ImageValidation::Valid
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            2,
            "an unknown size is probed again"
        );

        assert_eq!(
            lookup(&flaky, ImageValidation::Invalid).await,
            ImageValidation::Valid
        );
        assert_eq!(
            lookup(&small, ImageValidation::Invalid).await,
            ImageValidation::Invalid
        );
        assert_eq!(
            lookup(&small, ImageValidation::Valid).await,
            ImageValidation::Invalid
        );
        assert_eq!(
            probes.load(Ordering::SeqCst),
            3,
            "valid and invalid sizes are served from the cache"
        );
    }

    /// A range probe asks for the identity encoding on the wire, so the bytes it reads are the
    /// image's own first bytes, while the crawler client names gzip and brotli for pages.
    #[tokio::test]
    async fn range_probe_should_ask_for_the_identity_encoding() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = Url::parse(&format!(
            "http://{}/image.jpg",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.unwrap();
                assert_ne!(read, 0, "request ended before its headers");
                request.extend_from_slice(&buffer[..read]);
            }
            socket
                .write_all(b"HTTP/1.1 206 Partial Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .unwrap();
            String::from_utf8(request).unwrap().to_ascii_lowercase()
        });
        let client = crate::network::policy::crawler_client_builder(RequestTimeouts::uniform(
            std::time::Duration::from_secs(5),
        ))
        .unwrap()
        .build()
        .unwrap();

        range_probe(&client, &url).send().await.unwrap();

        let request = server.await.unwrap();
        let mut headers = request
            .lines()
            .filter(|line| line.starts_with("accept-encoding:") || line.starts_with("range:"))
            .collect::<Vec<_>>();
        headers.sort_unstable();
        assert_eq!(
            headers,
            vec!["accept-encoding: identity", "range: bytes=0-32767"]
        );
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
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
            &ProbeAdmission::new(&AdmitAll, &base),
        )
        .await;
        let alone = filter_valid_image_urls(
            vec![THUMBNAIL_NAMED_UPLOAD.to_string()],
            &base,
            &unknown_size,
            &ProbeAdmission::new(&AdmitAll, &base),
        )
        .await;

        assert_eq!(
            with_fallback,
            vec!["https://example.com/images/wall-light.jpg"]
        );
        assert!(alone.is_empty());
    }

    #[tokio::test]
    async fn should_keep_offline_the_images_capture_keeps_when_every_probe_accepts() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let raw_images = vec![
            format!("/image-100x100.jpg{IMAGE_CANDIDATE_SEPARATOR}/image-800x600.jpg"),
            "/photo.jpg?width=1000&height=100&mode=max".to_string(),
            THUMBNAIL_NAMED_UPLOAD.to_string(),
            "//".to_string(),
            "https://example.com/image-800x600.jpg".to_string(),
        ];
        let captured = filter_valid_image_urls(
            raw_images.clone(),
            &base,
            &StaticValidator {
                validation: ImageValidation::Valid,
            },
            &ProbeAdmission::new(&AdmitAll, &base),
        )
        .await;

        assert_eq!(
            captured,
            vec![
                "https://example.com/image-800x600.jpg",
                THUMBNAIL_NAMED_UPLOAD
            ],
            "URL dimensions reject without a probe; a thumbnail name alone does not; a repeated image is kept once"
        );
        assert_eq!(filter_image_urls_offline(&raw_images, &base), captured);
    }

    /// Capture's target check rejects these URLs before resolving any host, so offline
    /// ranking rejects them too and falls back to the group's next candidate.
    #[tokio::test]
    async fn should_reject_offline_the_targets_capture_rejects_by_their_url() {
        let base = Url::parse("https://example.com/products/1").unwrap();
        let unsafe_targets: Vec<String> = [
            "http://10.0.0.5/image-800x600.jpg",
            "http://[::1]/image.jpg",
            "https://user:secret@example.com/image.jpg",
            "https://example.com:8443/image.jpg",
            "ftp://example.com/image.jpg",
        ]
        .map(String::from)
        .to_vec();

        let captured = filter_valid_image_urls(
            unsafe_targets.clone(),
            &base,
            &ReqwestImageValidator::new(),
            &ProbeAdmission::new(&AdmitAll, &base),
        )
        .await;

        assert!(captured.is_empty());
        assert_eq!(filter_image_urls_offline(&unsafe_targets, &base), captured);
        assert_eq!(
            filter_image_urls_offline(
                &[format!(
                    "http://10.0.0.5/image.jpg{IMAGE_CANDIDATE_SEPARATOR}/image.jpg"
                )],
                &base
            ),
            vec!["https://example.com/image.jpg"]
        );
    }

    #[rstest]
    #[case::small_archive_photo(228, 200)]
    #[case::portrait_archive_photo(200, 228)]
    #[case::smallest_landscape(200, 150)]
    #[case::smallest_portrait(150, 200)]
    fn should_accept_a_small_photo_at_or_above_the_floor(
        #[case] width: usize,
        #[case] height: usize,
    ) {
        assert_eq!(
            validation_for_dimensions(width, height),
            ImageValidation::Valid
        );
    }

    #[rstest]
    #[case::favicon(32, 32)]
    #[case::icon(64, 64)]
    #[case::square_thumbnail(150, 150)]
    #[case::logo_strip(300, 80)]
    #[case::wide_banner(1000, 120)]
    #[case::just_below_the_longest_side(199, 150)]
    #[case::just_below_the_shortest_side(200, 149)]
    fn should_reject_an_icon_logo_or_thumbnail_below_the_floor(
        #[case] width: usize,
        #[case] height: usize,
    ) {
        assert_eq!(
            validation_for_dimensions(width, height),
            ImageValidation::Invalid
        );
    }

    #[tokio::test]
    async fn should_still_reject_resize_query_with_a_small_height() {
        let resized =
            "https://example.com/Object/636610351541085812.jpg?width=1000&height=100&mode=max";
        let base = Url::parse("https://example.com/objects/1").unwrap();
        let result = filter_valid_image_urls(
            vec![resized.to_string()],
            &base,
            &StaticValidator {
                validation: ImageValidation::Valid,
            },
            &ProbeAdmission::new(&AdmitAll, &base),
        )
        .await;

        assert_eq!(
            ImageUrlHint::of(&Url::parse(resized).unwrap()),
            ImageUrlHint::Dimensions(ImageValidation::Invalid)
        );
        assert!(result.is_empty());
    }
}
