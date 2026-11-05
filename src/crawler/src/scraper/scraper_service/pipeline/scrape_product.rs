use crate::network::policy::NetworkErrorKind;

use crate::scraper::candidate_service::RedirectDestination;
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::removed_page_schema::{
    RemovedPageMatch, RemovedPageSchema, RemovedPageVerdict, match_removed_page,
    removed_page_verdict,
};
use crate::scraper::raw_input::{capture_source_listing_id, crawler_raw_input};
use crate::scraper::scraper_service::domain::errors::{
    RemovalEvidence, RemovalEvidenceKind, ScraperError,
};
use crate::scraper::scraper_service::domain::product::{
    DomainFetchHealth, ScrapeMode, ScrapeOutcome, ScrapeRequest, ScrapedProduct, ScraperService,
    begin_transport_observation, current_fetch_failure, current_transport_observation,
    scrape_health_for_scraper_error, with_scrape_observation,
};
use crate::scraper::scraper_service::html_fetcher::FetchedHtml;
use crate::scraper::scraper_service::pipeline::cached_schema_selection::{
    ExistingSchemaSelection, ExtractedSchemaSet, SelectedSchemaCandidate,
};
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::hash::{
    EXTRACTOR_VERSION, fingerprint_scraper_context, hash_extractions,
};
use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use crate::spider::utils::url::CrawledUrl;
use crate::spider::utils::url::canonical::{is_language_segment, without_language_segment};
use listing_source_core::ListingSourceId;
use regex::Regex;
use tracing::{debug, warn};
use url::Url;

/// Where a product URL's redirect on its own host, bare or `www.`, led, as the redirect
/// removal evidence rule reads it (`docs/crawler/networking.md#redirect-removal-evidence`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RedirectTarget {
    /// The page the URL names: no redirect, or one that only adds or removes the trailing slash
    /// or adds, removes or replaces the language segment before the path.
    SamePage,
    /// Another page the product URL pattern takes or, for a ListingSource without a usable pattern,
    /// any other page but the homepage.
    ProductPage,
    /// The shop's homepage, with or without a language segment: removal evidence on its own.
    Homepage,
    /// Another page outside the product URL pattern: removal evidence once every cached schema
    /// fails on it.
    OutsideProductPattern,
}

/// Where a product URL's redirect led and the URL the row is stored under afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Redirect {
    pub(crate) target: RedirectTarget,
    /// The requested URL for a redirect to the same page; otherwise the redirect target in its
    /// stored spelling. A target that adds a language segment to a row stored without one is
    /// stored without it when the product URL pattern takes that spelling, so the row stays a
    /// product URL of the ListingSource (`docs/crawler/networking.md#stored-urls`).
    pub(crate) url: Url,
}

impl Redirect {
    /// Where `original_url`, fetched as `final_url`, led under `product_url_pattern`.
    pub(crate) fn of(
        original_url: &Url,
        final_url: &Url,
        product_url_pattern: Option<&str>,
    ) -> Self {
        let original = CrawledUrl::new(original_url.clone());
        let target = CrawledUrl::new(final_url.clone());
        if original.is_same_page(&target) || differ_only_in_language_segment(&original, &target) {
            return Self {
                target: RedirectTarget::SamePage,
                url: original_url.clone(),
            };
        }
        let stored = |target: CrawledUrl, kind| Self {
            target: kind,
            url: target.as_url().clone(),
        };
        if is_homepage(&target) {
            return stored(target, RedirectTarget::Homepage);
        }
        let Some(pattern) = product_url_pattern.and_then(|raw| Regex::new(raw).ok()) else {
            return stored(target, RedirectTarget::ProductPage);
        };
        // The recognized language segments are a guess about the path (`/art/12`), so the
        // unprefixed spelling is stored only where the pattern takes it.
        let unprefixed = without_language_segment(&original)
            .is_none()
            .then(|| without_language_segment(&target))
            .flatten()
            .map(CrawledUrl::new)
            .filter(|unprefixed| unprefixed.matches_pattern(&pattern));
        match unprefixed {
            Some(unprefixed) => stored(unprefixed, RedirectTarget::ProductPage),
            None if target.matches_pattern(&pattern) => stored(target, RedirectTarget::ProductPage),
            None => stored(target, RedirectTarget::OutsideProductPattern),
        }
    }
}

