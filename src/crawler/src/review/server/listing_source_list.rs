//! The console's ListingSource reads: `GET /api/listing_sources`, the overview page by page with
//! server-side filters, search and sort, and `GET /api/listing_sources/{id}`, one ListingSource
//! with its review history, URL pattern and product schemas. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::query_params::{
    after_cursor, named, optional_bool, page_size, reject_unknown, search_term, sort_direction,
};
use super::review_list::page_read_error;
use super::{ReviewServer, internal_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{SourceActivity, SourceHealth, SourceSort};
use crate::review::readers::{ListingSourceQuery, ReviewHistoryQuery};
use listing_source_core::ListingSourceId;
use serde_json::json;
use std::collections::HashMap;

const LIST_PARAMETERS: [&str; 8] = [
    "q",
    "activity",
    "health",
    "crawl_enabled",
    "sort",
    "order",
    "after",
    "limit",
];
const DETAIL_PARAMETERS: [&str; 2] = ["after", "limit"];
const DETAIL_PREFIX: &str = "/api/listing_sources/";

impl ReviewServer {
    /// One page of the ListingSource overview, as the query of `request` asks for it.
    pub(super) async fn listing_source_list(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let query = match listing_source_query(&request.query) {
            Ok(query) => query,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self.listing_sources.list_listing_sources(&query).await {
            Ok(page) => HttpResponse::json(200, &page),
            Err(error) => page_read_error(error),
        }
    }

    /// Answers `GET /api/listing_sources/{id}`, or `None` for any other request.
    pub(super) async fn route_listing_source_detail(
        &self,
        request: &ParsedRequest<'_>,
    ) -> Option<HttpResponse> {
        let id = request.path.strip_prefix(DETAIL_PREFIX)?;
        (request.method == "GET").then_some(())?;
        Some(self.listing_source_detail(id, &request.query).await)
    }

    async fn listing_source_detail(
        &self,
        id: &str,
        query: &HashMap<String, String>,
    ) -> HttpResponse {
        let Ok(listing_source_id) = id.parse::<ListingSourceId>() else {
            return HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }));
        };
        let history = match review_history_query(query) {
            Ok(history) => history,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self
            .listing_source_details
            .find_listing_source(listing_source_id, history)
            .await
        {
            Ok(Some(detail)) => HttpResponse::json(200, &detail),
            Ok(None) => HttpResponse::json(
                404,
                &json!({ "error": format!("ListingSource {listing_source_id} does not exist") }),
            ),
            Err(error) => page_read_error(error),
        }
    }

    /// `GET /api/overview`: the counts of the console's first page.
    pub(super) async fn console_overview(&self) -> HttpResponse {
        match self.overview.overview().await {
            Ok(overview) => HttpResponse::json(200, &overview),
            Err(error) => internal_error(error),
        }
    }
}

/// The page `query` asks for: `activity` and `health` filter the ListingSources, `q` searches
/// their name, slug and host and their id, `sort` names the overview column to sort by, with the
/// direction `order` names or the column's own, and `after` and `limit` page through the result. A
/// value that is given but not accepted is refused with what is accepted.
fn listing_source_query(query: &HashMap<String, String>) -> Result<ListingSourceQuery, String> {
    reject_unknown(query, &LIST_PARAMETERS)?;
    let sort = named::<SourceSort>(query, "sort")?.unwrap_or_default();
    Ok(ListingSourceQuery {
        search: search_term(query)?,
        activity: named::<SourceActivity>(query, "activity")?,
        health: named::<SourceHealth>(query, "health")?,
        crawl_enabled: optional_bool(query, "crawl_enabled")?,
        sort,
        direction: sort_direction(query, sort.default_direction())?,
        after: after_cursor(query, "a ListingSource id")?,
        page_size: page_size(query)?,
    })
}

/// The page of the review history `query` asks for.
fn review_history_query(query: &HashMap<String, String>) -> Result<ReviewHistoryQuery, String> {
    reject_unknown(query, &DETAIL_PARAMETERS)?;
    Ok(ReviewHistoryQuery {
        after: after_cursor(query, "a review id")?,
        page_size: page_size(query)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{PageSize, SearchTerm, SortDirection};

    fn pairs_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn query_of(pairs: &[(&str, &str)]) -> Result<ListingSourceQuery, String> {
        listing_source_query(&pairs_of(pairs))
    }

    #[test]
    fn should_read_the_first_page_ranked_by_attention_when_nothing_is_asked_for() {
        assert_eq!(query_of(&[]), Ok(ListingSourceQuery::default()));
        assert_eq!(ListingSourceQuery::default().sort, SourceSort::Attention);
    }

    #[test]
    fn should_read_every_filter_the_sort_and_the_page_asked_for() {
        let after = ListingSourceId::new();

        let query = query_of(&[
            ("q", "antik.example"),
            ("activity", "active"),
            ("health", "scrape_failing"),
            ("crawl_enabled", "true"),
            ("sort", "product_urls"),
            ("order", "asc"),
            ("after", &after.to_string()),
            ("limit", "25"),
        ]);

        assert_eq!(
            query,
            Ok(ListingSourceQuery {
                search: SearchTerm::new("antik.example"),
                activity: Some(SourceActivity::Active),
                health: Some(SourceHealth::ScrapeFailing),
                crawl_enabled: Some(true),
                sort: SourceSort::ProductUrls,
                direction: SortDirection::Ascending,
                after: Some(after),
                page_size: PageSize::new(25).unwrap_or_default(),
            })
        );
    }

    #[test]
    fn should_sort_a_column_in_its_own_direction_unless_asked_otherwise() {
        let direction = |pairs: &[(&str, &str)]| query_of(pairs).map(|query| query.direction);

        assert_eq!(
            direction(&[("sort", "listing_source_name")]),
            Ok(SortDirection::Ascending)
        );
        assert_eq!(
            direction(&[("sort", "pending_reviews")]),
            Ok(SortDirection::Descending)
        );
        assert_eq!(
            direction(&[("sort", "listing_source_name"), ("order", "desc")]),
            Ok(SortDirection::Descending)
        );
    }

    #[test]
    fn should_refuse_values_that_are_not_accepted() {
        let review_id = crate::CrawlerReviewId::new().to_string();
        for pairs in [
            [("activity", "running")],
            [("health", "sick")],
            [("crawl_enabled", "1")],
            [("sort", "updated")],
            [("order", "up")],
            [("after", "not-a-source")],
            [("after", review_id.as_str())],
            [("limit", "501")],
            [("page", "2")],
        ] {
            assert!(query_of(&pairs).is_err(), "{pairs:?}");
        }
    }

    #[test]
    fn should_read_the_review_history_page_of_a_listing_source() {
        let after = crate::CrawlerReviewId::new();

        assert_eq!(
            review_history_query(&pairs_of(&[("after", &after.to_string()), ("limit", "5")])),
            Ok(ReviewHistoryQuery {
                after: Some(after),
                page_size: PageSize::new(5).unwrap_or_default(),
            })
        );
        assert_eq!(
            review_history_query(&pairs_of(&[])),
            Ok(ReviewHistoryQuery::default())
        );
        assert!(review_history_query(&pairs_of(&[("status", "APPROVED")])).is_err());
    }
}
