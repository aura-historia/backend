use content_notice_core::{
    content_notice_id::ContentNoticeId, content_notice_target::ContentNoticeTarget,
};
use content_notice_service::use_cases::queries::list_admin_content_notices::ContentNoticeSummary;
use listing_source_core::ListingSourceId;
use product_listing_core::product_listing_id::ProductListingId;
use serde::Serialize;
use time::OffsetDateTime;

/// Admin review queue row. It carries the submitter's name and email, so it deliberately has
/// no `Debug`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ContentNoticeSummaryData {
    content_notice_id: ContentNoticeId,
    state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    target: ContentNoticeTargetData,
    listing_source_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_listing_title: Option<String>,
    partner: bool,
    submitter_role: &'static str,
    reason_category: &'static str,
    submitter_name: String,
    submitter_email: String,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
pub(super) enum ContentNoticeTargetData {
    #[serde(rename = "LISTING_SOURCE")]
    ListingSource { listing_source_id: ListingSourceId },
    #[serde(rename = "PRODUCT_LISTING")]
    ProductListing {
        listing_source_id: ListingSourceId,
        product_listing_id: ProductListingId,
    },
}

impl From<ContentNoticeTarget> for ContentNoticeTargetData {
    fn from(target: ContentNoticeTarget) -> Self {
        match target {
            ContentNoticeTarget::ListingSource { listing_source_id } => {
                Self::ListingSource { listing_source_id }
            }
            ContentNoticeTarget::ProductListing {
                product_listing_id,
                listing_source_id,
            } => Self::ProductListing {
                listing_source_id,
                product_listing_id,
            },
        }
    }
}

impl From<ContentNoticeSummary> for ContentNoticeSummaryData {
    fn from(summary: ContentNoticeSummary) -> Self {
        Self {
            content_notice_id: summary.id,
            state: summary.state.as_str(),
            created: summary.created,
            target: summary.target.into(),
            listing_source_name: summary.listing_source_name.as_ref().to_owned(),
            product_listing_title: summary
                .product_listing_title
                .map(|title| title.as_str().to_owned()),
            partner: summary.listing_source_partner,
            submitter_role: summary.submitter_role.as_str(),
            reason_category: summary.reason_category.as_str(),
            submitter_name: summary.submitter_name.as_str().to_owned(),
            submitter_email: summary.submitter_email.as_str().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use content_notice_core::{
        content_notice_state::ContentNoticeState,
        content_notice_target::ProductListingTitleSnapshot,
        reason_category::ReasonCategory,
        submitter::{SubmitterEmail, SubmitterName},
        submitter_role::SubmitterRole,
    };
    use listing_source_core::ListingSourceName;
    use serde_json::json;
    use time::macros::datetime;

    fn summary(target: ContentNoticeTarget) -> ContentNoticeSummary {
        ContentNoticeSummary {
            id: ContentNoticeId::new(),
            state: ContentNoticeState::Submitted,
            created: datetime!(2026-11-13 10:44:18.123 UTC),
            target,
            listing_source_name: ListingSourceName::try_from("Antik Shop").unwrap(),
            product_listing_title: target
                .product_listing_id()
                .map(|_| ProductListingTitleSnapshot::try_from("Jugendstil-Vase").unwrap()),
            listing_source_partner: false,
            submitter_role: SubmitterRole::Provider,
            reason_category: ReasonCategory::ProviderOptOut,
            submitter_name: SubmitterName::try_from("Erika Mustermann").unwrap(),
            submitter_email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
        }
    }

    #[test]
    fn should_serialize_product_listing_summary() {
        let listing_source_id = ListingSourceId::new();
        let product_listing_id = ProductListingId::new();
        let summary = summary(ContentNoticeTarget::ProductListing {
            product_listing_id,
            listing_source_id,
        });
        let id = summary.id;

        assert_eq!(
            json!({
                "contentNoticeId": id.to_string(),
                "state": "SUBMITTED",
                "created": "2026-11-13T10:44:18.123Z",
                "target": {
                    "type": "PRODUCT_LISTING",
                    "listingSourceId": listing_source_id.to_string(),
                    "productListingId": product_listing_id.to_string(),
                },
                "listingSourceName": "Antik Shop",
                "productListingTitle": "Jugendstil-Vase",
                "partner": false,
                "submitterRole": "PROVIDER",
                "reasonCategory": "PROVIDER_OPT_OUT",
                "submitterName": "Erika Mustermann",
                "submitterEmail": "erika@antik.example",
            }),
            serde_json::to_value(ContentNoticeSummaryData::from(summary)).unwrap()
        );
    }

    #[test]
    fn should_omit_listing_fields_for_listing_source_summary() {
        let listing_source_id = ListingSourceId::new();

        let data = serde_json::to_value(ContentNoticeSummaryData::from(summary(
            ContentNoticeTarget::ListingSource { listing_source_id },
        )))
        .unwrap();

        assert_eq!(
            json!({"type": "LISTING_SOURCE", "listingSourceId": listing_source_id.to_string()}),
            data["target"]
        );
        assert!(data.get("productListingTitle").is_none());
    }
}
