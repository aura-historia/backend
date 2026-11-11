//! A redirect target many product URLs of one ListingSource share as removal evidence.
//!
//! A shop that sends removed products to one catch-all page, such as
//! `error.aspx?aspxerrorpath=/producto.aspx`, sends many product URLs to the same page, while a
//! live product moved elsewhere, or served under a language segment, leads to a page of its own.
//! The rule is pure and fails closed: a target that only a few URLs share, or that a URL met only
//! once, is no evidence. `docs/crawler/networking.md#redirect-removal-evidence` owns the
//! contract.

use crate::scraper::candidate_service::UnsettledRedirect;
use crate::scraper::scraper_service::domain::errors::{
    ListingRemoval, RemovalEvidence, RemovalEvidenceKind,
};
use crate::scraper::scraper_service::pipeline::scrape_product::is_same_logical_host;
use url::Url;

/// The product URLs of a ListingSource, the scraped one included, that must stand at one redirect
/// target before the target is taken for a catch-all page: well above the handful a few
/// coincidental redirects, or a moved category, reach.
pub const SHARED_REDIRECT_MIN_URLS: u64 = 10;
/// The consecutive scrapes, at least a scrape interval apart, that must end at the target before
/// the URL is withdrawn, so a redirect that lasts a day, such as a maintenance page, withdraws
/// nothing.
pub const SHARED_REDIRECT_MIN_STREAK: u32 = 2;

/// The other product URLs that must stand at the target besides the scraped one.
pub const fn other_urls_needed() -> u64 {
    SHARED_REDIRECT_MIN_URLS - 1
}

/// Whether the URL's own history makes the target worth counting: it met the target on
/// [`SHARED_REDIRECT_MIN_STREAK`] consecutive scrapes, or its last removal was already captured on
/// it, so a recheck of that removal keeps it.
pub fn url_keeps_redirecting_to(
    redirect: &UnsettledRedirect,
    removed_by_redirect_target: Option<&Url>,
) -> bool {
    redirect.streak >= SHARED_REDIRECT_MIN_STREAK
        || removed_by_redirect_target == Some(&redirect.destination)
}

/// The removal of `url` that its redirect shows, given how many other product URLs of its
/// ListingSource stand at the same target (`other_urls`), or `None` when that is no evidence. A
/// target on another host is never evidence, as for any redirect removal.
pub fn shared_redirect_removal(
    url: &Url,
    redirect: &UnsettledRedirect,
    removed_by_redirect_target: Option<&Url>,
    other_urls: u64,
) -> Option<ListingRemoval> {
    (is_same_logical_host(url, &redirect.destination)
        && url_keeps_redirecting_to(redirect, removed_by_redirect_target)
        && other_urls >= other_urls_needed())
    .then(|| ListingRemoval {
        url: url.clone(),
        details: format!(
            "product URL keeps redirecting outside its product URL pattern to a page at least {other_urls} other product URLs of its ListingSource redirect to: original={url}, final={}",
            redirect.destination
        ),
        evidence: RemovalEvidence::page(
            RemovalEvidenceKind::RedirectToSharedPage,
            redirect.destination.clone(),
            None,
        ),
        removed_by_templates: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).unwrap()
    }

    fn error_page() -> Url {
        url("https://shop.example/error.aspx?aspxerrorpath=/producto.aspx")
    }

    fn redirect(streak: u32) -> UnsettledRedirect {
        UnsettledRedirect {
            destination: error_page(),
            streak,
        }
    }

    #[test]
    fn should_withdraw_a_url_that_keeps_redirecting_to_a_page_many_urls_share() {
        let product = url("https://shop.example/producto.aspx?id=589");

        let removal = shared_redirect_removal(&product, &redirect(2), None, 9).unwrap();

        assert_eq!(removal.url, product);
        assert_eq!(
            removal.evidence.kind,
            RemovalEvidenceKind::RedirectToSharedPage
        );
        assert_eq!(removal.evidence.final_url, Some(error_page()));
        assert_eq!(removal.removed_by_redirect_target(), Some(&error_page()));
        assert!(removal.removed_by_templates.is_empty());
    }

    #[test]
    fn should_withdraw_nothing_when_few_urls_share_the_target() {
        let product = url("https://shop.example/producto.aspx?id=589");

        for other_urls in [0, 1, 8] {
            assert!(
                shared_redirect_removal(&product, &redirect(5), None, other_urls).is_none(),
                "{other_urls} other URLs"
            );
        }
    }

    #[test]
    fn should_withdraw_nothing_when_the_target_lies_on_another_host() {
        let product = url("https://shop.example/producto.aspx?id=589");
        let elsewhere = UnsettledRedirect {
            destination: url("https://other.example/error.aspx"),
            streak: 5,
        };

        assert!(shared_redirect_removal(&product, &elsewhere, None, 500).is_none());
    }

    #[test]
    fn should_withdraw_nothing_on_the_first_scrape_that_meets_the_target() {
        let product = url("https://shop.example/producto.aspx?id=589");

        assert!(shared_redirect_removal(&product, &redirect(1), None, 500).is_none());
    }

    #[test]
    fn should_keep_a_removal_captured_on_the_same_target_when_it_is_rechecked() {
        let product = url("https://shop.example/producto.aspx?id=589");

        assert!(shared_redirect_removal(&product, &redirect(1), Some(&error_page()), 9).is_some());
        assert!(
            shared_redirect_removal(
                &product,
                &redirect(1),
                Some(&url("https://shop.example/vendidos")),
                9
            )
            .is_none()
        );
    }
}
