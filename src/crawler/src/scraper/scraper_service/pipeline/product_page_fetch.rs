//! Fetching the requested product page: each fetch failure becomes the scrape error and the
//! transport observation that the scrape's outcome reports.

use crate::network::policy::NetworkErrorKind;
use crate::scraper::scraper_service::domain::errors::{
    HttpErrorMetadata, RemovalEvidence, RemovalEvidenceKind, ScraperError,
};
use crate::scraper::scraper_service::domain::product::{
    FetchFailureSource, ScrapeRequest, record_transport_failure, record_transport_success,
};
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml};
use crate::scraper::scraper_service::pipeline::scrape_product::{
    RedirectTarget, is_same_logical_host,
};
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use tracing::debug;
use url::Url;

/// The removal a redirect to `final_url` shows when robots.txt disallows that target, which is
/// judged by its URL alone: the shop's homepage, or another page outside the product URL
/// pattern. `None` for a target on another host or one the pattern takes, which is refused.
fn disallowed_redirect_removal(request: &ScrapeRequest, final_url: &Url) -> Option<ScraperError> {
    let url = &request.url;
    if !is_same_logical_host(url, final_url) {
        return None;
    }
    let (kind, reason) =
        match RedirectTarget::of(url, final_url, request.product_url_pattern.as_deref()) {
            RedirectTarget::Homepage => (RemovalEvidenceKind::RedirectToHomepage, "the homepage"),
            RedirectTarget::OutsideProductPattern => (
                RemovalEvidenceKind::RedirectToDisallowedPage,
                "a page outside its product URL pattern",
            ),
            RedirectTarget::SamePage | RedirectTarget::ProductPage => return None,
        };
    Some(ScraperError::product_listing_removed(
        url.clone(),
        format!(
            "product URL redirected to {reason}, which robots.txt disallows: original={url}, final={final_url}"
        ),
        RemovalEvidence::page(kind, final_url.clone(), None),
        Vec::new(),
    ))
}

impl ScraperServiceImpl {
    /// Fetches the page `request` names on `domain` and records the transport observation. A
    /// 404 or 410, or a redirect on the shop's own host to the homepage or to a page outside the
    /// product URL pattern that robots.txt disallows, fails as a removed listing; every other
    /// fetch failure fails as an HTTP error. A disallowed target is not fetched, so its URL alone
    /// judges it (`docs/crawler/networking.md#redirect-removal-evidence`).
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
            // the redirect leads decides the outcome, as far as its URL shows.
            Err(FetchError::RedirectDisallowed { final_url }) => {
                if let Some(removal) = disallowed_redirect_removal(request, &final_url) {
                    record_transport_success();
                    return Err(removal);
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
                Err(ScraperError::product_listing_removed(
                    url.clone(),
                    details,
                    RemovalEvidence::gone_status(status),
                    Vec::new(),
                ))
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
