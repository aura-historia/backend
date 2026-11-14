use crate::error::{
    ApiError, CONTENT_NOTICE_GOOD_FAITH_NOT_CONFIRMED, CONTENT_NOTICE_INVALID_FIELD,
};
use crate::wire::parse_body_object_id;
use content_notice_core::{
    additional_url::{AdditionalUrl, AdditionalUrls, AdditionalUrlsError},
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_service::use_cases::commands::submit_content_notice::{
    ContentNoticeTargetReference, SubmitContentNoticeCommand,
};
use localization::Language;
use serde::Deserialize;
use std::borrow::Cow;

/// The good-faith statement the submission form shows. A changed wording gets a new version
/// here and in the form.
const CONFIRMED_GOOD_FAITH_STATEMENT: GoodFaithStatementVersion =
    GoodFaithStatementVersion::GoodFaithV1;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SubmitContentNoticeData {
    target: ContentNoticeTargetData,
    submitter_role: SubmitterRoleData,
    reason_category: ReasonCategoryData,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    additional_urls: Vec<String>,
    name: String,
    email: String,
    #[serde(with = "crate::wire::language")]
    language: Language,
    /// Optional on the wire so that a missing confirmation is a field error, like `false`.
    #[serde(default)]
    good_faith_confirmed: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum ContentNoticeTargetData {
    ProductListing { product_listing_id: String },
    ListingSource { listing_source_id: String },
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum SubmitterRoleData {
    Provider,
    RightsHolder,
    IllegalContentReporter,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ReasonCategoryData {
    ProviderOptOut,
    RightsInfringement,
    IllegalContent,
    IncorrectInformation,
    Other,
}

impl TryFrom<SubmitContentNoticeData> for SubmitContentNoticeCommand {
    type Error = ApiError;

    /// Builds the command through the domain constructors. A rejected value is reported with
    /// the JSON pointer of the field and a detail that never repeats the value.
    fn try_from(data: SubmitContentNoticeData) -> Result<Self, Self::Error> {
        let target = target(data.target)?;
        let explanation = data
            .explanation
            .map(NoticeExplanation::try_from)
            .transpose()
            .map_err(|error| invalid_field("/explanation", error))?;
        let additional_urls = additional_urls(data.additional_urls)?;
        let name =
            SubmitterName::try_from(data.name).map_err(|error| invalid_field("/name", error))?;
        let email =
            SubmitterEmail::try_from(data.email).map_err(|error| invalid_field("/email", error))?;
        if data.good_faith_confirmed != Some(true) {
            return Err(
                ApiError::bad_request(CONTENT_NOTICE_GOOD_FAITH_NOT_CONFIRMED)
                    .with_body_field("/goodFaithConfirmed")
                    .with_detail("The good-faith statement must be confirmed with true."),
            );
        }
        Ok(Self {
            target,
            submitter_role: data.submitter_role.into(),
            reason_category: data.reason_category.into(),
            explanation,
            additional_urls,
            submitter: Submitter {
                name,
                email,
                language: data.language,
            },
            good_faith_statement_version: CONFIRMED_GOOD_FAITH_STATEMENT,
        })
    }
}

fn target(data: ContentNoticeTargetData) -> Result<ContentNoticeTargetReference, ApiError> {
    Ok(match data {
        ContentNoticeTargetData::ProductListing { product_listing_id } => {
            ContentNoticeTargetReference::ProductListing {
                product_listing_id: parse_body_object_id(
                    &product_listing_id,
                    "/target/productListingId",
                    "ProductListing",
                )?,
            }
        }
        ContentNoticeTargetData::ListingSource { listing_source_id } => {
            ContentNoticeTargetReference::ListingSource {
                listing_source_id: parse_body_object_id(
                    &listing_source_id,
                    "/target/listingSourceId",
                    "ListingSource",
                )?,
            }
        }
    })
}

fn additional_urls(values: Vec<String>) -> Result<AdditionalUrls, ApiError> {
    let urls = values
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            AdditionalUrl::try_from(value)
                .map_err(|error| invalid_field(format!("/additionalUrls/{index}"), error))
        })
        .collect::<Result<Vec<_>, _>>()?;
    AdditionalUrls::try_from(urls).map_err(|error| match error {
        AdditionalUrlsError::Duplicate { index } => {
            invalid_field(format!("/additionalUrls/{index}"), error)
        }
        AdditionalUrlsError::TooMany { .. } => invalid_field("/additionalUrls", error),
    })
}

fn invalid_field(pointer: impl Into<Cow<'static, str>>, error: impl std::fmt::Display) -> ApiError {
    ApiError::bad_request(CONTENT_NOTICE_INVALID_FIELD)
        .with_body_field(pointer)
        .with_detail(error.to_string())
}

impl From<SubmitterRoleData> for SubmitterRole {
    fn from(data: SubmitterRoleData) -> Self {
        match data {
            SubmitterRoleData::Provider => Self::Provider,
            SubmitterRoleData::RightsHolder => Self::RightsHolder,
            SubmitterRoleData::IllegalContentReporter => Self::IllegalContentReporter,
        }
    }
}

impl From<ReasonCategoryData> for ReasonCategory {
    fn from(data: ReasonCategoryData) -> Self {
        match data {
            ReasonCategoryData::ProviderOptOut => Self::ProviderOptOut,
            ReasonCategoryData::RightsInfringement => Self::RightsInfringement,
            ReasonCategoryData::IllegalContent => Self::IllegalContent,
            ReasonCategoryData::IncorrectInformation => Self::IncorrectInformation,
            ReasonCategoryData::Other => Self::Other,
        }
    }
}
