//! The console's routes for a human's label of a ListingSource the crawler should not crawl: a
//! human sets the label with an optional note, or clears it. The contract is in
//! `docs/crawler/operations.md` (review service).

use super::{ReviewServer, parse_listing_source_id_with_suffix, review_error, verdict_answer};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{ReviewActor, SourceLabelRequest};
use listing_source_core::ListingSourceId;
use serde_json::json;

const LABEL_SUFFIX: &str = "/label";

impl ReviewServer {
    /// Answers `PUT` and `DELETE /api/listing-sources/{id}/label`, or `None` for any other
    /// request.
    pub(super) async fn route_source_label(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if !request.path.starts_with("/api/listing-sources/")
            || !request.path.ends_with(LABEL_SUFFIX)
            || !matches!(request.method.as_str(), "PUT" | "DELETE")
        {
            return None;
        }
        let Some(listing_source_id) =
            parse_listing_source_id_with_suffix(request.path, LABEL_SUFFIX)
        else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        Some(if request.method == "PUT" {
            self.label_source(listing_source_id, actor, request.body)
                .await
        } else {
            verdict_answer(
                self.repository
                    .clear_source_label(&listing_source_id, actor)
                    .await,
            )
        })
    }

    async fn label_source(
        &self,
        listing_source_id: ListingSourceId,
        actor: ReviewActor,
        body: &str,
    ) -> HttpResponse {
        let request: SourceLabelRequest = match serde_json::from_str(body) {
            Ok(request) => request,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error.to_string() })),
        };
        match self
            .repository
            .label_source(&listing_source_id, actor, &request)
            .await
        {
            Ok(applied) => HttpResponse::json(200, &applied),
            Err(error) => review_error(error),
        }
    }
}
