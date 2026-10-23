use crate::network::policy::NetworkErrorKind;

use crate::scraper::css_selector::product_schema::ListingSourceProductSchema;
use crate::scraper::css_selector::removed_page_schema::{RemovedPageMatch, RemovedPageSchema};
use crate::scraper::raw_input::{capture_source_listing_id, crawler_raw_input};
use crate::scraper::scraper_service::domain::errors::{HttpErrorMetadata, ScraperError};
use crate::scraper::scraper_service::domain::product::{
    DomainFetchHealth, FetchFailureSource, ScrapeMode, ScrapeOutcome, ScrapeRequest,
    ScrapedProduct, ScraperService, begin_transport_observation, current_fetch_failure,
    current_transport_observation, domain_health_for_scraper_error, record_transport_failure,
    record_transport_success, with_scrape_observation,
};
use crate::scraper::scraper_service::html_fetcher::FetchError;
use crate::scraper::scraper_service::pipeline::cached_schema_selection::{
    ExistingSchemaSelection, apply_cached_schemas,
};
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use crate::scraper::scraper_service::util::hash::{
    fingerprint_scraper_context, hash_html, hash_main_fragment,
};
use crate::scraper::scraper_service::util::html::extract_main_fragment;
use crate::spider::classification::url_metadata::CrawlerUrlWriteOutcome;
use crate::spider::utils::url::CrawledUrl;
use listing_source_core::ListingSourceId;
use regex::Regex;
use tracing::{debug, warn};
use url::Url;

