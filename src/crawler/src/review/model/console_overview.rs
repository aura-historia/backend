//! `GET /api/overview`: the counts of the console's first page. The contract is in
//! `docs/crawler/operations.md` (review service).

use serde::Serialize;

/// What the operator sees first: how many ListingSources work or wait, and how many reviews wait
/// for whom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConsoleOverview {
    pub sources: SourceCounts,
    pub pending_reviews: PendingReviewCounts,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SourceCounts {
    pub total: i64,
    pub crawl_enabled: i64,
    /// Crawl-enabled ListingSources whose last crawl ended waiting for an operator.
    pub needs_operator: i64,
    /// ListingSources this process is crawling right now.
    pub crawling_now: i64,
    /// ListingSources this process is scraping right now.
    pub scraping_now: i64,
}

/// The pending reviews of all ListingSources, whether or not they are crawl-enabled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PendingReviewCounts {
    pub total: i64,
    pub url_pattern: i64,
    pub product_schema: i64,
    /// Those that wait for a human, as the reviews' `human_required` says; none outside `agent`
    /// authoring mode.
    pub waiting_for_human: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn should_serialize_the_counts_as_the_contract_names_them() {
        let overview = ConsoleOverview {
            sources: SourceCounts {
                total: 1144,
                crawl_enabled: 900,
                needs_operator: 7,
                crawling_now: 2,
                scraping_now: 3,
            },
            pending_reviews: PendingReviewCounts {
                total: 30,
                url_pattern: 10,
                product_schema: 20,
                waiting_for_human: 4,
            },
        };

        assert_eq!(
            serde_json::to_value(overview).unwrap_or_else(|error| panic!("{error}")),
            json!({
                "sources": {
                    "total": 1144,
                    "crawl_enabled": 900,
                    "needs_operator": 7,
                    "crawling_now": 2,
                    "scraping_now": 3
                },
                "pending_reviews": {
                    "total": 30,
                    "url_pattern": 10,
                    "product_schema": 20,
                    "waiting_for_human": 4
                }
            })
        );
    }
}
