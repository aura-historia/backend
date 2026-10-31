//! Fetching the requested product page: each fetch failure becomes the scrape error and the
//! transport observation that the scrape's outcome reports.

use crate::network::policy::NetworkErrorKind;
use crate::scraper::scraper_service::domain::errors::{HttpErrorMetadata, ScraperError};
use crate::scraper::scraper_service::domain::product::{
    FetchFailureSource, ScrapeRequest, record_transport_failure, record_transport_success,
};
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml};
use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use tracing::debug;

impl ScraperServiceImpl {
    /// Fetches the page `request` names on `domain` and records the transport observation. A
    /// 404 or 410, or a robots-disallowed redirect to a non-product page, fails as a removed
    /// listing; every other fetch failure fails as an HTTP error.
    pub(super) async fn fetch_product_page(
        &self,
        request: &ScrapeRequest,
        domain: &str,
    ) -> Result<FetchedHtml, ScraperError> {
        let url = &request.url;
        let expected_last_captured_raw_input_sha256 =
            request.expected_last_captured_raw_input_sha256.as_deref();
        debug!(domain, "Fetching product page HTML");
        match self.html_fetcher.fetch(url).await {
            Ok(fetched) => {
                record_transport_success();
                Ok(fetched)
            }
            // The shop answered with a redirect, so it is responsive; where
            // the redirect leads decides the outcome, as for a fetched target.
            Err(FetchError::RedirectDisallowed { final_url }) => {
                if is_redirect_to_non_product_page(
                    url,
                    &final_url,
                    request.product_url_pattern.as_deref(),
                ) {
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
                    None,
                );
                record_transport_success();
                Err(ScraperError::HttpError {
                    url: url.clone(),
                    kind: NetworkErrorKind::RobotsDisallowed,
                    details: format!(
                        "product URL redirected to a URL robots.txt disallows: final={final_url}"
                    ),
                })
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
                    None,
                );
                Err(ScraperError::ProductListingRemoved {
                    url: url.clone(),
                    details,
                })
            }
            Err(FetchError::Network { kind, details }) => {
                record_transport_failure(
                    url,
                    FetchFailureSource::Primary,
                    expected_last_captured_raw_input_sha256,
                    kind,
                    None,
                    None,
                    self.domain_fetch_stats_for(domain, kind),
                );
                Err(ScraperError::HttpError {
                    url: url.clone(),
                    kind,
                    details,
                })
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
                    self.domain_fetch_stats_for(domain, kind),
                );
                Err(ScraperError::HttpErrorWithMetadata(Box::new(
                    HttpErrorMetadata {
                        url: url.clone(),
                        kind,
                        status_code: status,
                        retry_after,
                        details,
                    },
                )))
            }
        }
    }
}
