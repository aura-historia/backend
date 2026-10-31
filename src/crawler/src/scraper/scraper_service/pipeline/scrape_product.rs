use crate::network::policy::NetworkErrorKind;

use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::css_selector::removed_page_schema::{RemovedPageMatch, RemovedPageSchema};
use crate::scraper::raw_input::{capture_source_listing_id, crawler_raw_input};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::domain::product::{
    DomainFetchHealth, ScrapeMode, ScrapeOutcome, ScrapeRequest, ScrapedProduct, ScraperService,
    begin_transport_observation, current_fetch_failure, current_transport_observation,
    domain_health_for_scraper_error, with_scrape_observation,
};
use crate::scraper::scraper_service::html_fetcher::FetchedHtml;
use crate::scraper::scraper_service::pipeline::cached_schema_selection::{
    ExistingSchemaSelection, SelectedSchemaCandidate, apply_cached_schemas,
};
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::hash::{
    EXTRACTOR_VERSION, fingerprint_scraper_context, hash_page,
};
use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use crate::spider::utils::url::CrawledUrl;
use listing_source_core::ListingSourceId;
use money::Currency;
use regex::Regex;
use tracing::{debug, warn};
use url::Url;

/// Whether `original_url` redirected to a page that is not a product page: on the same host,
/// outside `product_url_pattern`, or to the homepage without one. A redirect that names the same
/// page, such as one that only adds or removes the trailing slash, is none.
pub(crate) fn is_redirect_to_non_product_page(
    original_url: &Url,
    final_url: &Url,
    product_url_pattern: Option<&str>,
) -> bool {
    let normalized_original = CrawledUrl::new(original_url.clone());
    let normalized_final = CrawledUrl::new(final_url.clone());

    if normalized_original.is_same_page(&normalized_final) {
        return false;
    }

    if !is_same_logical_host(original_url, final_url) {
        return false;
    }

    if let Some(pattern) = product_url_pattern.and_then(|raw| Regex::new(raw).ok()) {
        return !normalized_final.matches_pattern(&pattern);
    }

    is_homepage(final_url)
}

fn is_same_logical_host(left: &Url, right: &Url) -> bool {
    normalize_host(left) == normalize_host(right)
}

fn normalize_host(url: &Url) -> Option<&str> {
    url.host_str()
        .map(|host| host.strip_prefix("www.").unwrap_or(host))
}

fn is_homepage(url: &Url) -> bool {
    url.path().is_empty() || url.path() == "/"
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

/// A fetched product page a schema is selected for.
#[derive(Clone, Copy)]
struct FetchedPage<'a> {
    listing_source_id: &'a ListingSourceId,
    domain: &'a str,
    url: &'a Url,
    html: &'a str,
    expected_last_captured_raw_input_sha256: Option<&'a [u8]>,
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
            url: self.url,
            html: self.html,
            existing_schemas,
            expected_last_captured_raw_input_sha256: self.expected_last_captured_raw_input_sha256,
        }
    }
}

/// What the stored removed-page schemas say about a fetched page.
pub(crate) enum RemovedPageVerdict {
    /// No stored schema matches the page.
    NotRemoved,
    /// A stored schema matches and no cached product schema still extracts this
    /// listing's product from the page.
    Removed { matches: Vec<RemovedPageMatch> },
    /// A stored schema matches, but a cached product schema still extracts this
    /// listing's product: the stored template is over-broad.
    ProductStillExtracts { matches: Vec<RemovedPageMatch> },
}

