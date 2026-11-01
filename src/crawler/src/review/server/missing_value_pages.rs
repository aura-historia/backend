//! The console's routes for pages whose last scrape recorded `MissingRequiredValue`: the
//! ListingSource's list of them, and adding one to its pending product-schema review, opened for
//! the purpose when none is pending, so a reviewer can declare a removed-page template for a
//! removal page that keeps an empty title element. The contract is in
//! `docs/crawler/extraction-and-review.md` (removed-page templates).

use super::{ReviewServer, internal_error, parse_listing_source_id_with_suffix, review_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::ReviewActor;
use serde::Deserialize;
use serde_json::json;
use url::Url;

const PAGES_SUFFIX: &str = "/missing-value-pages";
const REVIEW_SUFFIX: &str = "/missing-value-pages/review";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MissingValuePagePayload {
    url: String,
}

impl ReviewServer {
    /// Answers `GET /api/listing-sources/{id}/missing-value-pages` and `POST
    /// /api/listing-sources/{id}/missing-value-pages/review`, or `None` for any other request.
    pub(super) async fn route_missing_value_pages(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if !request.path.starts_with("/api/listing-sources/") {
            return None;
        }
        match request.method.as_str() {
            "GET" if request.path.ends_with(PAGES_SUFFIX) => {
                Some(self.list_missing_value_pages(request.path).await)
            }
            "POST" if request.path.ends_with(REVIEW_SUFFIX) => Some(
                self.review_missing_value_page(request, actor)
                    .await
                    .unwrap_or_else(|response| response),
            ),
            _ => None,
        }
    }

    async fn list_missing_value_pages(&self, path: &str) -> HttpResponse {
        let Some(listing_source_id) = parse_listing_source_id_with_suffix(path, PAGES_SUFFIX)
        else {
            return HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }));
        };
        match self
            .repository
            .list_missing_value_pages(&listing_source_id)
            .await
        {
            Ok(pages) => HttpResponse::json(200, &json!({ "pages": pages })),
            Err(error) => internal_error(error),
        }
    }

    /// Adds the page to the ListingSource's pending product-schema review as a page the reviewer
    /// added, opening the review when none is pending. The page is checked before it is fetched,
    /// and the review is opened only once the fetch succeeded and led to a page the review
    /// accepts, so a failed fetch or a redirect away from the product opens none.
    async fn review_missing_value_page(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Result<HttpResponse, HttpResponse> {
        let listing_source_id = parse_listing_source_id_with_suffix(request.path, REVIEW_SUFFIX)
            .ok_or_else(|| {
                HttpResponse::json(400, &json!({ "error": "invalid ListingSource id" }))
            })?;
        let payload: MissingValuePagePayload = serde_json::from_str(request.body)
            .map_err(|error| HttpResponse::json(400, &json!({ "error": error.to_string() })))?;
        let url = Url::parse(&payload.url).map_err(|error| {
            HttpResponse::json(
                400,
                &json!({ "error": "invalid page url", "details": error.to_string() }),
            )
        })?;
        self.repository
            .missing_value_page_review_schemas(&listing_source_id, &url)
            .await
            .map_err(review_error)?;
        let fetched = self.fetch_reviewer_page(&url).await?;
        self.repository
            .check_missing_value_page_fetch(&listing_source_id, &url, &fetched.final_url)
            .await
            .map_err(review_error)?;
        let review_id = self
            .repository
            .open_review_for_missing_value_page(&listing_source_id, &url)
            .await
            .map_err(review_error)?;
        let page = self
            .repository
            .add_reviewer_page(review_id, actor, &url, &fetched.final_url, &fetched.html)
            .await
            .map_err(review_error)?;
        Ok(HttpResponse::json(
            201,
            &json!({ "review_id": review_id, "review_page": page }),
        ))
    }
}
