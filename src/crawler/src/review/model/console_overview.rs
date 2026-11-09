//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

use serde::Serialize;

/// What the operator sees first: what the crawler does right now and what needs a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConsoleOverview {
    pub sources: SourceCounts,
    pub reviews: ReviewCounts,
    /// Whether review agents work the reviews, so a review can wait for a human.
    pub agent_mode: bool,
}

/// How the ListingSources stand. The four conditions overlap, and `needs_attention` counts a
/// source once whichever of them it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceCounts {
    /// ListingSources this process is crawling right now.
    pub crawling_now: i64,
    /// ListingSources this process is scraping right now.
    pub scraping_now: i64,
    pub needs_attention: i64,
    pub crawl_needs_operator: i64,
    pub blocked_by_review: i64,
    pub failing_to_scrape: i64,
    pub held_removals: i64,
}

/// The pending reviews of all ListingSources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ReviewCounts {
    pub pending_total: i64,
    pub pending_by_artifact: PendingByArtifact,
    /// Of `pending_total`, those of ListingSources with crawling disabled, which hold back no
    /// crawl or scrape.
    pub pending_disabled_sources: i64,
    /// Of `pending_total`, those that wait for a human; none outside `agent` authoring mode.
    pub waiting_for_human: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PendingByArtifact {
    #[serde(rename = "PRODUCT_SCHEMA")]
    pub product_schema: i64,
    #[serde(rename = "URL_PATTERN")]
    pub url_pattern: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_serialize_the_counts_as_the_contract_names_them() {
        let overview = ConsoleOverview {
            sources: SourceCounts {
                crawling_now: 4,
                scraping_now: 11,
                needs_attention: 23,
                crawl_needs_operator: 6,
                blocked_by_review: 9,
                failing_to_scrape: 7,
                held_removals: 1,
            },
            reviews: ReviewCounts {
                pending_total: 312,
                pending_by_artifact: PendingByArtifact {
                    product_schema: 241,
                    url_pattern: 71,
                },
                pending_disabled_sources: 4,
                waiting_for_human: 9,
            },
            agent_mode: true,
        };

        assert_eq!(
            serde_json::to_value(overview).unwrap_or_else(|error| panic!("{error}")),
            json!({
                "sources": {
                    "crawling_now": 4, "scraping_now": 11, "needs_attention": 23,
                    "crawl_needs_operator": 6, "blocked_by_review": 9,
                    "failing_to_scrape": 7, "held_removals": 1
                },
                "reviews": {
                    "pending_total": 312,
                    "pending_by_artifact": { "PRODUCT_SCHEMA": 241, "URL_PATTERN": 71 },
                    "pending_disabled_sources": 4,
                    "waiting_for_human": 9
                },
                "agent_mode": true
            })
        );
    }
}
