//! The console's route for a human's request to edit a ListingSource's live URL pattern or
//! product schemas through a review. The contract is in `docs/crawler/extraction-and-review.md`
//! (human edits of the live configuration) and `docs/crawler/operations.md` (review service).

use super::{ReviewServer, parse_listing_source_id_with_suffix, review_error};
use crate::review::http::{HttpResponse, ParsedRequest};
use crate::review::model::{ArtifactType, QueryValue, ReviewActor};
use crate::review::repository::ReviewPlacement;
use serde::Deserialize;
use serde_json::json;
use tracing::info;

const EDIT_REVIEW_SUFFIX: &str = "/edit-review";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditReviewPayload {
    artifact_type: String,
}

/// The artifact a request asks to edit, or the refusal that names the accepted ones.
fn edited_artifact(body: &str) -> Result<ArtifactType, String> {
    let payload =
        serde_json::from_str::<EditReviewPayload>(body).map_err(|error| error.to_string())?;
    ArtifactType::parse(&payload.artifact_type).ok_or_else(|| {
        format!(
            "artifact_type `{}` is none of {}",
            payload.artifact_type,
            ArtifactType::accepted()
        )
    })
}

impl ReviewServer {
    /// Answers `POST /api/listing-sources/{id}/edit-review`, or `None` for any other request: the
    /// review of the ListingSource's live `artifact_type` that a human edits, opened now or already
    /// pending, as `review_id` with `opened`. Only a human opens one: the console's access answers
    /// the agents' tokens `401` on this route before it is routed, and the repository refuses any
    /// other actor too.
    pub(super) async fn route_human_edit_review(
        &self,
        request: &ParsedRequest<'_>,
        actor: ReviewActor,
    ) -> Option<HttpResponse> {
        if request.method != "POST"
            || !request.path.starts_with("/api/listing-sources/")
            || !request.path.ends_with(EDIT_REVIEW_SUFFIX)
        {
            return None;
        }
        let Some(listing_source_id) =
            parse_listing_source_id_with_suffix(request.path, EDIT_REVIEW_SUFFIX)
        else {
            return Some(HttpResponse::json(
                400,
                &json!({ "error": "invalid ListingSource id" }),
            ));
        };
        let artifact = match edited_artifact(request.body) {
            Ok(artifact) => artifact,
            Err(error) => return Some(HttpResponse::json(400, &json!({ "error": error }))),
        };
        Some(
            match self
                .repository
                .open_human_edit_review(&listing_source_id, artifact, actor)
                .await
            {
                Ok(placement) => {
                    let opened = matches!(placement, ReviewPlacement::Opened(_));
                    info!(
                        event = "crawler.review.human_edit_requested",
                        listing_source_id = %listing_source_id,
                        artifact_type = artifact.as_str(),
                        review_id = %placement.review_id(),
                        opened,
                        "A human asked to edit the live configuration through a review"
                    );
                    HttpResponse::json(
                        200,
                        &json!({ "review_id": placement.review_id(), "opened": opened }),
                    )
                }
                Err(error) => review_error(error),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case::product_schemas(r#"{"artifact_type":"PRODUCT_SCHEMA"}"#, ArtifactType::ProductSchema)]
    #[case::url_pattern(r#"{"artifact_type":"URL_PATTERN"}"#, ArtifactType::UrlPattern)]
    fn should_take_the_artifact_type_to_edit(#[case] body: &str, #[case] expected: ArtifactType) {
        assert_eq!(edited_artifact(body), Ok(expected));
    }

    /// An artifact type that is not one, a missing one, another key and a body that is no JSON
    /// object are refused rather than read as some artifact.
    #[rstest::rstest]
    #[case::unknown_artifact(r#"{"artifact_type":"REMOVED_PAGE"}"#)]
    #[case::lower_case_artifact(r#"{"artifact_type":"url_pattern"}"#)]
    #[case::missing_artifact("{}")]
    #[case::extra_key(r#"{"artifact_type":"URL_PATTERN","note":"x"}"#)]
    #[case::no_body("")]
    fn should_refuse_a_request_that_names_no_artifact_type(#[case] body: &str) {
        assert!(edited_artifact(body).is_err(), "{body}");
    }

    #[test]
    fn should_name_the_accepted_artifact_types_when_refusing_one() {
        assert_eq!(
            edited_artifact(r#"{"artifact_type":"SCHEMA"}"#),
            Err("artifact_type `SCHEMA` is none of URL_PATTERN, PRODUCT_SCHEMA".to_owned())
        );
    }
}