pub(crate) fn is_redirect_to_non_product_page(
    original_url: &Url,
    final_url: &Url,
    product_url_pattern: Option<&str>,
) -> bool {
    let normalized_original = CrawledUrl::new(original_url.clone());
    let normalized_final = CrawledUrl::new(final_url.clone());

    if normalized_original.as_url() == normalized_final.as_url() {
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

/// What the stored removed-page schemas say about a fetched page.
enum RemovedPageVerdict {
    /// No stored schema matches the page.
    NotRemoved,
    /// A stored schema matches and no cached product schema still extracts this
    /// listing's product from the page.
    Removed,
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
fn removed_page_verdict(
    removed_page_schemas: Vec<RemovedPageSchema>,
    cached: Option<&ListingSourceProductSchema>,
    html: &str,
    source_record_key: &str,
    last_source_listing_id: Option<&str>,
) -> RemovedPageVerdict {
    if removed_page_schemas.is_empty() {
        return RemovedPageVerdict::NotRemoved;
    }
    let parsed = scraper::Html::parse_document(html);
    let matches = removed_page_schemas
        .into_iter()
        .filter_map(|schema| schema.match_page(&parsed))
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return RemovedPageVerdict::NotRemoved;
    }

    let product_still_extracts = cached.is_some_and(|cached| {
        apply_cached_schemas(&cached.product_schemas, &parsed)
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
            })
    });
    if product_still_extracts {
        RemovedPageVerdict::ProductStillExtracts { matches }
    } else {
        RemovedPageVerdict::Removed
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

impl ScraperServiceImpl {
    async fn scrape_request(
        &self,
        request: &ScrapeRequest,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let listing_source_id = &request.listing_source_id;
        let url = &request.url;
        let source_record_key = &request.source_record_key;
        let last_source_listing_id = request.last_source_listing_id.as_deref();
        let product_url_pattern = request.product_url_pattern.as_deref();
        let last_scraped_hash = request.last_scraped_hash.as_deref();
        let last_scraped_schema_fingerprint = request.last_scraped_schema_fingerprint.as_deref();
        let expected_last_captured_raw_input_sha256 =
            request.expected_last_captured_raw_input_sha256.as_deref();
        let fallback_currency = request.fallback_currency;
        let domain_id = request.domain_id;
        let mode = request.mode;

        begin_transport_observation();
        let domain = url
            .host_str()
            .ok_or_else(|| ScraperError::NoHost { url: url.clone() })?;

        // 1. Fetch HTML --------------------------------------------------
        debug!(domain, "Fetching product page HTML");
        let fetched = match self.html_fetcher.fetch(url).await {
            Ok(fetched) => {
                record_transport_success();
                fetched
            }
            // The shop answered with a redirect, so it is responsive; where
            // the redirect leads decides the outcome, as for a fetched target.
            Err(FetchError::RedirectDisallowed { final_url }) => {
                if is_redirect_to_non_product_page(url, &final_url, product_url_pattern) {
                    record_transport_success();
                    return Err(ScraperError::ProductListingRemoved {
                        url: url.clone(),
                        details: format!(
                            "product URL redirected to a non-product page robots.txt disallows: original={url}, final={final_url}"
                        ),
                    });
                }
                record_transport_failure(
                    url,
                    FetchFailureSource::Primary,
                    expected_last_captured_raw_input_sha256,
                    NetworkErrorKind::RobotsDisallowed,
                    None,
                    None,
                );
                record_transport_success();
                return Err(ScraperError::HttpError {
                    url: url.clone(),
                    kind: NetworkErrorKind::RobotsDisallowed,
                    details: format!(
                        "product URL redirected to a URL robots.txt disallows: final={final_url}"
                    ),
                });
            }
            Err(FetchError::Network {
                kind: NetworkErrorKind::HttpStatus(status @ (404 | 410)),
                details,
            })
            | Err(FetchError::NetworkWithMetadata {
                kind: NetworkErrorKind::HttpStatus(status @ (404 | 410)),
                details,
                ..
            }) => {
                record_transport_failure(
                    url,
                    FetchFailureSource::Primary,
                    expected_last_captured_raw_input_sha256,
                    NetworkErrorKind::HttpStatus(status),
                    Some(status),
                    None,
                );
                return Err(ScraperError::ProductListingRemoved {
                    url: url.clone(),
                    details,
                });
            }
            Err(FetchError::Network { kind, details }) => {
                record_transport_failure(
                    url,
                    FetchFailureSource::Primary,
                    expected_last_captured_raw_input_sha256,
                    kind,
                    None,
                    None,
                );
                return Err(ScraperError::HttpError {
                    url: url.clone(),
                    kind,
                    details,
                });
            }
            Err(FetchError::NetworkWithMetadata {
                kind,
                status,
                retry_after,
                details,
            }) => {
                record_transport_failure(
                    url,
                    FetchFailureSource::Primary,
                    expected_last_captured_raw_input_sha256,
                    kind,
                    status,
                    retry_after,
                );
                return Err(ScraperError::HttpErrorWithMetadata(Box::new(
                    HttpErrorMetadata {
                        url: url.clone(),
                        kind,
                        status_code: status,
                        retry_after,
                        details,
                    },
                )));
            }
        };
        let acquired_at = time::OffsetDateTime::now_utc();
        let normalized_original_url = CrawledUrl::new(url.clone());
        let normalized_final_url = CrawledUrl::new(fetched.final_url.clone());
        let url_moved = normalized_original_url.as_url() != normalized_final_url.as_url();
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
        let html = fetched.html;

        let removed_page_schemas = self.removed_page_schemas_for(listing_source_id).await?;
        // One schema-cache read serves both the removal guard and schema obtainment.
        let cached_product_schemas = self
            .schema_service
            .find_product_schema(listing_source_id)
            .await?;
        // Withdrawal needs unambiguous evidence: a removed-page template that also
        // matches a page a cached schema still extracts this listing from is an
        // over-broad stored template. Product evidence wins and the page is scraped
        // normally; the warning names the row an operator has to fix.
        match removed_page_verdict(
            removed_page_schemas,
            cached_product_schemas.as_ref(),
            &html,
            source_record_key,
            last_source_listing_id,
        ) {
            RemovedPageVerdict::NotRemoved => {}
            RemovedPageVerdict::Removed => {
                return Err(ScraperError::ProductListingRemoved {
                    url: url.clone(),
                    details: "soft-404 removed page matched configured removed-page schema"
                        .to_string(),
                });
            }
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
                    listing_source_id = %listing_source_id,
                    url = %url,
                    removed_page_selectors = %selectors,
                    removed_page_notice_count = notice_count,
                    "Stored removed-page schema matched a page a cached product schema still extracts; scraping it as a product"
                );
            }
        }

        let has_main = extract_main_fragment(&html).is_some();
        let current_hash = hash_main_fragment(&html).unwrap_or_else(|| hash_html(&html));

        // Obtain the effective schema set before the fast path. Selector or raw-attribute
        // changes must force extraction even when the page fragment is byte-identical.
        let listing_source_product_schemas = match cached_product_schemas {
            Some(existing) => existing,
            None => {
                self.obtain_schemas(
                    listing_source_id,
                    &effective_url,
                    product_url_pattern,
                    &html,
                    domain_id.as_ref(),
                    mode,
                )
                .await?
            }
        };
        let stored_schema_fingerprint = fingerprint_scraper_context(
            &listing_source_product_schemas.product_schemas,
            fallback_currency,
        )
        .map_err(ScraperError::SchemaFingerprint)?;

        if !url_moved
            && has_main
            && last_scraped_hash == Some(current_hash.as_str())
            && last_scraped_schema_fingerprint == Some(stored_schema_fingerprint.as_str())
        {
            debug!("Page and schema fingerprints match; skipping extraction.");
            match self
                .candidate_service
                .touch_scraped(
                    listing_source_id,
                    url,
                    &current_hash,
                    &stored_schema_fingerprint,
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
            return Ok(None);
        }

        // Select the best-ranked plausible cached schema.
        let selection = match self
            .select_cached_schema(
                listing_source_id,
                &effective_url,
                &html,
                &listing_source_product_schemas.product_schemas,
            )
            .await
        {
            ExistingSchemaSelection::Selected(selection) => *selection,
            ExistingSchemaSelection::GenerateNewSchema { reason } => {
                debug!(
                    domain,
                    schemas = listing_source_product_schemas.product_schemas.len(),
                    fresh_schema_generation_reason = reason.as_str(),
                    "Cached selection exhausted; generating new schema"
                );
                self.generate_fresh_schema_for_page(FreshSchemaGenerationContext {
                    listing_source_id,
                    domain,
                    url: &effective_url,
                    html: &html,
                    existing_schemas: &listing_source_product_schemas.product_schemas,
                    expected_last_captured_raw_input_sha256,
                })
                .await?
            }
        };

        let mut effective_schemas = listing_source_product_schemas.product_schemas;
        if selection.fresh_schema {
            effective_schemas.push(selection.schema.clone());
        }
        let schema_fingerprint = fingerprint_scraper_context(&effective_schemas, fallback_currency)
            .map_err(ScraperError::SchemaFingerprint)?;
        let source_listing_id =
            capture_source_listing_id(&selection.raw.source_listing_id, source_record_key)?;
        let raw_input = crawler_raw_input(
            &selection.raw,
            &selection.validated_image_urls,
            &effective_url,
            source_listing_id.as_ref(),
            fallback_currency,
        )
        .map_err(ScraperError::RawNormalizationInput)?;
        let raw_input_sha256 = raw_input
            .hash()
            .map_err(ScraperError::RawNormalizationInput)?
            .as_bytes()
            .to_vec();

        // `mark_as_scraped` is intentionally deferred until raw capture succeeds.
        debug!(domain, "Scraping complete");
        Ok(Some(ScrapedProduct {
            acquired_at,
            raw_input,
            effective_url,
            source_listing_id: source_listing_id.to_string(),
            hash: current_hash,
            schema_fingerprint,
            raw_input_sha256,
        }))
    }
}
