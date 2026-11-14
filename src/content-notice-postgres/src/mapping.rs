use content_notice_core::{
    additional_url::{AdditionalUrl, AdditionalUrlError, AdditionalUrls, AdditionalUrlsError},
    bounded_text::BoundedTextError,
    content_notice::{ContentNotice, ContentNoticeInvariantError, RehydratedContentNoticeState},
    content_notice_id::ContentNoticeId,
    content_notice_state::ContentNoticeState,
    content_notice_target::{
        ContentNoticeTarget, ListingSourceSnapshot, ProductListingSnapshot,
        ProductListingTitleSnapshot, TargetSnapshot,
    },
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterEmailError, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_service::ports::{ContentNoticeStorageVersion, VersionedContentNotice};
use domain_primitives::{object_id::ObjectIdError, versioned::Versioned};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceNameError};
use localization::Language;
use product_listing_core::product_listing_id::ProductListingId;
use url::Url;

pub(crate) const CONTENT_NOTICE_COLUMNS: &str = "content_notice_id, target_type, listing_source_id, product_listing_id, listing_source_name, listing_source_url, listing_source_partner, product_listing_title, product_listing_url, submitter_role, reason_category, explanation, additional_urls, submitter_name, submitter_email, submitter_language, good_faith_statement_version, business_state, version";

const LISTING_SOURCE_TARGET: &str = "LISTING_SOURCE";
const PRODUCT_LISTING_TARGET: &str = "PRODUCT_LISTING";

#[derive(sqlx::FromRow)]
pub(crate) struct ContentNoticeRow {
    content_notice_id: uuid::Uuid,
    target_type: String,
    listing_source_id: uuid::Uuid,
    product_listing_id: Option<uuid::Uuid>,
    listing_source_name: String,
    listing_source_url: Option<String>,
    listing_source_partner: bool,
    product_listing_title: Option<String>,
    product_listing_url: Option<String>,
    submitter_role: String,
    reason_category: String,
    explanation: Option<String>,
    additional_urls: Vec<String>,
    submitter_name: String,
    submitter_email: String,
    submitter_language: String,
    good_faith_statement_version: String,
    business_state: String,
    version: i64,
}

