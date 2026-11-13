//! Fetching the requested product page: each fetch failure becomes the scrape error and the
//! transport observation that the scrape's outcome reports.

use crate::network::policy::NetworkErrorKind;
use crate::scraper::scraper_service::domain::errors::{
    HttpErrorMetadata, RemovalEvidence, RemovalEvidenceKind, ScraperError,
};
use crate::scraper::scraper_service::domain::product::{
    FetchFailureSource, ScrapeRequest, record_product_page_fetch, record_transport_failure,
    record_transport_success,
};
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml};
use crate::scraper::scraper_service::pipeline::scrape_product::{
    RedirectTarget, is_same_logical_host,
};
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use tracing::debug;
use url::Url;

/// The removal a redirect to `final_url` shows when the shop's robots rules refuse that target,
/// which is then judged by its URL alone: the shop's homepage, or another page outside the
/// product URL pattern. `refusal` names the rule in the removal's details. `None` for a target
/// on another host or one the pattern takes, which is refused.
fn disallowed_redirect_removal(
    request: &ScrapeRequest,
    final_url: &Url,
    refusal: &str,
) -> Option<ScraperError> {
    let url = &request.url;
    if !is_same_logical_host(url, final_url) {
        return None;
    }
    let (kind, reason) =
        match RedirectTarget::of(url, final_url, request.product_url_pattern.as_ref()) {
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
            "product URL redirected to {reason}, which {refusal}: original={url}, final={final_url}"
        ),
        RemovalEvidence::page(kind, final_url.clone(), None),
        Vec::new(),
    ))
}

/// The scrape error of a page whose robots directives refused its capture at `final_url`. Only a
/// refusal of the requested page itself, under any spelling of it, is a refusal of the URL,
/// which a stored listing is withdrawn after; a redirect target that refuses capture is judged
/// by its URL alone, as one robots.txt disallows, and is otherwise reported as the refusal of
/// that target (`docs/crawler/networking.md#page-robots-directives`).
fn page_directive_refusal(
    request: &ScrapeRequest,
    final_url: Url,
    details: String,
) -> ScraperError {
    let url = &request.url;
    let kind = NetworkErrorKind::PageDirectiveDisallowed;
    let is_requested_page = is_same_logical_host(url, &final_url)
        && RedirectTarget::of(url, &final_url, request.product_url_pattern.as_ref())
            == RedirectTarget::SamePage;
    if is_requested_page {
        return ScraperError::HttpError {
            url: url.clone(),
            kind,
            details: format!("{details} {final_url}"),
        };
    }
    disallowed_redirect_removal(
        request,
        &final_url,
        "refuses capture by its robots directives",
    )
    .unwrap_or_else(|| ScraperError::HttpError {
        details: format!(
            "product URL redirected to a page whose {details}: original={url}, final={final_url}"
        ),
        url: final_url,
        kind,
    })
}

/// The status the shop answered a product-page fetch with, when it answered with one.
fn answered_status(result: &Result<FetchedHtml, FetchError>) -> Option<u16> {
    match result {
        Ok(fetched) => Some(fetched.status.as_u16()),
        Err(FetchError::Network {
            kind: NetworkErrorKind::HttpStatus(status),
            ..
        }) => Some(*status),
        Err(FetchError::NetworkWithMetadata { kind, status, .. }) => status.or(match kind {
            NetworkErrorKind::HttpStatus(status) => Some(*status),
            _ => None,
        }),
        Err(_) => None,
    }
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
        let fetch_started_at = std::time::Instant::now();
        let fetch_result = self.html_fetcher.fetch(url).await;
        record_product_page_fetch(answered_status(&fetch_result), fetch_started_at.elapsed());
        match fetch_result {
            Ok(fetched) => {
                record_transport_success();
                Ok(fetched)
            }
            // The shop answered with a redirect, so it is responsive; where
            // the redirect leads decides the outcome, as far as its URL shows.
            Err(FetchError::RedirectDisallowed { final_url }) => {
                if let Some(removal) =
                    disallowed_redirect_removal(request, &final_url, "robots.txt disallows")
                {
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
            Err(FetchError::PageDirectiveRefused { final_url, details }) => {
                let kind = NetworkErrorKind::PageDirectiveDisallowed;
                record_transport_failure(
                    url,
                    FetchFailureSource::Primary,
                    expected_last_captured_raw_input_sha256,
                    kind,
                    None,
                    None,
                    self.domain_fetch_stats_for(domain, kind),
                );
                Err(page_directive_refusal(request, final_url, details))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::scraper_service::ScrapeMode;
    use crate::scraper::scraper_service::domain::errors::ListingRemoval;
    use crate::spider::classification::product_url_pattern::ProductUrlPattern;

    fn request(url: &str) -> ScrapeRequest {
        let url = Url::parse(url).unwrap();
        ScrapeRequest {
            listing_source_id: listing_source_core::ListingSourceId::new(),
            source_record_key: url.to_string(),
            last_source_listing_id: None,
            url,
            product_url_pattern: Some(ProductUrlPattern::parse("/product/").unwrap()),
            last_scraped_hash: None,
            last_scraped_schema_fingerprint: None,
            expected_last_captured_raw_input_sha256: None,
            mode: ScrapeMode::Normal,
        }
    }

    fn refusal(requested: &str, refused: &str) -> ScraperError {
        page_directive_refusal(
            &request(requested),
            Url::parse(refused).unwrap(),
            "meta robots tag forbids capture (noindex)".to_owned(),
        )
    }

    fn refused_url(error: &ScraperError) -> &Url {
        match error {
            ScraperError::HttpError {
                url,
                kind: NetworkErrorKind::PageDirectiveDisallowed,
                ..
            } => url,
            other => panic!("expected a page-directive refusal, got {other:?}"),
        }
    }

    /// A refusal of the requested page, under any spelling of it, is the URL's own refusal.
    #[test]
    fn should_report_a_refusal_of_the_requested_page_as_the_urls_own() {
        let requested = "https://shop.example/product/vase";
        for refused in [
            requested,
            "https://shop.example/product/vase/",
            "https://shop.example/en/product/vase",
        ] {
            let error = refusal(requested, refused);

            assert_eq!(refused_url(&error).as_str(), requested, "{refused}");
        }
    }

    /// A redirect target that refuses capture is never the URL's own refusal: its URL alone
    /// shows a removal, as for a target robots.txt disallows, or the refusal names the target.
    #[test]
    fn should_judge_a_refusing_redirect_target_by_its_url() {
        let requested = "https://shop.example/product/vase";

        let homepage = refusal(requested, "https://shop.example/");
        let elsewhere = refusal(requested, "https://shop.example/search?q=vase");
        let other_product = refusal(requested, "https://shop.example/product/bowl");
        let other_host = refusal(requested, "https://other.example/product/vase");

        for (error, evidence) in [
            (homepage, RemovalEvidenceKind::RedirectToHomepage),
            (elsewhere, RemovalEvidenceKind::RedirectToDisallowedPage),
        ] {
            let ScraperError::ProductListingRemoved(removal) = &error else {
                panic!("expected a removal, got {error:?}");
            };
            let ListingRemoval {
                evidence: found,
                details,
                ..
            } = removal.as_ref();
            assert_eq!(found.kind, evidence);
            assert!(
                details.contains("refuses capture by its robots directives"),
                "{details}"
            );
        }
        assert_eq!(
            refused_url(&other_product).as_str(),
            "https://shop.example/product/bowl"
        );
        assert_eq!(
            refused_url(&other_host).as_str(),
            "https://other.example/product/vase"
        );
    }
}
