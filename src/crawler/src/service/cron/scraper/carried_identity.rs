//! What a scrape does when its page shows an identity another product URL of its ListingSource
//! carries (`docs/crawler/scheduling-and-retries.md#captured-identity`): it retires the URL as
//! that URL's duplicate only when [`same_product_evidence`] shows one product, and otherwise
//! parks it in an identity conflict, so it neither captures another product's content under its
//! own listing nor retires a different product that shares a reused identity.

use super::identity_claims::IdentityClaim;
use super::{
    ScrapeBatchContext, ScrapeCandidateOutcome, errored_outcome, persist_identity_conflict,
    retire_duplicate_of_known_identity,
};
use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::same_product::{
    ProductUrlIdentity, SameProductEvidence, same_product_evidence,
};
use crate::scraper::scraper_service::ScrapedProduct;
use tracing::{debug, info, warn};

/// The event of a scrape whose page shows an identity another product URL carries.
pub(super) const SHOWS_CARRIED_IDENTITY_EVENT: &str = "crawler.product_url_shows_carried_identity";

/// What the scrape does with the identity its page shows.
pub(super) enum CarriedIdentity {
    /// The scrape captures, holding the batch's claim of the identity on a first scrape.
    Capture(Option<IdentityClaim>),
    /// The scrape ended: the URL was retired as a duplicate, parked in a conflict, or the
    /// lookup failed.
    Settled(Box<ScrapeCandidateOutcome>),
}

/// Decides whether the scrape of `candidate` captures, and settles it otherwise:
/// - a first scrape (no captured identity) whose identity another URL carries, or claimed
///   earlier in the batch, is retired as that URL's duplicate with evidence of one product and
///   parked in a conflict without; otherwise it captures and claims the identity;
/// - an unredirected scrape of a captured URL captures under its captured identity, unless its
///   page shows another identity that another URL carries, or that another URL claimed earlier in
///   the batch: then it is retired with evidence of one product and parked in a conflict without;
/// - a redirected scrape keeps its own identity, which the redirect's conflict check judged.
///
/// It looks the carrier up at most once.
pub(super) async fn settle_carried_identity(
    candidate: &ScraperCandidate,
    ctx: &ScrapeBatchContext,
    scraped: &ScrapedProduct,
) -> CarriedIdentity {
    match candidate.last_source_listing_id.as_deref() {
        None => settle_first_scrape(candidate, ctx, scraped).await,
        Some(captured) if candidate.url == scraped.effective_url => {
            settle_captured_url(candidate, ctx, captured, scraped).await
        }
        Some(_) => CarriedIdentity::Capture(None),
    }
}

async fn settle_first_scrape(
    candidate: &ScraperCandidate,
    ctx: &ScrapeBatchContext,
    scraped: &ScrapedProduct,
) -> CarriedIdentity {
    let identity = scraped.source_listing_id.as_str();
    let page = ProductUrlIdentity {
        url: &candidate.url,
        captured_identity: None,
        title: scraped.title.as_deref(),
    };
    let carrier = match ctx
        .scraper_candidates
        .identity_carrier(&candidate.listing_source_id, identity, &candidate.url)
        .await
    {
        Ok(Some(carrier)) => carrier,
        Ok(None) => {
            return match ctx.claimed_identities.claim(
                identity,
                &candidate.url,
                scraped.title.as_deref(),
            ) {
                Ok(claim) => CarriedIdentity::Capture(Some(claim)),
                Err(claimant) => CarriedIdentity::Settled(Box::new(
                    settle_shown_identity(
                        candidate,
                        ctx,
                        page,
                        identity,
                        &claimant.url,
                        claimant.title.as_deref(),
                    )
                    .await,
                )),
            };
        }
        Err(error) => return lookup_failed(candidate, &error),
    };
    CarriedIdentity::Settled(Box::new(
        settle_shown_identity(
            candidate,
            ctx,
            page,
            identity,
            &carrier.url,
            carrier.title.as_deref(),
        )
        .await,
    ))
}