impl RedirectTarget {
    /// Where `original_url`, fetched as `final_url`, led under `product_url_pattern`.
    pub(crate) fn of(
        original_url: &Url,
        final_url: &Url,
        product_url_pattern: Option<&str>,
    ) -> Self {
        Redirect::of(original_url, final_url, product_url_pattern).target
    }

    /// Whether the target is a product page: the requested page or another one the product URL
    /// pattern takes.
    pub(crate) fn is_product_page(self) -> bool {
        matches!(self, Self::SamePage | Self::ProductPage)
    }
}

/// Whether `original_url` redirected on its own host to a page that is not a product page: the
/// homepage, or a page outside `product_url_pattern`. A redirect that names the same page, such
/// as one that only changes the trailing slash or the language segment, is none.
pub(crate) fn is_redirect_to_non_product_page(
    original_url: &Url,
    final_url: &Url,
    product_url_pattern: Option<&str>,
) -> bool {
    is_same_logical_host(original_url, final_url)
        && !RedirectTarget::of(original_url, final_url, product_url_pattern).is_product_page()
}

/// Whether `left` and `right` name one path and query apart from the language segment before the
/// path, which one of them adds, removes or replaces (`/x`, `/en-us/x`, `/de/x`).
fn differ_only_in_language_segment(left: &CrawledUrl, right: &CrawledUrl) -> bool {
    let (left_unprefixed, right_unprefixed) = (
        without_language_segment(left),
        without_language_segment(right),
    );
    if left_unprefixed.is_none() && right_unprefixed.is_none() {
        return false;
    }
    let left = left_unprefixed.unwrap_or_else(|| left.page_spelling());
    let right = right_unprefixed.unwrap_or_else(|| right.page_spelling());
    left.path() == right.path() && left.query() == right.query()
}

pub(crate) fn is_same_logical_host(left: &Url, right: &Url) -> bool {
    normalize_host(left) == normalize_host(right)
}

fn normalize_host(url: &Url) -> Option<&str> {
    url.host_str()
        .map(|host| host.strip_prefix("www.").unwrap_or(host))
}

/// Whether `url` is the shop's homepage: the root path, or a language segment alone (`/en-us/`).
fn is_homepage(url: &CrawledUrl) -> bool {
    let path = url.as_url().path().trim_matches('/');
    path.is_empty() || is_language_segment(path)
}

impl ScraperServiceImpl {
    #[tracing::instrument(
        skip(self, expected_last_captured_raw_input_sha256),
        fields(listing_source_id = %listing_source_id, url = %url)
    )]
    pub(crate) async fn mark_url_other_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) {
        match self
            .candidate_service
            .set_class(
                listing_source_id,
                url,
                crate::spider::classification::url_metadata::UrlClass::Other,
                expected_last_captured_raw_input_sha256,
            )
            .await
        {
            Ok(CrawlerUrlWriteOutcome::Applied) => {}
            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                debug!(
                    crawler_url_write_outcome = "stale_noop",
                    "Skipped stale crawler URL classification"
                );
            }
            Err(error) => {
                warn!(error = %error, "Failed to mark URL as other");
            }
        }
    }

    /// Records a scrape that skipped extraction because neither the page nor the scraper
    /// context changed since the last scrape. A failed write only delays the next scrape's
    /// skip, so it is logged instead of failing the scrape.
    async fn touch_unchanged_page(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        page_hash: &str,
        schema_fingerprint: &str,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) {
        match self
            .candidate_service
            .touch_scraped(
                listing_source_id,
                url,
                page_hash,
                schema_fingerprint,
                expected_last_captured_raw_input_sha256,
            )
            .await
        {
            Ok(CrawlerUrlWriteOutcome::Applied) => {}
            Ok(CrawlerUrlWriteOutcome::NoopStale) => {
                debug!("Skipped stale page/schema fast-path completion");
            }
            Err(error) => {
                warn!(error = %error, "Failed to touch URL after page/schema fast-path skip");
            }
        }
    }

    #[allow(clippy::result_large_err)]
    async fn removed_page_schemas_for(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<RemovedPageSchema>, ScraperError> {
        let mut schemas = Vec::new();

        if let Some(stored) = self
            .removed_page_schema_repository
            .find_removed_page_schema(listing_source_id)
            .await
            .map_err(ScraperError::RemovedPageSchemaDatabaseError)?
        {
            schemas.extend(stored.removed_page_schemas);
        }

        Ok(schemas)
    }
}