/// Why a stored row is not a valid notice. Variants never carry the stored values, which can
/// be personal data.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MappingError {
    #[error("invalid ContentNotice ID persisted")]
    ContentNoticeId(#[source] ObjectIdError),
    #[error("invalid target ListingSource ID persisted")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid target ProductListing ID persisted")]
    ProductListingId(#[source] ObjectIdError),
    #[error("invalid content notice target persisted")]
    Target,
    #[error("invalid listing source name snapshot persisted")]
    ListingSourceName(#[source] ListingSourceNameError),
    #[error("invalid listing source URL snapshot persisted")]
    ListingSourceUrl(#[source] url::ParseError),
    #[error("invalid product listing title snapshot persisted")]
    ProductListingTitle(#[source] BoundedTextError),
    #[error("invalid product listing URL snapshot persisted")]
    ProductListingUrl(#[source] url::ParseError),
    #[error("invalid submitter role persisted")]
    SubmitterRole,
    #[error("invalid reason category persisted")]
    ReasonCategory,
    #[error("invalid explanation persisted")]
    Explanation(#[source] BoundedTextError),
    #[error("invalid additional URL persisted")]
    AdditionalUrl(#[source] AdditionalUrlError),
    #[error("invalid additional URL list persisted")]
    AdditionalUrls(#[source] AdditionalUrlsError),
    #[error("invalid submitter name persisted")]
    SubmitterName(#[source] BoundedTextError),
    #[error("invalid submitter email persisted")]
    SubmitterEmail(#[source] SubmitterEmailError),
    #[error("invalid submitter language persisted")]
    SubmitterLanguage,
    #[error("invalid good-faith statement version persisted")]
    GoodFaithStatementVersion,
    #[error("invalid content notice state persisted")]
    State,
    #[error("invalid content notice version persisted")]
    Version,
    #[error("persisted content notice breaks an invariant")]
    Invariant(#[source] ContentNoticeInvariantError),
}

pub(crate) fn target_type(target: ContentNoticeTarget) -> &'static str {
    match target {
        ContentNoticeTarget::ListingSource { .. } => LISTING_SOURCE_TARGET,
        ContentNoticeTarget::ProductListing { .. } => PRODUCT_LISTING_TARGET,
    }
}

pub(crate) fn content_notice(
    row: ContentNoticeRow,
) -> Result<VersionedContentNotice, MappingError> {
    let version =
        ContentNoticeStorageVersion::try_from(row.version).map_err(|_| MappingError::Version)?;
    let target = target(&row)?;
    let target_snapshot = target_snapshot(&row)?;
    let notice = ContentNotice::rehydrate(RehydratedContentNoticeState {
        id: ContentNoticeId::try_from(row.content_notice_id)
            .map_err(MappingError::ContentNoticeId)?,
        target,
        target_snapshot,
        submitter_role: SubmitterRole::from_code(&row.submitter_role)
            .ok_or(MappingError::SubmitterRole)?,
        reason_category: ReasonCategory::from_code(&row.reason_category)
            .ok_or(MappingError::ReasonCategory)?,
        explanation: row
            .explanation
            .map(NoticeExplanation::try_from)
            .transpose()
            .map_err(MappingError::Explanation)?,
        additional_urls: additional_urls(row.additional_urls)?,
        submitter: Submitter {
            name: SubmitterName::try_from(row.submitter_name)
                .map_err(MappingError::SubmitterName)?,
            email: SubmitterEmail::try_from(row.submitter_email)
                .map_err(MappingError::SubmitterEmail)?,
            language: Language::from_code(&row.submitter_language)
                .ok_or(MappingError::SubmitterLanguage)?,
        },
        good_faith_statement_version: GoodFaithStatementVersion::from_code(
            &row.good_faith_statement_version,
        )
        .ok_or(MappingError::GoodFaithStatementVersion)?,
        state: ContentNoticeState::from_code(&row.business_state).ok_or(MappingError::State)?,
    })
    .map_err(MappingError::Invariant)?;
    Ok(Versioned::new(notice, version))
}

fn target(row: &ContentNoticeRow) -> Result<ContentNoticeTarget, MappingError> {
    let listing_source_id =
        ListingSourceId::try_from(row.listing_source_id).map_err(MappingError::ListingSourceId)?;
    match (row.target_type.as_str(), row.product_listing_id) {
        (LISTING_SOURCE_TARGET, None) => {
            Ok(ContentNoticeTarget::ListingSource { listing_source_id })
        }
        (PRODUCT_LISTING_TARGET, Some(product_listing_id)) => {
            Ok(ContentNoticeTarget::ProductListing {
                product_listing_id: ProductListingId::try_from(product_listing_id)
                    .map_err(MappingError::ProductListingId)?,
                listing_source_id,
            })
        }
        _ => Err(MappingError::Target),
    }
}

fn target_snapshot(row: &ContentNoticeRow) -> Result<TargetSnapshot, MappingError> {
    let listing_source = ListingSourceSnapshot {
        name: ListingSourceName::try_from(row.listing_source_name.as_str())
            .map_err(MappingError::ListingSourceName)?,
        url: row
            .listing_source_url
            .as_deref()
            .map(Url::parse)
            .transpose()
            .map_err(MappingError::ListingSourceUrl)?,
        partner: row.listing_source_partner,
    };
    let product_listing = match (&row.product_listing_url, &row.product_listing_title) {
        (Some(url), title) => Some(ProductListingSnapshot {
            title: title
                .as_deref()
                .map(ProductListingTitleSnapshot::try_from)
                .transpose()
                .map_err(MappingError::ProductListingTitle)?,
            url: Url::parse(url).map_err(MappingError::ProductListingUrl)?,
        }),
        (None, None) => None,
        (None, Some(_)) => return Err(MappingError::Target),
    };
    Ok(TargetSnapshot {
        listing_source,
        product_listing,
    })
}

fn additional_urls(values: Vec<String>) -> Result<AdditionalUrls, MappingError> {
    values
        .into_iter()
        .map(AdditionalUrl::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(MappingError::AdditionalUrl)?
        .try_into()
        .map_err(MappingError::AdditionalUrls)
}
