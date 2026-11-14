use content_notice_core::{
    content_notice_id::ContentNoticeId,
    content_notice_target::{ContentNoticeTarget, TargetSnapshot},
    submitter::Submitter,
};
use content_notice_service::use_cases::queries::{
    get_admin_content_notice::{
        AdminContentNoticeDetails, CurrentContentNoticeTarget, CurrentListingSource,
        CurrentProductListing,
    },
    list_admin_content_notices::ContentNoticeSummary,
};
use listing_source_core::{ListingIngestionMethod, ListingSourceId};
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

/// One notice for admin review: what was submitted, the target as it was then and the target
/// as it is now. It carries the submitter's contact and free text, so it deliberately has no
/// `Debug`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AdminContentNoticeDetailsData {
    content_notice_id: ContentNoticeId,
    state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    version: u64,
    target: ContentNoticeTargetData,
    target_snapshot: TargetSnapshotData,
    current_target: CurrentTargetData,
    submitter_role: &'static str,
    reason_category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    explanation: Option<String>,
    additional_urls: Vec<String>,
    submitter: SubmitterData,
    good_faith_statement: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetSnapshotData {
    listing_source_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    listing_source_url: Option<String>,
    partner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_listing_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product_listing_url: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentTargetData {
    listing_source: CurrentData<CurrentListingSourceData>,
    /// Present exactly for a notice about a product listing.
    #[serde(skip_serializing_if = "Option::is_none")]
    product_listing: Option<CurrentData<CurrentProductListingData>>,
}

/// A target part as it is now: `exists` and, only while it exists, its current values.
#[derive(Serialize)]
struct CurrentData<T> {
    exists: bool,
    #[serde(flatten)]
    current: Option<T>,
}

impl<T> CurrentData<T> {
    fn of<S>(current: Option<S>, map: impl FnOnce(S) -> T) -> Self {
        Self {
            exists: current.is_some(),
            current: current.map(map),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrentListingSourceData {
    name: String,
    slug_id: String,
    ingestion_methods: Vec<&'static str>,
    partner: bool,
}

#[derive(Serialize)]
struct CurrentProductListingData {
    lifecycle: &'static str,
}

#[derive(Serialize)]
struct SubmitterData {
    name: String,
    email: String,
    language: &'static str,
}

impl From<AdminContentNoticeDetails> for AdminContentNoticeDetailsData {
    fn from(details: AdminContentNoticeDetails) -> Self {
        let notice = details.notice;
        let target = notice.target();
        Self {
            content_notice_id: notice.id(),
            state: notice.state().as_str(),
            created: details.created,
            updated: details.updated,
            version: details.version.into_inner(),
            target: target.into(),
            target_snapshot: notice.target_snapshot().into(),
            current_target: CurrentTargetData::new(target, details.current_target),
            submitter_role: notice.submitter_role().as_str(),
            reason_category: notice.reason_category().as_str(),
            explanation: notice
                .explanation()
                .map(|explanation| explanation.as_str().to_owned()),
            additional_urls: notice
                .additional_urls()
                .as_slice()
                .iter()
                .map(|url| url.as_str().to_owned())
                .collect(),
            submitter: notice.submitter().into(),
            good_faith_statement: notice.good_faith_statement_version().as_str(),
        }
    }
}

impl From<&TargetSnapshot> for TargetSnapshotData {
    fn from(snapshot: &TargetSnapshot) -> Self {
        let listing_source = &snapshot.listing_source;
        let product_listing = snapshot.product_listing.as_ref();
        Self {
            listing_source_name: listing_source.name.as_ref().to_owned(),
            listing_source_url: listing_source
                .url
                .as_ref()
                .map(|url| url.as_str().to_owned()),
            partner: listing_source.partner,
            product_listing_title: product_listing
                .and_then(|listing| listing.title.as_ref())
                .map(|title| title.as_str().to_owned()),
            product_listing_url: product_listing.map(|listing| listing.url.as_str().to_owned()),
        }
    }
}

impl CurrentTargetData {
    fn new(target: ContentNoticeTarget, current: CurrentContentNoticeTarget) -> Self {
        Self {
            listing_source: CurrentData::of(current.listing_source, CurrentListingSourceData::from),
            product_listing: target
                .product_listing_id()
                .map(|_| CurrentData::of(current.product_listing, CurrentProductListingData::from)),
        }
    }
}

impl From<CurrentListingSource> for CurrentListingSourceData {
    fn from(source: CurrentListingSource) -> Self {
        Self {
            name: source.name.as_ref().to_owned(),
            slug_id: source.slug_id.as_ref().to_owned(),
            ingestion_methods: source
                .ingestion_methods
                .into_iter()
                .map(ListingIngestionMethod::as_str)
                .collect(),
            partner: source.partner,
        }
    }
}

impl From<CurrentProductListing> for CurrentProductListingData {
    fn from(listing: CurrentProductListing) -> Self {
        Self {
            lifecycle: listing.lifecycle.as_str(),
        }
    }
}

impl From<&Submitter> for SubmitterData {
    fn from(submitter: &Submitter) -> Self {
        Self {
            name: submitter.name.as_str().to_owned(),
            email: submitter.email.as_str().to_owned(),
            language: submitter.language.as_str(),
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

    mod details {
        use super::super::*;
        use content_notice_core::{
            additional_url::{AdditionalUrl, AdditionalUrls},
            content_notice::{ContentNotice, NewContentNotice},
            content_notice_target::{
                ListingSourceSnapshot, ProductListingSnapshot, ProductListingTitleSnapshot,
            },
            good_faith_statement_version::GoodFaithStatementVersion,
            notice_explanation::NoticeExplanation,
            reason_category::ReasonCategory,
            submitter::{SubmitterEmail, SubmitterName},
            submitter_role::SubmitterRole,
        };
        use content_notice_service::ports::ContentNoticeStorageVersion;
        use listing_source_core::{ListingSourceName, ListingSourceSlugId};
        use localization::Language;
        use product_listing_core::listing_lifecycle::ListingLifecycle;
        use serde_json::{Value, json};
        use time::macros::datetime;
        use url::Url;

        fn details(
            target: ContentNoticeTarget,
            current_target: CurrentContentNoticeTarget,
        ) -> AdminContentNoticeDetails {
            let listing_target = target.product_listing_id().is_some();
            let notice = ContentNotice::submit(NewContentNotice {
                id: ContentNoticeId::new(),
                target,
                target_snapshot: TargetSnapshot {
                    listing_source: ListingSourceSnapshot {
                        name: ListingSourceName::try_from("Antik Shop").unwrap(),
                        url: Some(Url::parse("https://antik.example/").unwrap()),
                        partner: false,
                    },
                    product_listing: listing_target.then(|| ProductListingSnapshot {
                        title: Some(
                            ProductListingTitleSnapshot::try_from("Jugendstil-Vase").unwrap(),
                        ),
                        url: Url::parse("https://antik.example/vase").unwrap(),
                    }),
                },
                submitter_role: if listing_target {
                    SubmitterRole::RightsHolder
                } else {
                    SubmitterRole::Provider
                },
                reason_category: if listing_target {
                    ReasonCategory::RightsInfringement
                } else {
                    ReasonCategory::ProviderOptOut
                },
                explanation: listing_target
                    .then(|| NoticeExplanation::try_from("Das Foto ist meins.").unwrap()),
                additional_urls: AdditionalUrls::try_from(vec![
                    AdditionalUrl::try_from("https://photo.example/vase").unwrap(),
                ])
                .unwrap(),
                submitter: Submitter {
                    name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                    email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
                    language: Language::De,
                },
                good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
            })
            .unwrap();
            AdminContentNoticeDetails {
                notice,
                version: ContentNoticeStorageVersion::default(),
                created: datetime!(2026-11-13 10:44:18.123 UTC),
                updated: datetime!(2026-11-14 08:00 UTC),
                current_target,
            }
        }

        fn current_source() -> CurrentListingSource {
            CurrentListingSource {
                name: ListingSourceName::try_from("Antik Shop GmbH").unwrap(),
                slug_id: ListingSourceSlugId::raw("antik-shop").unwrap(),
                ingestion_methods: vec![ListingIngestionMethod::WebCrawl],
                partner: true,
            }
        }

        fn serialize(details: AdminContentNoticeDetails) -> Value {
            serde_json::to_value(AdminContentNoticeDetailsData::from(details)).unwrap()
        }

        #[test]
        fn should_serialize_product_listing_notice_with_snapshot_and_current_target() {
            let listing_source_id = ListingSourceId::new();
            let product_listing_id = ProductListingId::new();
            let details = details(
                ContentNoticeTarget::ProductListing {
                    product_listing_id,
                    listing_source_id,
                },
                CurrentContentNoticeTarget {
                    listing_source: Some(current_source()),
                    product_listing: Some(CurrentProductListing {
                        lifecycle: ListingLifecycle::Withdrawn,
                    }),
                },
            );
            let id = details.notice.id();

            assert_eq!(
                json!({
                    "contentNoticeId": id.to_string(),
                    "state": "SUBMITTED",
                    "created": "2026-11-13T10:44:18.123Z",
                    "updated": "2026-11-14T08:00:00Z",
                    "version": 1,
                    "target": {
                        "type": "PRODUCT_LISTING",
                        "listingSourceId": listing_source_id.to_string(),
                        "productListingId": product_listing_id.to_string(),
                    },
                    "targetSnapshot": {
                        "listingSourceName": "Antik Shop",
                        "listingSourceUrl": "https://antik.example/",
                        "partner": false,
                        "productListingTitle": "Jugendstil-Vase",
                        "productListingUrl": "https://antik.example/vase",
                    },
                    "currentTarget": {
                        "listingSource": {
                            "exists": true,
                            "name": "Antik Shop GmbH",
                            "slugId": "antik-shop",
                            "ingestionMethods": ["WEB_CRAWL"],
                            "partner": true,
                        },
                        "productListing": {"exists": true, "lifecycle": "WITHDRAWN"},
                    },
                    "submitterRole": "RIGHTS_HOLDER",
                    "reasonCategory": "RIGHTS_INFRINGEMENT",
                    "explanation": "Das Foto ist meins.",
                    "additionalUrls": ["https://photo.example/vase"],
                    "submitter": {
                        "name": "Erika Mustermann",
                        "email": "erika@antik.example",
                        "language": "de",
                    },
                    "goodFaithStatement": "GOOD_FAITH_V1",
                }),
                serialize(details)
            );
        }

        #[test]
        fn should_omit_absent_explanation_listing_fields_and_values_of_gone_targets() {
            let source_notice = serialize(details(
                ContentNoticeTarget::ListingSource {
                    listing_source_id: ListingSourceId::new(),
                },
                CurrentContentNoticeTarget {
                    listing_source: None,
                    product_listing: None,
                },
            ));
            let gone_listing = serialize(details(
                ContentNoticeTarget::ProductListing {
                    product_listing_id: ProductListingId::new(),
                    listing_source_id: ListingSourceId::new(),
                },
                CurrentContentNoticeTarget {
                    listing_source: None,
                    product_listing: None,
                },
            ));

            assert!(source_notice.get("explanation").is_none());
            assert_eq!(
                json!({
                    "listingSourceName": "Antik Shop",
                    "listingSourceUrl": "https://antik.example/",
                    "partner": false,
                }),
                source_notice["targetSnapshot"]
            );
            assert_eq!(
                json!({"listingSource": {"exists": false}}),
                source_notice["currentTarget"]
            );
            assert_eq!(
                json!({
                    "listingSource": {"exists": false},
                    "productListing": {"exists": false},
                }),
                gone_listing["currentTarget"]
            );
        }
    }
}