/// A fetched product page schemas are obtained or selected for.
#[derive(Clone, Copy)]
pub(crate) struct FetchedPage<'a> {
    pub(crate) listing_source_id: &'a ListingSourceId,
    pub(crate) domain: &'a str,
    /// The crawler URL the scrape requested, whose row the scrape writes.
    pub(crate) requested_url: &'a Url,
    /// The URL the page is captured under: `requested_url`, or the redirect target when a
    /// redirect moved the page.
    pub(crate) url: &'a Url,
    /// The success status the page was answered with.
    pub(crate) status: u16,
    /// Where a redirect of `requested_url` led.
    pub(crate) redirect: RedirectTarget,
    pub(crate) html: &'a str,
    pub(crate) expected_last_captured_raw_input_sha256: Option<&'a [u8]>,
}

impl<'a> FetchedPage<'a> {
    fn with_existing_schemas<'s>(
        self,
        existing_schemas: &'s [ProductCssSelectorSchema],
    ) -> FreshSchemaGenerationContext<'s>
    where
        'a: 's,
    {
        FreshSchemaGenerationContext {
            listing_source_id: self.listing_source_id,
            domain: self.domain,
            requested_url: self.requested_url,
            url: self.url,
            html: self.html,
            existing_schemas,
            expected_last_captured_raw_input_sha256: self.expected_last_captured_raw_input_sha256,
        }
    }
}

/// One parse of a fetched page: the stored removed-page schemas that match it and what the
/// cached product schemas extract from it. Synchronous, so the `!Send` document is dropped
/// before the scrape awaits again; a page with neither is not parsed here. A schema set known
/// only after a later await is extracted from a second parse
/// ([`ExtractedSchemaSet::parse_and_extract`] says when and why).
fn parse_fetched_page(
    html: &str,
    removed_page_schemas: Vec<RemovedPageSchema>,
    cached_product_schemas: Option<ListingSourceProductSchema>,
) -> (Vec<RemovedPageMatch>, Option<ExtractedSchemaSet>) {
    if removed_page_schemas.is_empty() && cached_product_schemas.is_none() {
        return (Vec::new(), None);
    }
    let parsed = scraper::Html::parse_document(html);
    (
        match_removed_page(removed_page_schemas, &parsed),
        cached_product_schemas.map(|schemas| ExtractedSchemaSet::extract(schemas, &parsed)),
    )
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    #[tracing::instrument(skip(self, request), fields(listing_source_id = %request.listing_source_id, url = %request.url, mode = ?request.mode))]
    async fn scrape(&self, request: ScrapeRequest) -> ScrapeOutcome {
        let (result, observed_scrape_health, fetch_failure) = with_scrape_observation(async {
            let result = self.scrape_request(&request).await;
            let observed = current_transport_observation();
            let fetch_failure = current_fetch_failure();
            (result, observed, fetch_failure)
        })
        .await
        .0;
        let scrape_health = observed_scrape_health.unwrap_or_else(|| match &result {
            Ok(_) => DomainFetchHealth::Responsive,
            Err(error) => scrape_health_for_scraper_error(error),
        });
        ScrapeOutcome {
            result,
            scrape_health,
            fetch_failure,
        }
    }
}

