//! `GET /api/reviews`: the console's review queue, page by page, with server-side filters and
//! search. The contract is in `docs/crawler/operations.md` (review service).

use super::query_params::{
    after_cursor, id_param, named, optional_bool, page_size, reject_unknown, search_term,
    sort_direction,
};
use super::{ReviewServer, internal_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{ArtifactType, QueryValue, ReviewStatus, SortDirection};
use crate::review::readers::{PageReadError, ReviewQuery};
use serde_json::json;
use std::collections::HashMap;

const PARAMETERS: [&str; 8] = [
    "q",
    "listing_source_id",
    "status",
    "artifact_type",
    "human_required",
    "order",
    "after",
    "limit",
];

impl ReviewServer {
    /// One page of the review queue, as the query of `request` asks for it.
    pub(super) async fn review_list(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let query = match review_query(&request.query) {
            Ok(query) => query,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self.review_queue.list_reviews(&query).await {
            Ok(page) => HttpResponse::json(200, &page),
            Err(error) => page_read_error(error),
        }
    }
}

/// The answer to a read that failed: `404` for a cursor that names no row, else `500`.
pub(super) fn page_read_error(error: PageReadError) -> HttpResponse {
    match error {
        PageReadError::UnknownCursor(_) => {
            HttpResponse::json(404, &json!({ "error": error.to_string() }))
        }
        PageReadError::Database(error) => internal_error(error),
    }
}

/// The page `query` asks for. `status` lists the statuses to read, separated by commas; `q`
/// searches the ListingSource's name, slug and host and the review id; `order` sorts by creation
/// time, newest first by default. A value that is given but not accepted is refused with what is
/// accepted.
fn review_query(query: &HashMap<String, String>) -> Result<ReviewQuery, String> {
    reject_unknown(query, &PARAMETERS)?;
    Ok(ReviewQuery {
        search: search_term(query)?,
        listing_source_id: id_param(query, "listing_source_id", "a ListingSource id")?,
        statuses: statuses(query)?,
        artifact_type: named::<ArtifactType>(query, "artifact_type")?,
        human_required: optional_bool(query, "human_required")?,
        direction: sort_direction(query, SortDirection::Descending)?,
        after: after_cursor(query, "a review id")?,
        page_size: page_size(query)?,
    })
}

fn statuses(query: &HashMap<String, String>) -> Result<Vec<ReviewStatus>, String> {
    let Some(list) = query.get("status") else {
        return Ok(Vec::new());
    };
    let mut statuses = Vec::new();
    for name in list.split(',') {
        let status = ReviewStatus::parse(name).ok_or_else(|| {
            format!(
                "status must be a comma-separated list of {}",
                ReviewStatus::accepted()
            )
        })?;
        if !statuses.contains(&status) {
            statuses.push(status);
        }
    }
    Ok(statuses)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CrawlerReviewId;
    use crate::review::model::{PageSize, SearchTerm};
    use listing_source_core::ListingSourceId;

    fn query_of(pairs: &[(&str, &str)]) -> Result<ReviewQuery, String> {
        review_query(
            &pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn should_read_the_first_page_of_all_reviews_newest_first_when_nothing_is_asked_for() {
        assert_eq!(query_of(&[]), Ok(ReviewQuery::default()));
        assert_eq!(ReviewQuery::default().direction, SortDirection::Descending);
    }

    #[test]
    fn should_read_every_filter_the_cursor_and_the_page_size_asked_for() {
        let after = CrawlerReviewId::new();
        let listing_source_id = ListingSourceId::new();

        let query = query_of(&[
            ("q", " antik "),
            ("listing_source_id", &listing_source_id.to_string()),
            ("status", "APPROVED,REJECTED,APPROVED"),
            ("artifact_type", "URL_PATTERN"),
            ("human_required", "true"),
            ("order", "asc"),
            ("after", &after.to_string()),
            ("limit", "250"),
        ]);

        assert_eq!(
            query,
            Ok(ReviewQuery {
                search: SearchTerm::new("antik"),
                listing_source_id: Some(listing_source_id),
                statuses: vec![ReviewStatus::Approved, ReviewStatus::Rejected],
                artifact_type: Some(ArtifactType::UrlPattern),
                human_required: Some(true),
                direction: SortDirection::Ascending,
                after: Some(after),
                page_size: PageSize::new(250).unwrap_or_default(),
            })
        );
    }

    #[test]
    fn should_refuse_values_that_are_not_accepted() {
        let bare_uuid = CrawlerReviewId::new().as_uuid().to_string();
        for pairs in [
            [("status", "PENDING")],
            [("status", "")],
            [("status", "APPROVED,")],
            [("artifact_type", "SCHEMA")],
            [("human_required", "yes")],
            [("order", "newest")],
            [("listing_source_id", "not-a-source")],
            [("after", bare_uuid.as_str())],
            [("limit", "0")],
            [("limit", "501")],
            [("q", "\u{7}")],
            [("humen_required", "true")],
        ] {
            assert!(query_of(&pairs).is_err(), "{pairs:?}");
        }
    }
}
