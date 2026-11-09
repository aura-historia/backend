//! `GET /api/reviews` and `GET /api/listing-sources/{id}/reviews`: the console's review queue, one
//! window at a time, with server-side filters, search and order. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::query_params::{
    id_param, list_window, named, reject_unknown, search_term, sort_direction,
};
use super::{ReviewServer, internal_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{ArtifactType, ReviewSort, ReviewStatus, ReviewWaiting};
use crate::review::readers::ReviewQuery;
use listing_source_core::ListingSourceId;
use serde_json::json;
use std::collections::HashMap;

const PARAMETERS: [&str; 9] = [
    "q",
    "status",
    "artifact",
    "waiting",
    "listing_source",
    "sort",
    "dir",
    "limit",
    "offset",
];
const SOURCE_PARAMETERS: [&str; 4] = ["status", "artifact", "limit", "offset"];

impl ReviewServer {
    /// One window of the review queue, as the query of `request` asks for it.
    pub(super) async fn review_list(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let query = match review_query(&request.query) {
            Ok(query) => query,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self.review_queue.list_reviews(&query).await {
            Ok(page) => HttpResponse::json(200, &page),
            Err(error) => internal_error(error),
        }
    }

    /// One window of the reviews of `listing_source_id`, as the query of `request` asks for it;
    /// `404` when the ListingSource does not exist.
    pub(super) async fn source_review_list(
        &self,
        listing_source_id: ListingSourceId,
        request: &ParsedRequest<'_>,
    ) -> HttpResponse {
        let query = match source_review_query(&request.query) {
            Ok(query) => query,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self
            .listing_sources
            .find_listing_source(listing_source_id)
            .await
        {
            Ok(Some(_)) => {}
            Ok(None) => return super::listing_source_list::source_not_found(listing_source_id),
            Err(error) => return internal_error(error),
        }
        match self
            .review_queue
            .list_source_reviews(listing_source_id, &query)
            .await
        {
            Ok(page) => HttpResponse::json(200, &page),
            Err(error) => internal_error(error),
        }
    }
}

/// The window `query` asks for. `q` searches the ListingSource's name, `waiting` lists the
/// pending reviews that wait for a human or for a repair, and `sort` names the order, `queue` by
/// default, in the direction `dir` names or the order's own. A value that is given but not
/// accepted is refused with what is accepted.
fn review_query(query: &HashMap<String, String>) -> Result<ReviewQuery, String> {
    reject_unknown(query, &PARAMETERS)?;
    let sort = named::<ReviewSort>(query, "sort")?.unwrap_or_default();
    Ok(ReviewQuery {
        search: search_term(query)?,
        listing_source_id: id_param(query, "listing_source", "a ListingSource id")?,
        status: named::<ReviewStatus>(query, "status")?,
        artifact_type: named::<ArtifactType>(query, "artifact")?,
        waiting: named::<ReviewWaiting>(query, "waiting")?,
        sort,
        direction: sort_direction(query, sort.default_direction())?,
        window: list_window(query)?,
    })
}

/// The window of one ListingSource's reviews `query` asks for.
fn source_review_query(query: &HashMap<String, String>) -> Result<ReviewQuery, String> {
    reject_unknown(query, &SOURCE_PARAMETERS)?;
    Ok(ReviewQuery {
        status: named::<ReviewStatus>(query, "status")?,
        artifact_type: named::<ArtifactType>(query, "artifact")?,
        window: list_window(query)?,
        ..ReviewQuery::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::review::model::{ListWindow, SearchTerm, SortDirection};

    fn pairs_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    fn query_of(pairs: &[(&str, &str)]) -> Result<ReviewQuery, String> {
        review_query(&pairs_of(pairs))
    }

    #[test]
    fn should_read_the_first_window_in_queue_order_when_nothing_is_asked_for() {
        assert_eq!(query_of(&[]), Ok(ReviewQuery::default()));
        assert_eq!(ReviewQuery::default().sort, ReviewSort::Queue);
    }

    #[test]
    fn should_read_every_filter_the_order_and_the_window_asked_for() {
        let listing_source_id = ListingSourceId::new();

        let query = query_of(&[
            ("q", " antik "),
            ("status", "APPROVED"),
            ("artifact", "URL_PATTERN"),
            ("waiting", "repair"),
            ("listing_source", &listing_source_id.to_string()),
            ("sort", "updated"),
            ("dir", "asc"),
            ("limit", "50"),
            ("offset", "100"),
        ]);

        assert_eq!(
            query,
            Ok(ReviewQuery {
                search: SearchTerm::new("antik"),
                listing_source_id: Some(listing_source_id),
                status: Some(ReviewStatus::Approved),
                artifact_type: Some(ArtifactType::UrlPattern),
                waiting: Some(ReviewWaiting::Repair),
                sort: ReviewSort::Updated,
                direction: SortDirection::Ascending,
                window: ListWindow::new(50, 100).unwrap_or_default(),
            })
        );
    }

    #[test]
    fn should_sort_a_column_in_its_own_direction_unless_asked_otherwise() {
        let direction = |pairs: &[(&str, &str)]| query_of(pairs).map(|query| query.direction);

        assert_eq!(
            direction(&[("sort", "source")]),
            Ok(SortDirection::Ascending)
        );
        assert_eq!(
            direction(&[("sort", "created")]),
            Ok(SortDirection::Descending)
        );
        assert_eq!(
            direction(&[("sort", "source"), ("dir", "desc")]),
            Ok(SortDirection::Descending)
        );
    }

    #[test]
    fn should_refuse_values_that_are_not_accepted() {
        for pairs in [
            [("status", "PENDING")],
            [("status", "")],
            [("artifact", "SCHEMA")],
            [("waiting", "agents")],
            [("sort", "reviewed")],
            [("dir", "up")],
            [("listing_source", "not-a-source")],
            [("limit", "0")],
            [("limit", "201")],
            [("offset", "-1")],
            [("q", "\u{7}")],
            [("after", "cr_x")],
        ] {
            assert!(query_of(&pairs).is_err(), "{pairs:?}");
        }
    }

    #[test]
    fn should_read_only_the_filters_and_window_of_a_listing_sources_reviews() {
        let query = source_review_query(&pairs_of(&[("status", "REJECTED"), ("offset", "100")]));

        assert_eq!(
            query.map(|query| (query.status, query.window.offset())),
            Ok((Some(ReviewStatus::Rejected), 100))
        );
        assert!(source_review_query(&pairs_of(&[("q", "antik")])).is_err());
    }
}
