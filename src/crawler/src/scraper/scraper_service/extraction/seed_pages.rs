use crate::CrawlerDomainId;
use crate::network::policy::domain_failure_kind;
use crate::scraper::scraper_service::domain::errors::{HttpErrorMetadata, ScraperError};
use crate::scraper::scraper_service::domain::product::{
    FetchFailureSource, ScrapeMode, record_transport_failure,
};
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;
use crate::scraper::scraper_service::service::{FetchError, ScraperServiceImpl};
use listing_source_core::ListingSourceId;
use std::collections::HashSet;
use tracing::warn;
use url::Url;

pub(crate) struct SchemaSeedPage {
    pub(crate) url: Url,
    pub(crate) raw_html: String,
}

impl ScraperServiceImpl {
    /// Fetches up to `schema_seed_pages` HTML pages to use as context when
    /// generating a schema for the first time.  Always includes `primary_html`
    /// as the first entry. URL-scoped failures are best effort, but
    /// circuit-opening transport failures are returned immediately.
    #[tracing::instrument(
        skip(self, primary_html),
        fields(listing_source_id = %listing_source_id, url = %url, schema_seed_pages = self.schema_seed_pages)
    )]
    pub(crate) async fn collect_schema_seed_pages(
        &self,
        listing_source_id: &ListingSourceId,
        domain_id: Option<&CrawlerDomainId>,
        url: &Url,
        product_url_pattern: Option<&str>,
        primary_html: &str,
        mode: ScrapeMode,
    ) -> Result<Vec<SchemaSeedPage>, ScraperError> {
        let mut pages = vec![SchemaSeedPage {
            url: url.clone(),
            raw_html: primary_html.to_string(),
        }];
        if self.schema_seed_pages <= 1
            || matches!(mode, ScrapeMode::DomainProbe | ScrapeMode::PrimaryOnly)
        {
            return Ok(pages);
        }

        let Some(domain_id) = domain_id else {
            return Err(ScraperError::MissingDomainContext {
                url: url.clone(),
                mode: "normal",
            });
        };

        let extra_limit = (self.schema_seed_pages - 1) as i64;
        let sample_urls = match self
            .candidate_service
            .get_random_product_urls_for_schema_seed(listing_source_id, domain_id, url, extra_limit)
            .await
        {
            Ok(urls) => urls,
            Err(err) => {
                warn!(
                    error = ?err,
                    "Failed to load random schema-seed URLs; falling back to current page only"
                );
                return Ok(pages);
            }
        };

        // Keep this exclusion keying aligned with the DB query in
        // `get_random_product_urls_for_schema_seed`: both currently operate on
        // raw URL strings for exclusion. If URL canonicalization is introduced,
        // update both places together to avoid duplicate samples slipping through.
        let mut seen_urls = HashSet::new();
        seen_urls.insert(url.as_str().to_string());
        for seed_candidate in sample_urls {
            if pages.len() >= self.schema_seed_pages {
                break;
            }
            let sample_url = seed_candidate.url;
            let sample_url_key = sample_url.as_str().to_string();
            if !seen_urls.insert(sample_url_key) {
                continue;
            }
            match self.html_fetcher.fetch(&sample_url).await {
                Ok(sample) => {
                    if is_redirect_to_non_product_page(
                        &sample_url,
                        &sample.final_url,
                        product_url_pattern,
                    ) {
                        warn!(
                            sample_url = %sample_url,
                            final_url = %sample.final_url,
                            "Skipping sampled schema-seed page because it redirected to a non-product page"
                        );
                        continue;
                    }
                    pages.push(SchemaSeedPage {
                        url: sample_url.clone(),
                        raw_html: sample.html,
                    });
                }
                Err(err) => {
                    if let Some(error) = domain_failure_error(&sample_url, err.clone()) {
                        record_fetch_transport_error(
                            &sample_url,
                            seed_candidate
                                .expected_last_captured_raw_input_sha256
                                .as_deref(),
                            &err,
                        );
                        return Err(error);
                    }
                    warn!(
                        error = ?err,
                        sample_url = %sample_url,
                        "Failed to fetch sampled schema-seed page; continuing with available samples"
                    );
                }
            }
        }

        Ok(pages)
    }
}

fn record_fetch_transport_error(
    url: &Url,
    expected_last_captured_raw_input_sha256: Option<&[u8]>,
    error: &FetchError,
) {
    match error {
        FetchError::Network { kind, .. } => record_transport_failure(
            url,
            FetchFailureSource::SchemaSeed,
            expected_last_captured_raw_input_sha256,
            *kind,
            None,
            None,
        ),
        FetchError::NetworkWithMetadata {
            kind,
            status,
            retry_after,
            ..
        } => record_transport_failure(
            url,
            FetchFailureSource::SchemaSeed,
            expected_last_captured_raw_input_sha256,
            *kind,
            *status,
            *retry_after,
        ),
    }
}

fn domain_failure_error(url: &Url, error: FetchError) -> Option<ScraperError> {
    match error {
        FetchError::Network { kind, details } if domain_failure_kind(kind).is_some() => {
            Some(ScraperError::HttpError {
                url: url.clone(),
                kind,
                details,
            })
        }
        FetchError::NetworkWithMetadata {
            kind,
            status,
            retry_after,
            details,
        } if domain_failure_kind(kind).is_some() => Some(ScraperError::HttpErrorWithMetadata(
            Box::new(HttpErrorMetadata {
                url: url.clone(),
                kind,
                status_code: status,
                retry_after,
                details,
            }),
        )),
        _ => None,
    }
}