async fn settle_captured_url(
    candidate: &ScraperCandidate,
    ctx: &ScrapeBatchContext,
    captured: &str,
    scraped: &ScrapedProduct,
) -> CarriedIdentity {
    let Some(page_identity) = scraped
        .extracted_source_listing_id
        .as_deref()
        .filter(|identity| *identity != captured)
    else {
        return CarriedIdentity::Capture(None);
    };
    let carrier = match ctx
        .scraper_candidates
        .identity_carrier(&candidate.listing_source_id, page_identity, &candidate.url)
        .await
    {
        Ok(Some(carrier)) => Some((carrier.url, carrier.title)),
        Ok(None) => ctx
            .claimed_identities
            .claimant(page_identity, &candidate.url)
            .map(|claimant| (claimant.url, claimant.title)),
        Err(error) => return lookup_failed(candidate, &error),
    };
    let Some((carrier_url, carrier_title)) = carrier else {
        return CarriedIdentity::Capture(None);
    };
    CarriedIdentity::Settled(Box::new(
        settle_shown_identity(
            candidate,
            ctx,
            ProductUrlIdentity {
                url: &candidate.url,
                captured_identity: Some(captured),
                title: candidate.last_captured_title.as_deref(),
            },
            page_identity,
            &carrier_url,
            carrier_title.as_deref(),
        )
        .await,
    ))
}

/// Retires `candidate` as the duplicate of `carrier_url` with evidence of one product, and parks
/// it in an identity conflict without.
async fn settle_shown_identity(
    candidate: &ScraperCandidate,
    ctx: &ScrapeBatchContext,
    page: ProductUrlIdentity<'_>,
    page_identity: &str,
    carrier_url: &url::Url,
    carrier_title: Option<&str>,
) -> ScrapeCandidateOutcome {
    let evidence = same_product_evidence(
        page,
        page_identity,
        ProductUrlIdentity {
            url: carrier_url,
            captured_identity: Some(page_identity),
            title: carrier_title,
        },
    );
    log_shown_identity(
        candidate,
        page.captured_identity,
        page_identity,
        carrier_url,
        evidence,
    );
    match evidence {
        Some(_) => {
            retire_duplicate_of_known_identity(candidate, ctx, page_identity, carrier_url).await
        }
        None => {
            persist_identity_conflict(
                candidate,
                ctx,
                &known_product_conflict_message(page.captured_identity, page_identity, carrier_url),
            )
            .await
        }
    }
}

/// The conflict message of a page that shows another known product.
fn known_product_conflict_message(
    captured: Option<&str>,
    page_identity: &str,
    carrier_url: &url::Url,
) -> String {
    let captured = captured.map_or_else(String::new, |captured| {
        format!(", not the product captured here as `{captured}`")
    });
    format!(
        "the page shows another known product: source listing id `{page_identity}`, which \
         {carrier_url} carries{captured}"
    )
}

/// Logs the decision on a page that shows an identity another URL carries: at INFO for a URL
/// that carries an identity of its own, which may have another product now, and for every
/// retirement, which nothing undoes; at DEBUG for a first scrape parked in a conflict.
fn log_shown_identity(
    candidate: &ScraperCandidate,
    captured: Option<&str>,
    page_identity: &str,
    carrier_url: &url::Url,
    evidence: Option<SameProductEvidence>,
) {
    let evidence_name = evidence.map_or("none", SameProductEvidence::as_str);
    let outcome = if evidence.is_some() {
        "retired_duplicate"
    } else {
        "identity_conflict"
    };
    let captured_identity = captured.unwrap_or_default();
    if captured.is_some() || evidence.is_some() {
        info!(
            event = SHOWS_CARRIED_IDENTITY_EVENT,
            listing_source_id = %candidate.listing_source_id,
            url = %candidate.url,
            captured_identity,
            page_identity,
            carrier_url = %carrier_url,
            evidence = evidence_name,
            outcome,
            "A product page shows an identity another product URL of its ListingSource carries"
        );
    } else {
        debug!(
            event = SHOWS_CARRIED_IDENTITY_EVENT,
            listing_source_id = %candidate.listing_source_id,
            url = %candidate.url,
            captured_identity,
            page_identity,
            carrier_url = %carrier_url,
            evidence = evidence_name,
            outcome,
            "A product page shows an identity another product URL of its ListingSource carries"
        );
    }
}

