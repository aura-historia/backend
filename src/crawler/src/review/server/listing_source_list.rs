//! The console's ListingSource reads: `GET /api/listing_sources`, the overview one window at a time
//! with server-side filters, search and order, `GET /api/listing-sources/{id}`, one ListingSource
//! with its live configuration, and `GET /api/overview`, the counts of the first page. The contract
//! is in `docs/crawler/operations.md` (review service).

use super::query_params::{list_window, named, reject_unknown, search_term, sort_direction};
use super::{ReviewServer, read_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{SourceActivity, SourceSort, SourceStatus};
use crate::review::readers::ListingSourceQuery;
use listing_source_core::ListingSourceId;
use serde_json::json;
use std::collections::HashMap;

const LIST_PARAMETERS: [&str; 7] = ["q", "status", "activity", "sort", "dir", "limit", "offset"];
const DETAIL_PREFIX: &str = "/api/listing-sources/";
const REVIEWS_SUFFIX: &str = "/reviews";

impl ReviewServer {
    /// One window of the ListingSource overview, as the query of `request` asks for it.
    pub(super) async fn listing_source_list(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let query = match listing_source_query(&request.query) {
            Ok(query) => query,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self.listing_sources.list_listing_sources(&query).await {
            Ok(page) => HttpResponse::json(200, &page),
            Err(error) => read_error(error),
        }
    }

    /// Answers `GET /api/listing-sources/{id}` and `GET /api/listing-sources/{id}/reviews`, or
    /// `None` for any other request.
    pub(super) async fn route_listing_source_page(
        &self,
        request: &ParsedRequest<'_>,
    ) -> Option<HttpResponse> {
        let rest = request.path.strip_prefix(DETAIL_PREFIX)?;
        if request.method != "GET" {
            return None;
        }
        let (id, reviews) = match rest.strip_suffix(REVIEWS_SUFFIX) {
            Some(id) => (id, true),
            None => (rest, false),
        };
        if id.contains('/') {
            return None;
        }
        let Ok(listing_source_id) = id.parse::<ListingSourceId>() else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        Some(if reviews {
            self.source_review_list(listing_source_id, request).await
        } else {
            self.listing_source_detail(listing_source_id).await
        })
    }

    async fn listing_source_detail(&self, listing_source_id: ListingSourceId) -> HttpResponse {
        match self
            .listing_source_details
            .find_listing_source(listing_source_id)
            .await
        {
            Ok(Some(detail)) => HttpResponse::json(200, &detail),
            Ok(None) => source_not_found(listing_source_id),
            Err(error) => read_error(error),
        }
    }

    /// `GET /api/overview`: the counts of the console's first page.
    pub(super) async fn console_overview(&self) -> HttpResponse {
        match self.overview.overview().await {
            Ok(overview) => HttpResponse::json(200, &overview),
            Err(error) => read_error(error),
        }
    }
}

pub(super) fn source_not_found(listing_source_id: ListingSourceId) -> HttpResponse {
    HttpResponse::json(
        404,
        &json!({ "error": format!("ListingSource {listing_source_id} does not exist") }),
    )
}

/// The window `query` asks for: `status` and `activity` filter the ListingSources, `q` searches
/// their name and host, `sort` names the overview column to sort by, with the direction `dir`
/// names or the column's own. A value that is given but not accepted is refused with what is
/// accepted.
fn listing_source_query(query: &HashMap<String, String>) -> Result<ListingSourceQuery, String> {
    reject_unknown(query, &LIST_PARAMETERS)?;
    let sort = named::<SourceSort>(query, "sort")?.unwrap_or_default();
    Ok(ListingSourceQuery {
        search: search_term(query)?,
        activity: named::<SourceActivity>(query, "activity")?,
        status: named::<SourceStatus>(query, "status")?,
        sort,
        direction: sort_direction(query, sort.default_direction())?,
        window: list_window(query)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{ListWindow, SearchTerm, SortDirection};

    fn query_of(pairs: &[(&str, &str)]) -> Result<ListingSourceQuery, String> {
        listing_source_query(
            &pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn should_read_the_first_window_ranked_by_attention_when_nothing_is_asked_for() {
        assert_eq!(query_of(&[]), Ok(ListingSourceQuery::default()));
        assert_eq!(ListingSourceQuery::default().sort, SourceSort::Attention);
    }

    #[test]
    fn should_read_every_filter_the_sort_and_the_window_asked_for() {
        let query = query_of(&[
            ("q", "antik.example"),
            ("status", "failing"),
            ("activity", "idle"),
            ("sort", "last_successful_scrape"),
            ("dir", "asc"),
            ("limit", "25"),
            ("offset", "50"),
        ]);

        assert_eq!(
            query,
            Ok(ListingSourceQuery {
                search: SearchTerm::new("antik.example"),
                activity: Some(SourceActivity::Idle),
                status: Some(SourceStatus::Failing),
                sort: SourceSort::LastSuccessfulScrape,
                direction: SortDirection::Ascending,
                window: ListWindow::new(25, 50).unwrap_or_default(),
            })
        );
    }

    #[test]
    fn should_sort_a_column_in_its_own_direction_unless_asked_otherwise() {
        let direction = |pairs: &[(&str, &str)]| query_of(pairs).map(|query| query.direction);

        assert_eq!(direction(&[("sort", "name")]), Ok(SortDirection::Ascending));
        assert_eq!(
            direction(&[("sort", "pending_reviews")]),
            Ok(SortDirection::Descending)
        );
        assert_eq!(
            direction(&[("sort", "name"), ("dir", "desc")]),
            Ok(SortDirection::Descending)
        );
    }

    #[test]
    fn should_refuse_values_that_are_not_accepted() {
        for pairs in [
            [("activity", "active")],
            [("status", "sick")],
            [("sort", "updated")],
            [("dir", "up")],
            [("limit", "201")],
            [("offset", "x")],
            [("after", "ls_x")],
            [("page", "2")],
        ] {
            assert!(query_of(&pairs).is_err(), "{pairs:?}");
        }
    }
}
