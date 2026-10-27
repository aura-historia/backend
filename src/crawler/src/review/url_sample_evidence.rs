//! What the crawler knows about a URL-pattern review's sample URLs independently of the candidate:
//! a URL it captured a product from, or whose page describes one schema.org `Product`, is a
//! product page; one its URL classification without a pattern puts in the category, imprint or
//! info class is not. The live evaluation of a sample fetches, for one decision, a bounded spread
//! of the URLs nothing else classifies. The contract is in `docs/crawler/extraction-and-review.md`
//! (approval guard).

use crate::review::model::{SampleUrlOutcome, UrlPatternReviewCandidate};
use crate::review::product_markup::describes_one_product;
use crate::scraper::scraper_service::html_fetcher::{FetchError, FetchedHtml};
use crate::spider::classification::url_metadata::UrlClass;
use crate::spider::utils::url::CrawledUrl;
use regex::Regex;
use url::Url;

/// The most sample URLs one live evaluation fetches. Each fetch waits for the shop's crawl delay,
/// as a matrix refresh's page fetches do.
pub(crate) const MAX_EVALUATED_SAMPLE_URLS: usize = 8;

/// A URL of a URL-pattern review's sample, with whether the crawler captured a product from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SampleUrl {
    pub url: String,
    pub captured_product: bool,
}

/// A sample URL a live evaluation fetches, with whether the evaluated decision classifies it as a
/// product page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SampleUrlToFetch {
    pub url: String,
    pub decision_matches: bool,
}

/// The sample URLs a live evaluation of `candidate` fetches for schema.org `Product` markup: those
/// nothing else classifies, neither captured products nor category, imprint or info URLs. Half of
/// them come from the URLs the decision classifies as products and half from the others, the rest
/// of the budget going to the other half when one has fewer, each half spread evenly over the
/// sample in its order, so both what the decision takes and what it leaves out are tested.
pub(crate) fn sample_urls_to_fetch(
    candidate: &UrlPatternReviewCandidate,
    sample: &[SampleUrl],
) -> Vec<SampleUrlToFetch> {
    let pattern = candidate.decision_pattern();
    let (matched, unmatched): (Vec<SampleUrlToFetch>, Vec<SampleUrlToFetch>) = sample
        .iter()
        .filter(|url| !url.captured_product && !is_non_product_url(&url.url))
        .map(|url| SampleUrlToFetch {
            url: url.url.clone(),
            decision_matches: classified_as_product(&url.url, pattern.as_ref()),
        })
        .partition(|url| url.decision_matches);
    let half = MAX_EVALUATED_SAMPLE_URLS / 2;
    let matched_count = matched
        .len()
        .min(half.max(MAX_EVALUATED_SAMPLE_URLS.saturating_sub(unmatched.len())));
    let unmatched_count = unmatched
        .len()
        .min(MAX_EVALUATED_SAMPLE_URLS - matched_count);
    spread(&matched, matched_count)
        .chain(spread(&unmatched, unmatched_count))
        .cloned()
        .collect()
}

/// `count` of `items`, at most all of them, evenly spaced from the first one on.
fn spread<T>(items: &[T], count: usize) -> impl Iterator<Item = &T> {
    let count = count.min(items.len());
    (0..count).map(move |step| &items[step * items.len() / count])
}

/// What the fetch of the sample URL `requested` shows. Only the page the URL itself serves can
/// show a product: a redirect to another page, a failed fetch or an error status shows nothing.
pub(crate) fn sample_url_outcome(
    requested: &Url,
    fetched: Result<FetchedHtml, FetchError>,
) -> SampleUrlOutcome {
    match fetched {
        Err(error) => SampleUrlOutcome::FetchFailed {
            error: error.to_string(),
        },
        Ok(fetched) if !fetched.status.is_success() => SampleUrlOutcome::FetchFailed {
            error: format!("HTTP {}", fetched.status.as_u16()),
        },
        Ok(fetched)
            if CrawledUrl::new(fetched.final_url.clone()) != CrawledUrl::new(requested.clone()) =>
        {
            SampleUrlOutcome::Redirected {
                final_url: fetched.final_url.to_string(),
            }
        }
        Ok(fetched) if describes_one_product(&fetched.html) => SampleUrlOutcome::ProductMarkup,
        Ok(_) => SampleUrlOutcome::NoProductMarkup,
    }
}

/// Whether `pattern` classifies `url` as a product page, as the spider and approval do.
pub(crate) fn classified_as_product(url: &str, pattern: Option<&Regex>) -> bool {
    crawled_url(url).is_some_and(|url| url.classify(pattern) == UrlClass::ProductListing)
}

/// A URL the crawler classifies as a category, imprint or info page without a product pattern.
pub(crate) fn is_non_product_url(url: &str) -> bool {
    crawled_url(url).is_some_and(|url| {
        matches!(
            url.classify(None),
            UrlClass::Category | UrlClass::Imprint | UrlClass::Info
        )
    })
}

