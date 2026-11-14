use content_notice_core::{
    bounded_text::BoundedTextError,
    content_notice_id::ContentNoticeId,
    content_notice_target::{ContentNoticeTargetType, ProductListingTitleSnapshot},
    reason_category::ReasonCategory,
};
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::{ListingSourceName, ListingSourceNameError};
use notification_core::external_email::{
    ContentNoticeReceipt, ContentNoticeReceiptTarget, ExternalEmailContent, ExternalEmailTemplate,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use time::OffsetDateTime;
use uuid::Uuid;

pub(crate) const PAYLOAD_VERSION: i32 = 1;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ExternalEmailMappingError {
    #[error("unknown external email template {0}")]
    UnknownTemplate(String),
    #[error("unsupported external email payload version {0}")]
    UnsupportedPayloadVersion(i32),
    #[error("external email payload serialization failed")]
    PayloadSerialization(#[source] serde_json::Error),
    #[error("external email payload is invalid")]
    InvalidPayload(#[source] serde_json::Error),
    #[error("content notice receipt payload contains an invalid content notice ID")]
    InvalidContentNoticeId(#[source] ObjectIdError),
    #[error("content notice receipt payload contains an invalid provider name")]
    InvalidProviderName(#[source] ListingSourceNameError),
    #[error("content notice receipt payload contains an invalid listing title")]
    InvalidListingTitle(#[source] BoundedTextError),
    #[error("content notice receipt payload contains an unknown reason category")]
    UnknownReasonCategory,
    #[error("content notice receipt payload contains an unknown target type")]
    UnknownTargetType,
    #[error("content notice receipt payload names a listing title for a listing source")]
    ListingTitleForListingSource,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContentNoticeReceiptPayloadV1 {
    content_notice_id: Uuid,
    #[serde(with = "time::serde::rfc3339")]
    received_at: OffsetDateTime,
    provider_name: String,
    target_type: String,
    /// Only for a `PRODUCT_LISTING` target that had a title.
    listing_title: Option<String>,
    reason_category: String,
}

pub(crate) fn encode_payload(
    content: &ExternalEmailContent,
) -> Result<Value, ExternalEmailMappingError> {
    match content {
        ExternalEmailContent::ContentNoticeReceipt(receipt) => {
            let (target_type, listing_title) = match &receipt.target {
                ContentNoticeReceiptTarget::ListingSource => {
                    (ContentNoticeTargetType::ListingSource, None)
                }
                ContentNoticeReceiptTarget::ProductListing { title } => (
                    ContentNoticeTargetType::ProductListing,
                    title.as_ref().map(|title| title.as_str().to_owned()),
                ),
            };
            serde_json::to_value(ContentNoticeReceiptPayloadV1 {
                content_notice_id: receipt.content_notice_id.into_uuid(),
                received_at: receipt.received_at,
                provider_name: receipt.provider_name.as_ref().to_owned(),
                target_type: target_type.as_str().to_owned(),
                listing_title,
                reason_category: receipt.reason_category.as_str().to_owned(),
            })
        }
    }
    .map_err(ExternalEmailMappingError::PayloadSerialization)
}

pub(crate) fn decode_content(
    template: &str,
    payload_version: i32,
    payload: Value,
) -> Result<ExternalEmailContent, ExternalEmailMappingError> {
    let template = ExternalEmailTemplate::from_code(template)
        .ok_or_else(|| ExternalEmailMappingError::UnknownTemplate(template.to_owned()))?;
    if payload_version != PAYLOAD_VERSION {
        return Err(ExternalEmailMappingError::UnsupportedPayloadVersion(
            payload_version,
        ));
    }
    match template {
        ExternalEmailTemplate::ContentNoticeReceipt => {
            let payload = serde_json::from_value::<ContentNoticeReceiptPayloadV1>(payload)
                .map_err(ExternalEmailMappingError::InvalidPayload)?;
            content_notice_receipt(payload).map(ExternalEmailContent::ContentNoticeReceipt)
        }
    }
}

fn content_notice_receipt(
    payload: ContentNoticeReceiptPayloadV1,
) -> Result<ContentNoticeReceipt, ExternalEmailMappingError> {
    Ok(ContentNoticeReceipt {
        content_notice_id: ContentNoticeId::try_from(payload.content_notice_id)
            .map_err(ExternalEmailMappingError::InvalidContentNoticeId)?,
        received_at: payload.received_at,
        provider_name: ListingSourceName::try_from(payload.provider_name)
            .map_err(ExternalEmailMappingError::InvalidProviderName)?,
        target: receipt_target(&payload.target_type, payload.listing_title)?,
        reason_category: ReasonCategory::from_code(&payload.reason_category)
            .ok_or(ExternalEmailMappingError::UnknownReasonCategory)?,
    })
}

fn receipt_target(
    target_type: &str,
    listing_title: Option<String>,
) -> Result<ContentNoticeReceiptTarget, ExternalEmailMappingError> {
    match ContentNoticeTargetType::from_code(target_type) {
        Some(ContentNoticeTargetType::ListingSource) => match listing_title {
            None => Ok(ContentNoticeReceiptTarget::ListingSource),
            Some(_) => Err(ExternalEmailMappingError::ListingTitleForListingSource),
        },
        Some(ContentNoticeTargetType::ProductListing) => {
            Ok(ContentNoticeReceiptTarget::ProductListing {
                title: listing_title
                    .map(ProductListingTitleSnapshot::try_from)
                    .transpose()
                    .map_err(ExternalEmailMappingError::InvalidListingTitle)?,
            })
        }
        None => Err(ExternalEmailMappingError::UnknownTargetType),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt(target: ContentNoticeReceiptTarget) -> ExternalEmailContent {
        ExternalEmailContent::ContentNoticeReceipt(ContentNoticeReceipt {
            content_notice_id: ContentNoticeId::try_from(
                Uuid::parse_str("01890a5d-ac96-774b-bf1d-d5586c639f75")
                    .unwrap_or_else(|error| panic!("invalid test uuid: {error}")),
            )
            .unwrap_or_else(|error| panic!("invalid test id: {error}")),
            received_at: time::macros::datetime!(2026-11-14 12:30:05.123 UTC),
            provider_name: ListingSourceName::try_from("Antik Shop")
                .unwrap_or_else(|error| panic!("invalid test name: {error}")),
            target,
            reason_category: ReasonCategory::RightsInfringement,
        })
    }

    fn listing(title: Option<&str>) -> ContentNoticeReceiptTarget {
        ContentNoticeReceiptTarget::ProductListing {
            title: title.map(|title| {
                ProductListingTitleSnapshot::try_from(title)
                    .unwrap_or_else(|error| panic!("invalid test title: {error}"))
            }),
        }
    }

    fn receipt_payload() -> Value {
        json!({
            "content_notice_id": "01890a5d-ac96-774b-bf1d-d5586c639f75",
            "received_at": "2026-11-14T12:30:05.123Z",
            "provider_name": "Antik Shop",
            "target_type": "PRODUCT_LISTING",
            "listing_title": "Jugendstil-Vase",
            "reason_category": "RIGHTS_INFRINGEMENT",
        })
    }

    #[test]
    fn should_reject_unknown_template() {
        assert!(matches!(
            decode_content("UNKNOWN", PAYLOAD_VERSION, receipt_payload()),
            Err(ExternalEmailMappingError::UnknownTemplate(_))
        ));
    }

    #[test]
    fn should_reject_unsupported_payload_version() {
        assert!(matches!(
            decode_content("CONTENT_NOTICE_RECEIPT", 2, receipt_payload()),
            Err(ExternalEmailMappingError::UnsupportedPayloadVersion(2))
        ));
    }

    #[test]
    fn should_round_trip_content_notice_receipt_payloads_of_every_target_kind()
    -> Result<(), ExternalEmailMappingError> {
        let titled = receipt(listing(Some("Jugendstil-Vase")));
        let untitled = receipt(listing(None));
        let source = receipt(ContentNoticeReceiptTarget::ListingSource);

        assert_eq!(receipt_payload(), encode_payload(&titled)?);
        let mut source_payload = receipt_payload();
        source_payload["target_type"] = json!("LISTING_SOURCE");
        source_payload["listing_title"] = json!(null);
        assert_eq!(source_payload, encode_payload(&source)?);
        for content in [titled, untitled, source] {
            let payload = encode_payload(&content)?;
            assert_eq!(
                content,
                decode_content("CONTENT_NOTICE_RECEIPT", PAYLOAD_VERSION, payload)?
            );
        }
        Ok(())
    }

    #[test]
    fn should_fail_closed_on_invalid_content_notice_receipt_values() {
        let with = |field: &str, value: Value| {
            let mut payload = receipt_payload();
            payload[field] = value;
            decode_content("CONTENT_NOTICE_RECEIPT", PAYLOAD_VERSION, payload)
        };

        assert!(matches!(
            with(
                "content_notice_id",
                json!("2b6f0cc9-04f8-4c0e-8f5c-2f1a8a3f5e10")
            ),
            Err(ExternalEmailMappingError::InvalidContentNoticeId(_))
        ));
        assert!(matches!(
            with("provider_name", json!(" ")),
            Err(ExternalEmailMappingError::InvalidProviderName(_))
        ));
        assert!(matches!(
            with("listing_title", json!("Vase\nZwei")),
            Err(ExternalEmailMappingError::InvalidListingTitle(_))
        ));
        assert!(matches!(
            with("reason_category", json!("rights_infringement")),
            Err(ExternalEmailMappingError::UnknownReasonCategory)
        ));
        assert!(matches!(
            with("target_type", json!("AUCTION")),
            Err(ExternalEmailMappingError::UnknownTargetType)
        ));
        assert!(matches!(
            with("target_type", json!("LISTING_SOURCE")),
            Err(ExternalEmailMappingError::ListingTitleForListingSource)
        ));
        assert!(matches!(
            decode_content("CONTENT_NOTICE_RECEIPT", PAYLOAD_VERSION, json!({})),
            Err(ExternalEmailMappingError::InvalidPayload(_))
        ));
        for (field, value) in [
            ("received_at", json!("yesterday")),
            ("reason_category", json!(null)),
            ("target_type", json!(null)),
            ("explanation", json!("submitter text")),
        ] {
            assert!(
                matches!(
                    with(field, value),
                    Err(ExternalEmailMappingError::InvalidPayload(_))
                ),
                "{field}"
            );
        }
    }
}