/// Weighs the stored removed-page schemas against the cached product schemas on one
/// parse of the page. Synchronous and free of I/O: the `!Send` document never lives
/// across an await, no image is probed and no LLM budget is spent.
///
/// Product evidence is a cached schema that applies with a title other than the
/// matched notice and, when the crawler already knows the listing's identity, the
/// same source listing id. A related-item card on a soft-404 page therefore does not
/// count as evidence. Plausibility is deliberately not required: a candidate that
/// fails a plausibility check must reach fresh schema generation, not withdrawal.
pub(crate) fn removed_page_verdict(
    removed_page_schemas: Vec<RemovedPageSchema>,
    cached_schemas: &[ProductCssSelectorSchema],
    html: &str,
    source_record_key: &str,
    last_source_listing_id: Option<&str>,
) -> RemovedPageVerdict {
    if removed_page_schemas.is_empty() {
        return RemovedPageVerdict::NotRemoved;
    }
    removed_page_verdict_in(
        removed_page_schemas,
        cached_schemas,
        &scraper::Html::parse_document(html),
        source_record_key,
        last_source_listing_id,
    )
}

/// Same as [`removed_page_verdict`] on an already parsed page, so a caller that parsed the
/// page for other work does not parse it again.
pub(crate) fn removed_page_verdict_in(
    removed_page_schemas: Vec<RemovedPageSchema>,
    cached_schemas: &[ProductCssSelectorSchema],
    parsed: &scraper::Html,
    source_record_key: &str,
    last_source_listing_id: Option<&str>,
) -> RemovedPageVerdict {
    let matches = removed_page_schemas
        .into_iter()
        .filter_map(|schema| schema.match_page(parsed))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return RemovedPageVerdict::NotRemoved;
    }

    let product_still_extracts =
        apply_cached_schemas(cached_schemas, parsed)
            .iter()
            .any(|candidate| {
                let title = candidate.raw.title.trim();
                if title.is_empty() || matches.iter().any(|matched| matched.is_notice(title)) {
                    return false;
                }
                last_source_listing_id.is_none_or(|known| {
                    candidate_source_listing_id(&candidate.raw.source_listing_id, source_record_key)
                        .is_some_and(|extracted| extracted == known)
                })
            });
    if product_still_extracts {
        RemovedPageVerdict::ProductStillExtracts { matches }
    } else {
        RemovedPageVerdict::Removed { matches }
    }
}

