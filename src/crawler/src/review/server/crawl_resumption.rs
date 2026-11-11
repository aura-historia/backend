//! The console's route for an operator's resumption of a ListingSource whose crawl waits for an
//! operator. The contract is in `docs/crawler/operations.md` (review service).

use super::{ReviewServer, parse_listing_source_id_with_suffix, review_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::ReviewActor;
use serde_json::json;

const RESUMPTION_SUFFIX: &str = "/crawl-resumption";

impl ReviewServer {
    /// Answers `POST /api/listing-sources/{id}/crawl-resumption`, or `None` for any other request.
    pub(super) async fn route_crawl_resumption(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if request.method != "POST"
            || !request.path.starts_with("/api/listing-sources/")
            || !request.path.ends_with(RESUMPTION_SUFFIX)
        {
            return None;
        }
        let Some(listing_source_id) =
            parse_listing_source_id_with_suffix(request.path, RESUMPTION_SUFFIX)
        else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        Some(
            match self
                .repository
                .resume_crawl(&listing_source_id, actor)
                .await
            {
                Ok(resumption) => HttpResponse::json(200, &resumption),
                Err(error) => review_error(error),
            },
        )
    }
}
