//! The console's route for a human's request that the live schema audit reopen a ListingSource
//! for the review agents, with a note of what to repair. The contract is in
//! `docs/crawler/extraction-and-review.md` (live schema audit).

use super::{ReviewServer, parse_listing_source_id_with_suffix, review_error};
use crate::authoring::CrawlerAuthoringMode;
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::ReviewActor;
use crate::review::repository::{ReviewRepositoryError, SchemaAuditRequestNote};
use serde::Deserialize;
use serde_json::json;
use tracing::info;

const SCHEMA_AUDIT_SUFFIX: &str = "/schema-audit";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SchemaAuditRequestPayload {
    note: String,
}

impl ReviewServer {
    /// Answers `POST /api/listing-sources/{id}/schema-audit`, or `None` for any other request.
    /// Only a human requests an audit: the console's access answers the agents' tokens `401` on
    /// this route before it is routed. Outside `agent` authoring mode no audit runs, so a request
    /// is refused rather than left waiting.
    pub(super) async fn route_schema_audit_request(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if request.method != "POST"
            || !request.path.starts_with("/api/listing-sources/")
            || !request.path.ends_with(SCHEMA_AUDIT_SUFFIX)
        {
            return None;
        }
        let Some(listing_source_id) =
            parse_listing_source_id_with_suffix(request.path, SCHEMA_AUDIT_SUFFIX)
        else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        let note = match serde_json::from_str::<SchemaAuditRequestPayload>(request.body)
            .map_err(|error| error.to_string())
            .and_then(|payload| {
                SchemaAuditRequestNote::parse(&payload.note).map_err(|error| error.to_string())
            }) {
            Ok(note) => note,
            Err(error) => return Some(HttpResponse::json(400, &json!({ "error": error }))),
        };
        if self.repository.agent_decisions().authoring() != CrawlerAuthoringMode::Agent {
            return Some(review_error(ReviewRepositoryError::SchemaAuditNotPossible(
                listing_source_id,
                "the live schema audit runs in agent authoring mode only",
            )));
        }
        Some(
            match self
                .repository
                .request_schema_audit(&listing_source_id, &note)
                .await
            {
                Ok(()) => {
                    info!(
                        event = "crawler.schema_audit.requested",
                        listing_source_id = %listing_source_id,
                        actor = actor.as_str(),
                        "A schema audit was requested; the audit takes the source in its next pass"
                    );
                    HttpResponse::json(200, &json!({ "requested": true }))
                }
                Err(error) => review_error(error),
            },
        )
    }
}