/// The identity a raw extraction would be captured under, so it compares with the
/// crawler URL's last captured source listing id.
fn candidate_source_listing_id(
    raw_source_listing_id: &str,
    source_record_key: &str,
) -> Option<String> {
    capture_source_listing_id(raw_source_listing_id, source_record_key)
        .ok()
        .map(|id| id.to_string())
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    #[tracing::instrument(skip(self, request), fields(listing_source_id = %request.listing_source_id, url = %request.url, mode = ?request.mode))]
    async fn scrape(&self, request: ScrapeRequest) -> ScrapeOutcome {
        if request.mode != ScrapeMode::PrimaryOnly && request.domain_id.is_none() {
            return ScrapeOutcome {
                result: Err(ScraperError::MissingDomainContext {
                    url: request.url,
                    mode: match request.mode {
                        ScrapeMode::Normal => "normal",
                        ScrapeMode::DomainProbe => "domain probe",
                        ScrapeMode::PrimaryOnly => "primary-only",
                    },
                }),
                domain_health: DomainFetchHealth::NotObserved,
                fetch_failure: None,
            };
        }

        let (result, observed_domain_health, fetch_failure) = with_scrape_observation(async {
            let result = self.scrape_request(&request).await;
            let observed = current_transport_observation();
            let fetch_failure = current_fetch_failure();
            (result, observed, fetch_failure)
        })
        .await
        .0;
        let domain_health = observed_domain_health.unwrap_or_else(|| match &result {
            Ok(_) => DomainFetchHealth::Responsive,
            Err(error) => domain_health_for_scraper_error(error),
        });
        ScrapeOutcome {
            result,
            domain_health,
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
    url_moved: bool,
}

impl FetchedProductPage {
    /// Locates the page `fetched` from the requested `url`. A redirect that only adds or removes
    /// the trailing slash leads to the page the row is stored under, so the row keeps its URL
    /// (`docs/crawler/networking.md#stored-urls`). Fails when a redirect left the crawler domain
    /// or led to a non-product page.
    #[allow(clippy::result_large_err)]
    fn locate(
        url: &Url,
        fetched: FetchedHtml,
        product_url_pattern: Option<&str>,
        acquired_at: time::OffsetDateTime,
    ) -> Result<Self, ScraperError> {
        let normalized_original_url = CrawledUrl::new(url.clone());
        let normalized_final_url = CrawledUrl::new(fetched.final_url.clone());
        let url_moved = !normalized_original_url.is_same_page(&normalized_final_url);
        let effective_url = if url_moved {
            normalized_final_url.as_url().clone()
        } else {
            url.clone()
        };
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
        if is_redirect_to_non_product_page(url, &fetched.final_url, product_url_pattern) {
            return Err(ScraperError::ProductListingRemoved {
                url: url.clone(),
                details: format!(
                    "product URL redirected to non-product page: original={url}, final={}",
                    fetched.final_url
                ),
            });
        }
        Ok(Self {
            acquired_at,
            html: fetched.html,
            effective_url,
            url_moved,
        })
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
    fn of(
        html: &str,
        schemas: &[ProductCssSelectorSchema],
        fallback_currency: Option<Currency>,
    ) -> Result<Self, ScraperError> {
        Ok(Self {
            schema_fingerprint: fingerprint_scraper_context(
                EXTRACTOR_VERSION,
                schemas,
                fallback_currency,
            )
            .map_err(ScraperError::SchemaFingerprint)?,
            page_hash: hash_page(html, schemas),
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
    verdict: RemovedPageVerdict,
) -> Result<(), ScraperError> {
    match verdict {
        RemovedPageVerdict::NotRemoved => Ok(()),
        RemovedPageVerdict::Removed { .. } => Err(ScraperError::ProductListingRemoved {
            url: request.url.clone(),
            details: "soft-404 removed page matched configured removed-page schema".to_string(),
        }),
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
/// it was selected from. `stored` holds those of `schemas` before a fresh schema was appended.
#[allow(clippy::result_large_err)]
fn capture_selection(
    request: &ScrapeRequest,
    page: FetchedProductPage,
    mut schemas: Vec<ProductCssSelectorSchema>,
    selection: SelectedSchemaCandidate,
    stored: PageFingerprints,
) -> Result<ScrapedProduct, ScraperError> {
    if selection.fresh_schema {
        schemas.push(selection.schema.clone());
    }
    let schema_fingerprint =
        fingerprint_scraper_context(EXTRACTOR_VERSION, &schemas, request.fallback_currency)
            .map_err(ScraperError::SchemaFingerprint)?;
    let source_listing_id =
        capture_source_listing_id(&selection.raw.source_listing_id, &request.source_record_key)?;
    let raw_input = crawler_raw_input(
        &selection.raw,
        &selection.validated_image_urls,
        &page.effective_url,
        source_listing_id.as_ref(),
        request.fallback_currency,
    )
    .map_err(ScraperError::RawNormalizationInput)?;
    let raw_input_sha256 = raw_input
        .hash()
        .map_err(ScraperError::RawNormalizationInput)?
        .as_bytes()
        .to_vec();
    // The hash is stored with `schema_fingerprint`, so it is what the same effective set
    // extracts; an unchanged fingerprint means an unchanged set, already hashed.
    let hash = if schema_fingerprint == stored.schema_fingerprint {
        stored.page_hash
    } else {
        hash_page(&page.html, &schemas)
    };

    Ok(ScrapedProduct {
        acquired_at: page.acquired_at,
        raw_input,
        effective_url: page.effective_url,
        source_listing_id: source_listing_id.to_string(),
        hash,
        schema_fingerprint,
        raw_input_sha256,
    })
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

        let removed_page_schemas = self.removed_page_schemas_for(listing_source_id).await?;
        // One schema-cache read serves both the removal guard and schema obtainment.
        let cached_product_schemas = self
            .schema_service
            .find_product_schema(listing_source_id)
            .await?;
        withdraw_removed_page(
            request,
            removed_page_verdict(
                removed_page_schemas,
                cached_product_schemas
                    .as_ref()
                    .map_or(&[], |cached| cached.product_schemas.as_slice()),
                &page.html,
                &request.source_record_key,
                request.last_source_listing_id.as_deref(),
            ),
        )?;

        // Ahead of the schemas and the unchanged-page skip, so a page rejected as no product page
        // is never captured, whichever schema extracts it.
        self.skip_rejected_page(
            listing_source_id,
            &page.effective_url,
            expected_last_captured_raw_input_sha256,
        )
        .await?;

        // Obtain the effective schema set before the fast path: the page hash is what that set
        // extracts from the page, and the fingerprint forces extraction whenever the set changes.
        let listing_source_product_schemas = match cached_product_schemas {
            Some(existing) => existing,
            None => {
                self.obtain_schemas(
                    listing_source_id,
                    &page.effective_url,
                    request.product_url_pattern.as_deref(),
                    &page.html,
                    request.domain_id.as_ref(),
                    request.mode,
                )
                .await?
            }
        };
        let stored = PageFingerprints::of(
            &page.html,
            &listing_source_product_schemas.product_schemas,
            request.fallback_currency,
        )?;

        if !page.url_moved && stored.match_last_scrape(request) {
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

        let (listing_source_product_schemas, selection) = self
            .select_schema_for_page(
                listing_source_product_schemas,
                FetchedPage {
                    listing_source_id,
                    domain,
                    url: &page.effective_url,
                    html: &page.html,
                    expected_last_captured_raw_input_sha256,
                },
            )
            .await?;
        let scraped = capture_selection(
            request,
            page,
            listing_source_product_schemas.product_schemas,
            selection,
            stored,
        )?;

        // `mark_as_scraped` is intentionally deferred until raw capture succeeds.
        debug!(domain, "Scraping complete");
        Ok(Some(scraped))
    }

    /// Selects the best-ranked plausible cached schema for the page in `page`, or authors a fresh
    /// one when none applies. Returns the schema list the selection used with the selection. A
    /// page that leaves a required value empty where a cached schema found its element fails
    /// with [`ScraperError::MissingRequiredValue`] without waiting on or authoring a schema.
    ///
    /// Scraping continues while a product-schema review is pending, so its approval can replace the
    /// live schemas after `schemas` was read. A page that failed the stale list is therefore
    /// selected once more against the live list before it may author a schema; otherwise it would
    /// open a review whose candidate drops the schemas just approved.
    async fn select_schema_for_page(
        &self,
        mut schemas: ListingSourceProductSchema,
        page: FetchedPage<'_>,
    ) -> Result<(ListingSourceProductSchema, SelectedSchemaCandidate), ScraperError> {
        let mut reread = false;
        loop {
            let reason = match self
                .select_cached_schema(
                    page.listing_source_id,
                    page.url,
                    page.html,
                    &schemas.product_schemas,
                )
                .await
            {
                ExistingSchemaSelection::Selected(selection) => return Ok((schemas, *selection)),
                ExistingSchemaSelection::MissingRequiredValue { findings } => {
                    return Err(ScraperError::MissingRequiredValue {
                        url: page.url.clone(),
                        findings,
                    });
                }
                ExistingSchemaSelection::GenerateNewSchema { reason } => reason,
            };
            let ctx = page.with_existing_schemas(&schemas.product_schemas);
            self.wait_instead_of_authoring(&ctx).await?;
            if !reread
                && let Some(live) = self
                    .product_schemas_changed_since(page.listing_source_id, ctx.existing_schemas)
                    .await?
            {
                debug!(
                    schemas = live.product_schemas.len(),
                    "Live product schemas changed during the scrape; selecting again"
                );
                reread = true;
                schemas = live;
                continue;
            }
            debug!(
                domain = page.domain,
                schemas = schemas.product_schemas.len(),
                fresh_schema_generation_reason = reason.as_str(),
                "Cached selection exhausted; generating new schema"
            );
            let selection = self.generate_fresh_schema_for_page(ctx).await?;
            return Ok((schemas, selection));
        }
    }
}