/// A fetched product page and where it lives.
struct FetchedProductPage {
    acquired_at: time::OffsetDateTime,
    html: String,
    /// The URL the page is captured under: the requested URL, or the redirect target when a
    /// redirect moved the page.
    effective_url: Url,
    /// The URL the shop answered the page under, as fetched.
    final_url: Url,
    status: u16,
    redirect: RedirectTarget,
}

impl FetchedProductPage {
    /// Locates the page `fetched` from the requested `url`. A redirect that only adds or removes
    /// the trailing slash or a language segment leads to the page the row is stored under, so
    /// the row keeps its URL (`docs/crawler/networking.md#stored-urls`). Fails when a redirect
    /// left the requested URL's host (or its `www.` twin) or led to the homepage.
    #[allow(clippy::result_large_err)]
    fn locate(
        url: &Url,
        fetched: FetchedHtml,
        product_url_pattern: Option<&str>,
        acquired_at: time::OffsetDateTime,
    ) -> Result<Self, ScraperError> {
        if !is_same_logical_host(url, &fetched.final_url) {
            return Err(ScraperError::HttpError {
                url: url.clone(),
                kind: NetworkErrorKind::UnsafeTarget,
                details: format!(
                    "product URL redirected outside configured crawler domain: original={url}, final={}",
                    fetched.final_url
                ),
            });
        }
        let status = fetched.status.as_u16();
        let Redirect {
            target: redirect,
            url: effective_url,
        } = Redirect::of(url, &fetched.final_url, product_url_pattern);
        if redirect == RedirectTarget::Homepage {
            return Err(ScraperError::product_listing_removed(
                url.clone(),
                format!(
                    "product URL redirected to the homepage: original={url}, final={}",
                    fetched.final_url
                ),
                RemovalEvidence::page(
                    RemovalEvidenceKind::RedirectToHomepage,
                    fetched.final_url,
                    Some(status),
                ),
                Vec::new(),
            ));
        }
        Ok(Self {
            acquired_at,
            html: fetched.html,
            effective_url,
            final_url: fetched.final_url,
            status,
            redirect,
        })
    }

    /// Whether a redirect moved the page to another URL than the requested one.
    fn url_moved(&self) -> bool {
        self.redirect != RedirectTarget::SamePage
    }
}

/// What an effective schema set extracts from a page, as the last scrape stored it: the page
/// hash and the scraper-context fingerprint.
struct PageFingerprints {
    page_hash: String,
    schema_fingerprint: String,
}

impl PageFingerprints {
    #[allow(clippy::result_large_err)]
    fn of(extracted: &ExtractedSchemaSet) -> Result<Self, ScraperError> {
        Ok(Self {
            schema_fingerprint: fingerprint_scraper_context(
                EXTRACTOR_VERSION,
                extracted.product_schemas(),
            )
            .map_err(ScraperError::SchemaFingerprint)?,
            page_hash: extracted.page_hash(),
        })
    }

    /// Whether neither the page nor the scraper context changed since the last scrape of
    /// `request`.
    fn match_last_scrape(&self, request: &ScrapeRequest) -> bool {
        request.last_scraped_hash.as_deref() == Some(self.page_hash.as_str())
            && request.last_scraped_schema_fingerprint.as_deref()
                == Some(self.schema_fingerprint.as_str())
    }
}