fn crawled_url(url: &str) -> Option<CrawledUrl> {
    Url::parse(url).ok().map(CrawledUrl::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::policy::NetworkErrorKind;
    use crate::review::model::UrlPatternDecision;

    fn candidate(decision: UrlPatternDecision) -> UrlPatternReviewCandidate {
        UrlPatternReviewCandidate::new(decision, None)
    }

    fn pattern(value: &str) -> UrlPatternReviewCandidate {
        candidate(UrlPatternDecision::Pattern {
            value: value.to_owned(),
        })
    }

    fn sample(urls: &[(&str, bool)]) -> Vec<SampleUrl> {
        urls.iter()
            .map(|(url, captured_product)| SampleUrl {
                url: (*url).to_owned(),
                captured_product: *captured_product,
            })
            .collect()
    }

    fn fetched(urls: &[SampleUrlToFetch]) -> Vec<(&str, bool)> {
        urls.iter()
            .map(|url| (url.url.as_str(), url.decision_matches))
            .collect()
    }

    /// Captured products and category, imprint or info URLs are known already; only the others
    /// are fetched, with whether the decision takes them.
    #[test]
    fn should_fetch_only_the_sample_urls_nothing_else_classifies() {
        let sample = sample(&[
            ("https://antiques.example/item/captured", true),
            ("https://antiques.example/item/chair", false),
            ("https://antiques.example/category/chairs", false),
            ("https://antiques.example/contact", false),
            ("https://antiques.example/journal/restoring-oak", false),
        ]);

        assert_eq!(
            fetched(&sample_urls_to_fetch(&pattern("/item/"), &sample)),
            vec![
                ("https://antiques.example/item/chair", true),
                ("https://antiques.example/journal/restoring-oak", false),
            ]
        );
    }

    /// A large sample is fetched within the budget, half among what the decision takes and half
    /// among what it leaves, each spread over the sample.
    #[test]
    fn should_spread_the_budget_over_what_the_decision_takes_and_leaves() {
        let urls: Vec<String> = (0..20)
            .flat_map(|index| {
                [
                    format!("https://antiques.example/item/{index}"),
                    format!("https://antiques.example/journal/{index}"),
                ]
            })
            .collect();
        let sample = sample(
            &urls
                .iter()
                .map(|url| (url.as_str(), false))
                .collect::<Vec<_>>(),
        );

        let to_fetch = sample_urls_to_fetch(&pattern("/item/"), &sample);

        assert_eq!(
            fetched(&to_fetch),
            vec![
                ("https://antiques.example/item/0", true),
                ("https://antiques.example/item/5", true),
                ("https://antiques.example/item/10", true),
                ("https://antiques.example/item/15", true),
                ("https://antiques.example/journal/0", false),
                ("https://antiques.example/journal/5", false),
                ("https://antiques.example/journal/10", false),
                ("https://antiques.example/journal/15", false),
            ]
        );
    }

    /// When one side has fewer URLs than half the budget, the other side gets the rest; an
    /// undecided or `NO_PATTERN` candidate takes nothing, so every fetched URL is one it leaves.
    #[test]
    fn should_give_the_unused_half_of_the_budget_to_the_other_side() {
        let urls: Vec<String> = (0..10)
            .map(|index| format!("https://antiques.example/journal/{index}"))
            .chain(["https://antiques.example/item/1".to_owned()])
            .collect();
        let sample = sample(
            &urls
                .iter()
                .map(|url| (url.as_str(), false))
                .collect::<Vec<_>>(),
        );

        let decided = sample_urls_to_fetch(&pattern("/item/"), &sample);
        let undecided = sample_urls_to_fetch(&candidate(UrlPatternDecision::Undecided), &sample);

        assert_eq!(decided.len(), MAX_EVALUATED_SAMPLE_URLS);
        assert_eq!(decided.iter().filter(|url| url.decision_matches).count(), 1);
        assert_eq!(undecided.len(), MAX_EVALUATED_SAMPLE_URLS);
        assert!(undecided.iter().all(|url| !url.decision_matches));
        assert!(
            sample_urls_to_fetch(&pattern("/item/"), &sample[..0]).is_empty(),
            "an empty sample fetches nothing"
        );
    }

    fn page(url: &str, status: u16, html: &str) -> Result<FetchedHtml, FetchError> {
        Ok(FetchedHtml {
            html: html.to_owned(),
            final_url: Url::parse(url).unwrap(),
            status: reqwest::StatusCode::from_u16(status).unwrap(),
        })
    }

    const PRODUCT: &str = r#"<script type="application/ld+json">{"@type": "Product"}</script>"#;

    /// Only the page the URL itself serves shows a product; the crawler's own spelling of the URL,
    /// such as a trailing slash, is the same page.
    #[test]
    fn should_take_product_markup_only_from_the_page_the_url_serves() {
        let requested = Url::parse("https://antiques.example/item/chair").unwrap();

        assert_eq!(
            sample_url_outcome(
                &requested,
                page("https://antiques.example/item/chair/", 200, PRODUCT)
            ),
            SampleUrlOutcome::ProductMarkup
        );
        assert_eq!(
            sample_url_outcome(
                &requested,
                page("https://antiques.example/item/chair", 200, "<h1>Chair</h1>")
            ),
            SampleUrlOutcome::NoProductMarkup
        );
        assert_eq!(
            sample_url_outcome(&requested, page("https://antiques.example/", 200, PRODUCT)),
            SampleUrlOutcome::Redirected {
                final_url: "https://antiques.example/".to_owned()
            }
        );
        assert_eq!(
            sample_url_outcome(
                &requested,
                page("https://antiques.example/item/chair", 404, PRODUCT)
            ),
            SampleUrlOutcome::FetchFailed {
                error: "HTTP 404".to_owned()
            }
        );
        assert!(matches!(
            sample_url_outcome(
                &requested,
                Err(FetchError::Network {
                    kind: NetworkErrorKind::RobotsDisallowed,
                    details: "disallowed".to_owned(),
                })
            ),
            SampleUrlOutcome::FetchFailed { .. }
        ));
    }
}
