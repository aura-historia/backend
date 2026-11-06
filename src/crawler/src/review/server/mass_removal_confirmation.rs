//! The console's routes for an operator's confirmation of a ListingSource's mass removal: a human
//! confirms it for a bounded time or revokes it; the mass-removal guard reads it per batch.
//! The contract is in `docs/crawler/scheduling-and-retries.md` (mass-removal guard).

use super::{ReviewServer, parse_listing_source_id_with_suffix, review_error, verdict_answer};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::ReviewActor;
use crate::scraper::removal_guard::MassRemovalConfirmationHours;
use listing_source_core::ListingSourceId;
use serde::Deserialize;
use serde_json::json;

const CONFIRMATION_SUFFIX: &str = "/mass-removal-confirmation";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MassRemovalConfirmationPayload {
    hours: u32,
}

impl ReviewServer {
    /// Answers `POST` and `DELETE /api/listing-sources/{id}/mass-removal-confirmation`, or `None`
    /// for any other request.
    pub(super) async fn route_mass_removal_confirmation(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if !request.path.starts_with("/api/listing-sources/")
            || !request.path.ends_with(CONFIRMATION_SUFFIX)
        {
            return None;
        }
        if !matches!(request.method.as_str(), "POST" | "DELETE") {
            return None;
        }
        let Some(listing_source_id) =
            parse_listing_source_id_with_suffix(request.path, CONFIRMATION_SUFFIX)
        else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        Some(if request.method == "POST" {
            self.confirm_mass_removal(listing_source_id, actor, request.body)
                .await
        } else {
            verdict_answer(
                self.repository
                    .revoke_mass_removal_confirmation(&listing_source_id, actor)
                    .await,
            )
        })
    }

    async fn confirm_mass_removal(
        &self,
        listing_source_id: ListingSourceId,
        actor: ReviewActor,
        body: &str,
    ) -> HttpResponse {
        let payload: MassRemovalConfirmationPayload = match serde_json::from_str(body) {
            Ok(payload) => payload,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error.to_string() })),
        };
        let hours = match MassRemovalConfirmationHours::try_from_hours(payload.hours) {
            Ok(hours) => hours,
            Err(error) => return HttpResponse::json(400, &json!({ "error": error.to_string() })),
        };
        match self
            .repository
            .confirm_mass_removal(&listing_source_id, actor, hours)
            .await
        {
            Ok(confirmation) => HttpResponse::json(200, &confirmation),
            Err(error) => review_error(error),
        }
    }
}