fn lookup_failed(candidate: &ScraperCandidate, error: &sqlx::Error) -> CarriedIdentity {
    warn!(
        error = %error,
        listing_source_id = %candidate.listing_source_id,
        url = %candidate.url,
        "Failed to look up the product URL carrying a scraped source listing id"
    );
    CarriedIdentity::Settled(Box::new(errored_outcome("SourceListingIdLookup")))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{scrape_candidate, scrape_candidate_context, scrape_outcome};
    use super::super::{RawCaptureSuccessAction, SOURCE_LISTING_ID_CONFLICT_ERROR_KIND};
    use super::*;
    use crate::scraper::candidate_service::ScraperCandidate;
    use crate::scraper::candidate_service::{IdentityCarrier, MockScraperCandidateService};
    use crate::scraper::raw_input::crawler_verified_removal_input;
    use crate::scraper::scraper_service::{MockScraperService, ScrapedProduct};
    use crate::service::cron::test_support::scraper_candidate;
    use crate::spider::classification::url_metadata::{CrawlerDisposition, CrawlerUrlWriteOutcome};

    const IDENTITY: &str = "source-listing";
    const GILLMARK_PAGE: &str =
        "https://www.gillmark.com/book/hertfordshire-countryside-238/page-14/";
    const GILLMARK_CARRIER: &str =
        "https://www.gillmark.com/book/hertfordshire-countryside-238/june-1971-5169/";

    fn url(raw: &str) -> url::Url {
        url::Url::parse(raw).unwrap()
    }

    fn scraped_product(
        effective_url: url::Url,
        source_listing_id: &str,
        extracted: Option<&str>,
        title: Option<&str>,
    ) -> ScrapedProduct {
        ScrapedProduct {
            acquired_at: time::OffsetDateTime::now_utc(),
            raw_input: crawler_verified_removal_input(&effective_url)
                .unwrap_or_else(|error| panic!("test raw input: {error}")),
            hash: "hash".to_owned(),
            schema_fingerprint: "schema".to_owned(),
            raw_input_sha256: vec![7; 32],
            effective_url,
            source_listing_id: source_listing_id.to_owned(),
            extracted_source_listing_id: extracted.map(str::to_owned),
            title: title.map(str::to_owned),
        }
    }

    /// A scraper whose every scrape answers `scraped` at the requested URL.
    fn scraping(
        source_listing_id: &str,
        extracted: Option<&str>,
        title: Option<&str>,
    ) -> MockScraperService {
        let (source_listing_id, extracted, title) = (
            source_listing_id.to_owned(),
            extracted.map(str::to_owned),
            title.map(str::to_owned),
        );
        let mut scraper_service = MockScraperService::new();
        scraper_service.expect_scrape().returning(move |request| {
            let scraped = scraped_product(
                request.url.clone(),
                &source_listing_id,
                extracted.as_deref(),
                title.as_deref(),
            );
            Box::pin(async move { scrape_outcome(Ok(Some(scraped))) })
        });
        scraper_service
    }

    fn carrying(
        scraper_candidates: &mut MockScraperCandidateService,
        identity: &'static str,
        carrier: Option<(&str, Option<&str>)>,
    ) {
        let carrier = carrier.map(|(carrier_url, title)| IdentityCarrier {
            url: url(carrier_url),
            title: title.map(str::to_owned),
        });
        scraper_candidates
            .expect_identity_carrier()
            .once()
            .withf(move |_, looked_up, _| looked_up == identity)
            .returning(move |_, _, _| {
                let carrier = carrier.clone();
                Box::pin(async move { Ok(carrier) })
            });
    }

    fn expect_retired(scraper_candidates: &mut MockScraperCandidateService, retired: &url::Url) {
        let retired = retired.clone();
        scraper_candidates
            .expect_set_disposition()
            .once()
            .withf(move |_, url, disposition, _| {
                *url == retired && *disposition == CrawlerDisposition::Duplicate
            })
            .returning(|_, _, _, _| Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) }));
    }

    fn expect_conflict(
        scraper_candidates: &mut MockScraperCandidateService,
        conflicted: &url::Url,
    ) {
        let conflicted = conflicted.clone();
        scraper_candidates
            .expect_mark_fetch_failure()
            .once()
            .withf(move |_, url, kind, message, _, _, _| {
                *url == conflicted
                    && kind == SOURCE_LISTING_ID_CONFLICT_ERROR_KIND
                    && message.contains("another known product")
            })
            .returning(|_, _, _, _, _, _, _| {
                Box::pin(async { Ok(CrawlerUrlWriteOutcome::Applied) })
            });
    }

    /// A captured URL: listing captured as `captured` under the title `title`.
    fn captured_candidate(page: &str, captured: &str, title: Option<&str>) -> ScraperCandidate {
        let mut candidate = scraper_candidate("ListingSource", url(page));
        candidate.last_source_listing_id = Some(captured.to_owned());
        candidate.last_extracted_source_listing_id = Some(captured.to_owned());
        candidate.last_captured_title = title.map(str::to_owned);
        candidate
    }

    fn captured_identity(outcome: &super::ScrapeCandidateOutcome) -> &str {
        match &outcome
            .capture
            .as_ref()
            .expect("the page is captured")
            .on_success
        {
            RawCaptureSuccessAction::MarkScraped(meta) => meta.source_listing_id.as_str(),
            _ => panic!("a product page is marked scraped after capture"),
        }
    }

    mod first_scrape {
        use super::*;

        #[tokio::test]
        async fn should_retire_a_spelling_of_the_product_another_url_carries() {
            let page = url("https://example.com/view.php?p_idx=7&page=2");
            let mut scraper_candidates = MockScraperCandidateService::new();
            carrying(
                &mut scraper_candidates,
                IDENTITY,
                Some((
                    "https://example.com/view.php?p_idx=7",
                    Some("Another title"),
                )),
            );
            expect_retired(&mut scraper_candidates, &page);
            scraper_candidates.expect_mark_as_scraped().never();
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping(IDENTITY, Some(IDENTITY), Some("Chair")),
            );

            let outcome = scrape_candidate(scraper_candidate("ListingSource", page), &ctx).await;

            assert!(outcome.capture.is_none());
            assert!(outcome.skipped && !outcome.errored);
        }

        /// A new URL whose page shows a reused stock code another product's URL carries is no
        /// spelling of that product: it is parked, neither captured nor retired.
        #[tokio::test]
        async fn should_park_a_page_of_another_product_under_a_carried_identity_in_a_conflict() {
            let page = url("https://haresfur.example/products/stig-lindberg-faience-dish");
            let mut scraper_candidates = MockScraperCandidateService::new();
            carrying(
                &mut scraper_candidates,
                "C327",
                Some((
                    "https://haresfur.example/products/rolf-palm-stoneware-vase",
                    Some("Rolf Palm stoneware vase"),
                )),
            );
            scraper_candidates.expect_set_disposition().never();
            expect_conflict(&mut scraper_candidates, &page);
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("C327", Some("C327"), Some("Stig Lindberg Faience dish")),
            );

            let outcome = scrape_candidate(scraper_candidate("ListingSource", page), &ctx).await;

            assert!(outcome.capture.is_none());
            assert_eq!(
                outcome.failure_kind.as_deref(),
                Some(SOURCE_LISTING_ID_CONFLICT_ERROR_KIND)
            );
        }

        #[tokio::test]
        async fn should_capture_and_claim_an_identity_no_other_url_carries() {
            let page = url("https://example.com/view.php?p_idx=7");
            let mut scraper_candidates = MockScraperCandidateService::new();
            carrying(&mut scraper_candidates, IDENTITY, None);
            scraper_candidates.expect_set_disposition().never();
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping(IDENTITY, Some(IDENTITY), Some("Chair")),
            );

            let outcome = scrape_candidate(scraper_candidate("ListingSource", page), &ctx).await;

            assert_eq!(captured_identity(&outcome), IDENTITY);
        }

        #[tokio::test]
        async fn should_report_an_error_without_a_capture_when_the_identity_lookup_fails() {
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_identity_carrier()
                .returning(|_, _, _| Box::pin(async { Err(sqlx::Error::PoolTimedOut) }));
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping(IDENTITY, Some(IDENTITY), Some("Chair")),
            );

            let outcome = scrape_candidate(
                scraper_candidate("ListingSource", url("https://example.com/view.php?p_idx=7")),
                &ctx,
            )
            .await;

            assert!(outcome.errored);
            assert!(outcome.capture.is_none());
        }
    }

    /// Two spellings of one product scraped in one batch, as Loveday's `/product/223/…-table/`
    /// and `…-table-1/` were: the collector has not pushed the first capture yet, so the database
    /// knows neither identity, and only the batch's claim retires the second, while the first's
    /// capture may still become durable.
    mod batch_claim {
        use super::*;

        fn spellings() -> (url::Url, url::Url) {
            (
                url("https://example.com/product/223/table/"),
                url("https://example.com/product/223/table-1/"),
            )
        }

        fn without_carrier(times: usize) -> MockScraperCandidateService {
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_identity_carrier()
                .times(times)
                .returning(|_, _, _| Box::pin(async { Ok(None) }));
            scraper_candidates
        }

        #[tokio::test]
        async fn should_retire_the_second_spelling_while_the_first_capture_may_become_durable() {
            let (first, second) = spellings();
            let mut scraper_candidates = without_carrier(2);
            expect_retired(&mut scraper_candidates, &second);
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("#2798", Some("#2798"), Some("Table")),
            );

            let first_outcome =
                scrape_candidate(scraper_candidate("ListingSource", first), &ctx).await;
            let second_outcome =
                scrape_candidate(scraper_candidate("ListingSource", second), &ctx).await;

            assert!(first_outcome.capture.is_some());
            assert!(second_outcome.capture.is_none());
            assert!(second_outcome.skipped && !second_outcome.errored);
        }

        /// The collector applied the first capture: the claim stays for the rest of the batch.
        #[tokio::test]
        async fn should_retire_the_second_spelling_after_the_first_capture_became_durable() {
            let (first, second) = spellings();
            let mut scraper_candidates = without_carrier(2);
            expect_retired(&mut scraper_candidates, &second);
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("#2798", Some("#2798"), Some("Table")),
            );

            let first_outcome =
                scrape_candidate(scraper_candidate("ListingSource", first), &ctx).await;
            match first_outcome.capture.unwrap().on_success {
                RawCaptureSuccessAction::MarkScraped(mut meta) => {
                    meta.identity_claim.take().unwrap().keep();
                }
                _ => panic!("a product page is marked scraped after capture"),
            }
            let second_outcome =
                scrape_candidate(scraper_candidate("ListingSource", second), &ctx).await;

            assert!(second_outcome.capture.is_none());
            assert!(second_outcome.skipped);
        }

        /// The first capture failed, so nothing is captured under the identity: the second
        /// spelling claims and captures it instead of being retired as the first's duplicate.
        #[tokio::test]
        async fn should_capture_the_second_spelling_when_the_first_capture_failed() {
            let (first, second) = spellings();
            let mut scraper_candidates = without_carrier(2);
            scraper_candidates.expect_set_disposition().never();
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("#2798", Some("#2798"), Some("Table")),
            );

            let first_outcome =
                scrape_candidate(scraper_candidate("ListingSource", first), &ctx).await;
            drop(
                first_outcome
                    .capture
                    .expect("the first spelling is captured"),
            );
            let second_outcome =
                scrape_candidate(scraper_candidate("ListingSource", second), &ctx).await;

            assert_eq!(captured_identity(&second_outcome), "#2798");
        }

        /// Another product scraped in the same batch under a reused code is no spelling of the
        /// first: it is parked in a conflict.
        #[tokio::test]
        async fn should_park_another_product_under_an_identity_the_batch_claimed() {
            let (first, second) = spellings();
            let mut scraper_service = MockScraperService::new();
            scraper_service
                .expect_scrape()
                .times(2)
                .returning(|request| {
                    let title = if request.url.path().ends_with("table/") {
                        "Oak table"
                    } else {
                        "Pine dresser"
                    };
                    let scraped =
                        scraped_product(request.url.clone(), "#2798", Some("#2798"), Some(title));
                    Box::pin(async move { scrape_outcome(Ok(Some(scraped))) })
                });
            let mut scraper_candidates = without_carrier(2);
            scraper_candidates.expect_set_disposition().never();
            expect_conflict(&mut scraper_candidates, &second);
            let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

            let first_outcome =
                scrape_candidate(scraper_candidate("ListingSource", first), &ctx).await;
            let second_outcome =
                scrape_candidate(scraper_candidate("ListingSource", second), &ctx).await;

            assert!(first_outcome.capture.is_some());
            assert!(second_outcome.capture.is_none());
            assert_eq!(
                second_outcome.failure_kind.as_deref(),
                Some(SOURCE_LISTING_ID_CONFLICT_ERROR_KIND)
            );
        }
    }

    mod captured_url {
        use super::*;

        /// The shop changed its identity rule, the page shows no identity (a sold page), or it
        /// still shows the captured one: the page is captured under its captured identity.
        #[rstest::rstest]
        #[case::no_identity_on_the_page(None)]
        #[case::captured_identity(Some("J156"))]
        #[tokio::test]
        async fn should_capture_under_the_captured_identity_without_a_lookup(
            #[case] extracted: Option<&str>,
        ) {
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates.expect_identity_carrier().never();
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("J156", extracted, Some("Tea bowl")),
            );

            let outcome = scrape_candidate(
                captured_candidate(
                    "https://haresfur.example/products/j156",
                    "J156",
                    Some("Tea bowl"),
                ),
                &ctx,
            )
            .await;

            assert_eq!(captured_identity(&outcome), "J156");
        }

        /// HaresFur's new rule derives the page URL, which no other URL carries.
        #[tokio::test]
        async fn should_capture_under_the_captured_identity_when_no_other_url_carries_the_page_identity()
         {
            let page = "https://haresfur.example/products/j156";
            let mut scraper_candidates = MockScraperCandidateService::new();
            carrying(
                &mut scraper_candidates,
                "https://haresfur.example/products/j156",
                None,
            );
            scraper_candidates.expect_set_disposition().never();
            scraper_candidates.expect_mark_fetch_failure().never();
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("J156", Some(page), Some("Tea bowl")),
            );

            let outcome =
                scrape_candidate(captured_candidate(page, "J156", Some("Tea bowl")), &ctx).await;

            assert_eq!(captured_identity(&outcome), "J156");
        }

        /// Hara Shobo captured its prints under the page path and derives the print number now,
        /// which another spelling of the print carries; the Gillmark-like case shows each signal.
        #[rstest::rstest]
        #[case::spelling(
            "https://hara.example/print/40475?Sprache=fr",
            "H-1",
            Some("Estampe"),
            "https://hara.example/print/40475",
            Some("Print")
        )]
        #[case::identity_token(
            "https://hara.example/ukiyoe/ukiyoe_detail.php?print_id=40475",
            "/ukiyoe/ukiyoe_detail.php?print_id=40475",
            Some("Hiroshige, Tokaido Hara"),
            "https://hara.example/print/40475",
            Some("Hiroshige: Tokaido, Hara station")
        )]
        #[case::title(
            "https://hara.example/ukiyoe/hiroshige-hara",
            "H-1",
            Some("Hiroshige: Tokaido, Hara station"),
            "https://hara.example/print/40475",
            Some("Hiroshige: Tokaido, Hara station")
        )]
        #[tokio::test]
        async fn should_retire_a_url_whose_page_shows_the_product_another_url_carries(
            #[case] page: &str,
            #[case] captured: &str,
            #[case] title: Option<&str>,
            #[case] carrier: &'static str,
            #[case] carrier_title: Option<&'static str>,
        ) {
            let page_url = url(page);
            let mut scraper_candidates = MockScraperCandidateService::new();
            carrying(
                &mut scraper_candidates,
                "40475",
                Some((carrier, carrier_title)),
            );
            expect_retired(&mut scraper_candidates, &page_url);
            scraper_candidates.expect_mark_fetch_failure().never();
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping(captured, Some("40475"), title),
            );

            let outcome = scrape_candidate(captured_candidate(page, captured, title), &ctx).await;

            assert!(outcome.capture.is_none());
            assert!(outcome.skipped && !outcome.errored);
        }

        /// Gillmark's page 14, captured as May 1971 (`5168`), derives June 1971's `5169`, which
        /// June's own URL carries: nothing shows one product, so the page is neither captured
        /// under `5168` with June's content nor retired.
        #[tokio::test]
        async fn should_park_a_url_whose_page_shows_another_known_product_in_a_conflict() {
            let page = url(GILLMARK_PAGE);
            let mut scraper_candidates = MockScraperCandidateService::new();
            carrying(
                &mut scraper_candidates,
                "5169",
                Some((GILLMARK_CARRIER, Some("June 1971"))),
            );
            scraper_candidates.expect_set_disposition().never();
            expect_conflict(&mut scraper_candidates, &page);
            let ctx = scrape_candidate_context(
                scraper_candidates,
                scraping("5168", Some("5169"), Some("June 1971")),
            );

            let outcome = scrape_candidate(
                captured_candidate(GILLMARK_PAGE, "5168", Some("May 1971")),
                &ctx,
            )
            .await;

            assert!(outcome.capture.is_none());
            assert!(outcome.errored);
            assert_eq!(
                outcome.failure_kind.as_deref(),
                Some(SOURCE_LISTING_ID_CONFLICT_ERROR_KIND)
            );
        }

        /// A first scrape earlier in the batch claimed the identity, but its capture is not
        /// stored yet: a captured URL whose page shows that identity is judged against the
        /// claimant, so it is not captured with the claimant's product under its own listing.
        #[tokio::test]
        async fn should_park_a_url_whose_page_shows_an_identity_the_batch_claimed() {
            let claimant = url("https://example.com/product/223/table/");
            let captured = url("https://example.com/product/9/dresser/");
            let mut scraper_service = MockScraperService::new();
            scraper_service
                .expect_scrape()
                .times(2)
                .returning(|request| {
                    let scraped = if request.url.path().ends_with("table/") {
                        scraped_product(
                            request.url.clone(),
                            "#2798",
                            Some("#2798"),
                            Some("Oak table"),
                        )
                    } else {
                        scraped_product(request.url.clone(), "J9", Some("#2798"), Some("Oak table"))
                    };
                    Box::pin(async move { scrape_outcome(Ok(Some(scraped))) })
                });
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates
                .expect_identity_carrier()
                .times(2)
                .returning(|_, _, _| Box::pin(async { Ok(None) }));
            scraper_candidates.expect_set_disposition().never();
            expect_conflict(&mut scraper_candidates, &captured);
            let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

            let first_outcome =
                scrape_candidate(scraper_candidate("ListingSource", claimant), &ctx).await;
            let captured_outcome = scrape_candidate(
                captured_candidate(captured.as_str(), "J9", Some("Pine dresser")),
                &ctx,
            )
            .await;

            assert!(first_outcome.capture.is_some());
            assert!(captured_outcome.capture.is_none());
            assert_eq!(
                captured_outcome.failure_kind.as_deref(),
                Some(SOURCE_LISTING_ID_CONFLICT_ERROR_KIND)
            );
        }

        /// A redirect can lead to another product, so a redirected page keeps its own identity and
        /// the redirect's conflict check; the carrier rule does not judge it.
        #[tokio::test]
        async fn should_leave_a_redirected_page_to_its_own_identity() {
            let destination = url("https://example.com/products/chair-2");
            let mut scraper_service = MockScraperService::new();
            scraper_service.expect_scrape().once().returning({
                let destination = destination.clone();
                move |_| {
                    let scraped =
                        scraped_product(destination.clone(), "J156", Some("SKU-9"), Some("Chair"));
                    Box::pin(async move { scrape_outcome(Ok(Some(scraped))) })
                }
            });
            let mut scraper_candidates = MockScraperCandidateService::new();
            scraper_candidates.expect_identity_carrier().never();
            let ctx = scrape_candidate_context(scraper_candidates, scraper_service);

            let outcome = scrape_candidate(
                captured_candidate("https://example.com/products/chair", "J156", Some("Chair")),
                &ctx,
            )
            .await;

            assert_eq!(captured_identity(&outcome), "J156");
        }
    }

    #[test]
    fn should_say_the_page_shows_another_known_product() {
        let message = known_product_conflict_message(Some("5168"), "5169", &url(GILLMARK_CARRIER));

        assert_eq!(
            message,
            format!(
                "the page shows another known product: source listing id `5169`, which \
                 {GILLMARK_CARRIER} carries, not the product captured here as `5168`"
            )
        );
    }
}
