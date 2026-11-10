//! The console's routes for the operator's label that a ListingSource cannot be scraped: a human
//! marks it with a reason and a note, or clears the mark. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::{ReviewServer, parse_listing_source_id_with_suffix, review_error, verdict_answer};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{ReviewActor, UnscrapableMark};
use listing_source_core::ListingSourceId;
use serde_json::json;

const UNSCRAPABLE_SUFFIX: &str = "/unscrapable";

impl ReviewServer {
    /// Answers `POST` and `DELETE /api/listing-sources/{id}/unscrapable`, or `None` for any other
    /// request.
    pub(super) async fn route_unscrapable(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if !request.path.starts_with("/api/listing-sources/")
            || !request.path.ends_with(UNSCRAPABLE_SUFFIX)
        {
            return None;
        }
        if !matches!(request.method.as_str(), "POST" | "DELETE") {
            return None;
        }
        let Some(listing_source_id) =
            parse_listing_source_id_with_suffix(request.path, UNSCRAPABLE_SUFFIX)
        else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        Some(if request.method == "POST" {
            self.mark_unscrapable(listing_source_id, actor, request.body)
                .await
        } else {
            verdict_answer(
                self.repository
                    .clear_unscrapable(&listing_source_id, actor)
                    .await,
            )
        })
    }

    async fn mark_unscrapable(
        &self,
        listing_source_id: ListingSourceId,
        actor: ReviewActor,
        body: &str,
    ) -> HttpResponse {
        let mark: UnscrapableMark = match serde_json::from_str(body) {
            Ok(mark) => mark,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error.to_string() })),
        };
        match self
            .repository
            .mark_unscrapable(&listing_source_id, actor, &mark)
            .await
        {
            Ok(label) => HttpResponse::json(200, &label),
            Err(error) => review_error(error),
        }
    }
}