/// Withdraws a page a stored removed-page schema matches. Withdrawal needs unambiguous evidence:
/// a removed-page template that also matches a page a cached schema still extracts this listing
/// from is an over-broad stored template. Product evidence wins and the page is scraped normally;
/// the warning names the row an operator has to fix.
#[allow(clippy::result_large_err)]
fn withdraw_removed_page(
    request: &ScrapeRequest,
    page: &FetchedProductPage,
    verdict: RemovedPageVerdict,
) -> Result<(), ScraperError> {
    match verdict {
        RemovedPageVerdict::NotRemoved => Ok(()),
        RemovedPageVerdict::Removed { matches } => Err(ScraperError::product_listing_removed(
            request.url.clone(),
            "soft-404 removed page matched configured removed-page schema".to_string(),
            RemovalEvidence::page(
                RemovalEvidenceKind::RemovedPageTemplate,
                page.final_url.clone(),
                Some(page.status),
            ),
            matches.into_iter().map(|matched| matched.schema).collect(),
        )),
        RemovedPageVerdict::ProductStillExtracts { matches } => {
            let selectors = matches
                .iter()
                .map(|matched| matched.schema.selector.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            let notice_count = matches
                .iter()
                .map(|matched| matched.notices.len())
                .sum::<usize>();
            warn!(
                listing_source_id = %request.listing_source_id,
                url = %request.url,
                removed_page_selectors = %selectors,
                removed_page_notice_count = notice_count,
                "Stored removed-page schema matched a page a cached product schema still extracts; scraping it as a product"
            );
            Ok(())
        }
    }
}

/// The product `selection` captures from `page`, with the hash and fingerprint of the schema set
/// it was selected from: `extracted`, with the fresh schema appended when one was authored.
#[allow(clippy::result_large_err)]
fn capture_selection(
    request: &ScrapeRequest,
    page: FetchedProductPage,
    extracted: ExtractedSchemaSet,
    selection: SelectedSchemaCandidate,
) -> Result<ScrapedProduct, ScraperError> {
    // A fresh schema's raw values are its extraction of this page, so the set with it appended
    // hashes without another parse.
    let hash = hash_extractions(
        extracted
            .extraction_outcomes()
            .chain(selection.fresh_schema.then_some(Some(&selection.raw))),
    );
    let mut schemas = extracted.into_schemas().product_schemas;
    if selection.fresh_schema {
        schemas.push(selection.schema.clone());
    }
    let schema_fingerprint = fingerprint_scraper_context(EXTRACTOR_VERSION, &schemas)
        .map_err(ScraperError::SchemaFingerprint)?;
    let source_listing_id =
        capture_source_listing_id(&selection.raw.source_listing_id, &request.source_record_key)?;
    let raw_input = crawler_raw_input(
        &selection.raw,
        &selection.validated_image_urls,
        &page.effective_url,
        source_listing_id.as_ref(),
        selection.carried_forward_title.as_deref(),
    )
    .map_err(ScraperError::RawNormalizationInput)?;
    let raw_input_sha256 = raw_input
        .hash()
        .map_err(ScraperError::RawNormalizationInput)?
        .as_bytes()
        .to_vec();
    Ok(ScrapedProduct {
        acquired_at: page.acquired_at,
        raw_input,
        effective_url: page.effective_url,
        source_listing_id: source_listing_id.to_string(),
        title: selection
            .carried_forward_title
            .unwrap_or(selection.raw.title),
        hash,
        schema_fingerprint,
        raw_input_sha256,
    })
}

/// The removal of a page a redirect took outside its product URL pattern to a page every cached
/// schema fails on.
fn redirect_to_non_product_page(page: FetchedPage<'_>) -> ScraperError {
    ScraperError::product_listing_removed(
        page.requested_url.clone(),
        format!(
            "product URL redirected outside its product URL pattern to a page every cached schema fails on: original={}, final={}",
            page.requested_url, page.url
        ),
        RemovalEvidence::page(
            RemovalEvidenceKind::RedirectToNonProductPage,
            page.url.clone(),
            Some(page.status),
        ),
        Vec::new(),
    )
}

impl ScraperServiceImpl {
    async fn scrape_request(
        &self,
        request: &ScrapeRequest,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let listing_source_id = &request.listing_source_id;
        let url = &request.url;
        let expected_last_captured_raw_input_sha256 =
            request.expected_last_captured_raw_input_sha256.as_deref();

        begin_transport_observation();
        let domain = url
            .host_str()
            .ok_or_else(|| ScraperError::NoHost { url: url.clone() })?;
        let fetched = self.fetch_product_page(request, domain).await?;
        let page = FetchedProductPage::locate(
            url,
            fetched,
            request.product_url_pattern.as_deref(),
            time::OffsetDateTime::now_utc(),
        )?;

        // Where the redirect led is known from the fetch, so a page the crawler stores settles
        // the URL before anything is extracted from it.
        self.settle_redirect_destination(request, &page).await?;

        let removed_page_schemas = self.removed_page_schemas_for(listing_source_id).await?;
        // One schema-cache read and one parse serve the removal guard, the page hash and cached
        // schema selection.
        let cached_product_schemas = self
            .schema_service
            .find_product_schema(listing_source_id)
            .await?;
        let (removed_page_matches, cached_extraction) =
            parse_fetched_page(&page.html, removed_page_schemas, cached_product_schemas);
        withdraw_removed_page(
            request,
            &page,
            removed_page_verdict(
                removed_page_matches,
                cached_extraction
                    .iter()
                    .flat_map(ExtractedSchemaSet::raw_extractions),
                &request.source_record_key,
                request.last_source_listing_id.as_deref(),
            ),
        )?;

        // Ahead of the schemas and the unchanged-page skip, so a page rejected as no product page
        // is never captured, whichever schema extracts it, and ahead of the unverified-target
        // check, so a redirect onto it is a removal even before any schema is cached.
        self.skip_rejected_page(
            listing_source_id,
            url,
            &page.effective_url,
            expected_last_captured_raw_input_sha256,
        )
        .await?;

        // A redirect outside the pattern that no cached schema can judge is no page to capture or
        // to open a schema review on.
        if page.redirect == RedirectTarget::OutsideProductPattern
            && cached_extraction
                .as_ref()
                .is_none_or(|extracted| extracted.product_schemas().is_empty())
        {
            return Err(ScraperError::RedirectTargetUnverified {
                destination: page.effective_url.clone(),
            });
        }

        let fetched_page = FetchedPage {
            listing_source_id,
            domain,
            requested_url: url,
            url: &page.effective_url,
            status: page.status,
            redirect: page.redirect,
            html: &page.html,
            expected_last_captured_raw_input_sha256,
        };
        // Obtain the effective schema set before the fast path: the page hash is what that set
        // extracts from the page, and the fingerprint forces extraction whenever the set changes.
        // Obtained schemas are known only after awaits, so they are extracted from a second parse.
        let extracted = match cached_extraction {
            Some(extracted) => extracted,
            None => ExtractedSchemaSet::parse_and_extract(
                self.obtain_schemas(
                    fetched_page,
                    request.product_url_pattern.as_deref(),
                    request.mode,
                )
                .await?,
                &page.html,
            ),
        };
        let stored = PageFingerprints::of(&extracted)?;

        if !page.url_moved() && stored.match_last_scrape(request) {
            debug!("Page and schema fingerprints match; skipping extraction.");
            self.touch_unchanged_page(
                listing_source_id,
                url,
                &stored.page_hash,
                &stored.schema_fingerprint,
                expected_last_captured_raw_input_sha256,
            )
            .await;
            return Ok(None);
        }

        let (extracted, selection) = self
            .select_schema_for_page(
                extracted,
                fetched_page,
                request.last_captured_title.as_deref(),
            )
            .await?;
        let scraped = capture_selection(request, page, extracted, selection)?;

        // `mark_as_scraped` is intentionally deferred until raw capture succeeds.
        debug!(domain, "Scraping complete");
        Ok(Some(scraped))
    }

    /// Settles a URL that redirected by the rows the crawler stores at its target
    /// (`docs/crawler/networking.md#duplicate-targets`): a target that is no product page of the
    /// ListingSource is a removal whatever the page shows, a product URL it stores under its own
    /// row retires the URL, and rows that disagree leave it unsettled. `Ok` for a target no row
    /// stores, which the scrape judges by what it extracts.
    #[allow(clippy::result_large_err)]
    async fn settle_redirect_destination(
        &self,
        request: &ScrapeRequest,
        page: &FetchedProductPage,
    ) -> Result<(), ScraperError> {
        if request.mode == ScrapeMode::PrimaryOnly || !page.url_moved() {
            return Ok(());
        }
        let destination = self
            .candidate_service
            .redirect_destination(&request.listing_source_id, &page.effective_url)
            .await
            .map_err(ScraperError::RedirectDestinationLookup)?;
        let url = request.url.clone();
        let destination_url = page.effective_url.clone();
        match destination {
            RedirectDestination::Unknown => Ok(()),
            RedirectDestination::ProductOfListingSource => {
                Err(ScraperError::RedirectToKnownProduct {
                    destination: destination_url,
                })
            }
            RedirectDestination::Unsettled => Err(ScraperError::RedirectDestinationUnsettled {
                destination: destination_url,
            }),
            RedirectDestination::NotAProductOfListingSource => {
                Err(ScraperError::product_listing_removed(
                    url.clone(),
                    format!(
                        "product URL redirected to a page stored as no product URL of its ListingSource: original={url}, final={}",
                        page.final_url
                    ),
                    RemovalEvidence::page(
                        RemovalEvidenceKind::RedirectToStoredNonProductPage,
                        page.final_url.clone(),
                        Some(page.status),
                    ),
                    Vec::new(),
                ))
            }
        }
    }

    /// Selects the best-ranked plausible cached schema for the page in `page` from what `extracted`
    /// extracted, or authors a fresh one when none applies. Returns the extracted schema set the
    /// selection used with the selection. A
    /// page that leaves a required value empty where a cached schema found its element fails
    /// with [`ScraperError::MissingRequiredValue`] without waiting on or authoring a schema, and a
    /// page a redirect took outside the product URL pattern that every cached schema fails on
    /// is a removed listing.
    ///
    /// Scraping continues while a product-schema review is pending, so its approval can replace the
    /// live schemas after `schemas` was read. A page that failed the stale list is therefore
    /// selected once more against the live list before it may author a schema, record a missing
    /// required value or be removed; otherwise it would open a review whose candidate drops the
    /// schemas just approved, or wait a scrape interval for a fix the approval already made live.
    async fn select_schema_for_page(
        &self,
        mut extracted: ExtractedSchemaSet,
        page: FetchedPage<'_>,
        last_captured_title: Option<&str>,
    ) -> Result<(ExtractedSchemaSet, SelectedSchemaCandidate), ScraperError> {
        let mut reread = false;
        loop {
            let selection = self
                .select_cached_schema(
                    page.listing_source_id,
                    page.url,
                    &extracted,
                    last_captured_title,
                )
                .await;
            let reason = match selection {
                ExistingSchemaSelection::Selected(selection) => return Ok((extracted, *selection)),
                ExistingSchemaSelection::MissingRequiredValue { findings } => {
                    if let Some(live) = self
                        .live_schemas_changed_once(&mut reread, page, &extracted)
                        .await?
                    {
                        extracted = live;
                        continue;
                    }
                    return Err(ScraperError::MissingRequiredValue {
                        url: page.url.clone(),
                        findings,
                    });
                }
                ExistingSchemaSelection::GenerateNewSchema { reason } => reason,
            };
            if page.redirect == RedirectTarget::OutsideProductPattern
                && !extracted.product_schemas().is_empty()
            {
                if let Some(live) = self
                    .live_schemas_changed_once(&mut reread, page, &extracted)
                    .await?
                {
                    extracted = live;
                    continue;
                }
                return Err(redirect_to_non_product_page(page));
            }
            let ctx = page.with_existing_schemas(extracted.product_schemas());
            self.wait_instead_of_authoring(&ctx).await?;
            if let Some(live) = self
                .live_schemas_changed_once(&mut reread, page, &extracted)
                .await?
            {
                extracted = live;
                continue;
            }
            let ctx = page.with_existing_schemas(extracted.product_schemas());
            debug!(
                domain = page.domain,
                schemas = extracted.product_schemas().len(),
                fresh_schema_generation_reason = reason.as_str(),
                "Cached selection exhausted; generating new schema"
            );
            let selection = self.generate_fresh_schema_for_page(ctx).await?;
            return Ok((extracted, selection));
        }
    }

    /// The live schemas extracted from the page when they changed since `extracted` was read,
    /// looked up at most once per scrape (`reread` records it). The live set is known only after
    /// awaits, so it is extracted from a second parse.
    async fn live_schemas_changed_once(
        &self,
        reread: &mut bool,
        page: FetchedPage<'_>,
        extracted: &ExtractedSchemaSet,
    ) -> Result<Option<ExtractedSchemaSet>, ScraperError> {
        if *reread {
            return Ok(None);
        }
        *reread = true;
        let Some(live) = self
            .product_schemas_changed_since(page.listing_source_id, extracted.product_schemas())
            .await?
        else {
            return Ok(None);
        };
        debug!(
            schemas = live.product_schemas.len(),
            "Live product schemas changed during the scrape; selecting again"
        );
        Ok(Some(ExtractedSchemaSet::parse_and_extract(live, page.html)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(original: &str, served: &str, pattern: Option<&str>) -> RedirectTarget {
        RedirectTarget::of(
            &Url::parse(original).unwrap(),
            &Url::parse(served).unwrap(),
            pattern,
        )
    }

    #[test]
    fn should_name_the_same_page_when_a_redirect_only_adds_removes_or_replaces_the_language_segment()
     {
        for (original, served) in [
            (
                "https://shop.example/item/1",
                "https://shop.example/en-us/item/1",
            ),
            (
                "https://shop.example/en/item/1",
                "https://shop.example/item/1/",
            ),
            (
                "https://shop.example/en/item/1",
                "https://shop.example/de_DE/item/1",
            ),
            (
                "https://shop.example/item/1?id=2",
                "https://www.shop.example/fr/item/1?id=2",
            ),
        ] {
            assert_eq!(
                target(original, served, Some(r"^https://shop\.example/item/")),
                RedirectTarget::SamePage,
                "{original} -> {served}"
            );
        }
    }

    #[test]
    fn should_judge_another_page_behind_a_language_segment_by_the_pattern_with_or_without_it() {
        assert_eq!(
            target(
                "https://shop.example/item/1",
                "https://shop.example/en-us/category/chairs",
                Some(r"/item/")
            ),
            RedirectTarget::OutsideProductPattern
        );
        assert_eq!(
            target(
                "https://shop.example/item/1",
                "https://shop.example/en-us/item/2",
                Some(r"/item/")
            ),
            RedirectTarget::ProductPage
        );
        assert_eq!(
            target(
                "https://shop.example/item/1?id=2",
                "https://shop.example/en/item/1?id=3",
                Some(r"^https://shop\.example/item/")
            ),
            RedirectTarget::ProductPage
        );
    }

    #[test]
    fn should_take_the_root_of_a_language_for_the_homepage() {
        for served in [
            "https://shop.example/",
            "https://shop.example/en-us/",
            "https://shop.example/de",
        ] {
            assert_eq!(
                target("https://shop.example/en-us/item/1", served, Some(r"/")),
                RedirectTarget::Homepage,
                "{served}"
            );
        }
    }

    #[test]
    fn should_not_take_a_first_segment_that_is_no_language_for_a_language_segment() {
        assert_eq!(
            target(
                "https://shop.example/item/1",
                "https://shop.example/antiques/item/1",
                Some(r"^https://shop\.example/item/")
            ),
            RedirectTarget::OutsideProductPattern
        );
    }
}
