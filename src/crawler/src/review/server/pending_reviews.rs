//! `GET /api/pending-reviews`: the review agents' queue, the pending reviews of crawl-enabled
//! ListingSources page by page, which the review agent runner, its digest and `review-mcp` read in
//! full instead of the browser console's `GET /api/reviews`. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::{ReviewServer, review_error};
use crate::CrawlerReviewId;
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::repository::{PendingReviewPageSize, PendingReviewQuery};
use serde_json::json;
use std::collections::HashMap;

impl ReviewServer {
    /// One page of the agents' queue, as the query of `request` asks for it.
    pub(super) async fn pending_reviews(&self, request: &ParsedRequest<'_>) -> HttpResponse {
        let query = match pending_review_query(&request.query) {
            Ok(query) => query,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error })),
        };
        match self.repository.list_pending_reviews(query).await {
            Ok(page) => HttpResponse::json(200, &page),
            Err(error) => review_error(error),
        }
    }
}

/// The page `query` asks for: `human_required` `true` or `false` lists only the reviews that do or
/// do not wait for a human, `after` names the review the previous page ended with, and `limit`
/// is the page size. A value that is given but not accepted is refused with what is accepted.
fn pending_review_query(query: &HashMap<String, String>) -> Result<PendingReviewQuery, String> {
    let human_required = match query.get("human_required").map(String::as_str) {
        None => None,
        Some("true") => Some(true),
        Some("false") => Some(false),
        Some(_) => return Err("human_required must be true or false".to_owned()),
    };
    let after = query
        .get("after")
        .map(|after| {
            after.parse::<CrawlerReviewId>().map_err(|_| {
                "after must be a review id, such as the next_after of the previous page".to_owned()
            })
        })
        .transpose()?;
    let page_size = match query.get("limit") {
        None => PendingReviewPageSize::DEFAULT,
        Some(limit) => limit
            .parse()
            .ok()
            .and_then(PendingReviewPageSize::new)
            .ok_or_else(|| {
                format!(
                    "limit must be a whole number from 1 to {}",
                    PendingReviewPageSize::MAX.get()
                )
            })?,
    };
    Ok(PendingReviewQuery {
        human_required,
        after,
        page_size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query_of(pairs: &[(&str, &str)]) -> Result<PendingReviewQuery, String> {
        pending_review_query(
            &pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect(),
        )
    }

    #[test]
    fn should_read_the_first_page_of_the_whole_queue_when_nothing_is_asked_for() {
        assert_eq!(query_of(&[]), Ok(PendingReviewQuery::default()));
    }

    #[test]
    fn should_read_the_filter_cursor_and_page_size_asked_for() {
        let after = CrawlerReviewId::new();

        let query = query_of(&[
            ("human_required", "false"),
            ("after", &after.to_string()),
            ("limit", "250"),
        ]);

        assert_eq!(
            query,
            Ok(PendingReviewQuery {
                human_required: Some(false),
                after: Some(after),
                page_size: PendingReviewPageSize::new(250).unwrap_or(PendingReviewPageSize::MAX),
            })
        );
        assert_eq!(
            query_of(&[("human_required", "true")]).map(|query| query.human_required),
            Ok(Some(true))
        );
    }

    #[test]
    fn should_refuse_values_that_are_not_accepted() {
        let bare_uuid = CrawlerReviewId::new().as_uuid().to_string();
        for pairs in [
            [("human_required", "yes")],
            [("human_required", "")],
            [("after", "not-a-review")],
            [("after", bare_uuid.as_str())],
            [("limit", "0")],
            [("limit", "501")],
            [("limit", "ten")],
        ] {
            assert!(query_of(&pairs).is_err(), "{pairs:?}");
        }
    }
}
