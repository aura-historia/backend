use application::error::box_error;
use auction_core::{
    Auction, AuctionDescription, AuctionFormat, AuctionId, AuctionKey, AuctionName,
    AuctionReportedStatus, AuctionSchedule, RehydratedAuctionState, ReportedCatalogueLotCount,
    SourceAuctionId,
};
use auction_service::ports::{AuctionStorageVersion, StoredAuction};
use domain_primitives::object_id::ObjectIdError;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use std::str::FromStr;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct AuctionRow {
    pub auction_id: uuid::Uuid,
    pub listing_source_id: uuid::Uuid,
    pub source_auction_id: String,
    pub name_text: Option<String>,
    pub name_language: Option<String>,
    pub description_text: Option<String>,
    pub description_language: Option<String>,
    pub catalogue_url: Option<String>,
    pub format: Option<String>,
    pub bidding_opens_at: Option<OffsetDateTime>,
    pub live_starts_at: Option<OffsetDateTime>,
    pub lots_begin_closing_at: Option<OffsetDateTime>,
    pub scheduled_end_at: Option<OffsetDateTime>,
    pub reported_status: Option<String>,
    pub reported_lot_count: Option<i64>,
    pub version: i64,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AuctionRowMappingError {
    #[error("invalid auction ID persisted")]
    AuctionId(#[source] ObjectIdError),
    #[error("invalid listing source ID persisted")]
    ListingSourceId(#[source] ObjectIdError),
    #[error("invalid source auction ID persisted")]
    SourceAuctionId,
    #[error("invalid localized auction field persisted")]
    LocalizedField,
    #[error("invalid auction URL persisted")]
    Url,
    #[error("invalid auction format persisted")]
    Format,
    #[error("invalid auction reported status persisted")]
    ReportedStatus,
    #[error("invalid auction reported lot count persisted")]
    ReportedLotCount,
    #[error("invalid auction version persisted")]
    Version,
    #[error("invalid auction schedule persisted")]
    Schedule,
}

pub(crate) fn storage_version_to_i64(
    version: AuctionStorageVersion,
) -> Result<i64, AuctionRowMappingError> {
    i64::try_from(version.into_inner()).map_err(|_| AuctionRowMappingError::Version)
}

pub(crate) fn map_stored_auction(row: AuctionRow) -> Result<StoredAuction, AuctionRowMappingError> {
    let version = AuctionStorageVersion::try_from(row.version)
        .map_err(|_| AuctionRowMappingError::Version)?;
    let auction_id =
        AuctionId::try_from(row.auction_id).map_err(AuctionRowMappingError::AuctionId)?;
    let listing_source_id = ListingSourceId::try_from(row.listing_source_id)
        .map_err(AuctionRowMappingError::ListingSourceId)?;
    let source_auction_id = SourceAuctionId::try_from(row.source_auction_id.as_str())
        .map_err(|_| AuctionRowMappingError::SourceAuctionId)?;
    if source_auction_id.as_ref() != row.source_auction_id {
        return Err(AuctionRowMappingError::SourceAuctionId);
    }
    let name = map_localized_name(row.name_text, row.name_language)?;
    let description = map_localized_description(row.description_text, row.description_language)?;
    let catalogue_url = row
        .catalogue_url
        .map(|text| {
            let value = Url::parse(&text).map_err(|_| AuctionRowMappingError::Url)?;
            (value.as_str() == text)
                .then_some(value)
                .ok_or(AuctionRowMappingError::Url)
        })
        .transpose()?;
    let format = row
        .format
        .map(|value| AuctionFormat::from_str(&value).map_err(|_| AuctionRowMappingError::Format))
        .transpose()?;
    let reported_status = row
        .reported_status
        .map(|value| {
            AuctionReportedStatus::from_str(&value)
                .map_err(|_| AuctionRowMappingError::ReportedStatus)
        })
        .transpose()?;
    let reported_lot_count = row
        .reported_lot_count
        .map(|value| {
            u32::try_from(value)
                .map(ReportedCatalogueLotCount::new)
                .map_err(|_| AuctionRowMappingError::ReportedLotCount)
        })
        .transpose()?;
    let schedule = AuctionSchedule::new(
        row.bidding_opens_at,
        row.live_starts_at,
        row.lots_begin_closing_at,
        row.scheduled_end_at,
    )
    .map_err(|_| AuctionRowMappingError::Schedule)?;
    let auction = Auction::rehydrate(RehydratedAuctionState {
        id: auction_id,
        key: AuctionKey::new(listing_source_id, source_auction_id),
        name,
        description,
        catalogue_url,
        format,
        schedule,
        reported_status,
        reported_lot_count,
    })
    .map_err(|_| AuctionRowMappingError::Schedule)?;
    Ok(StoredAuction {
        auction,
        version,
        created: row.created,
        updated: row.updated,
    })
}

fn map_localized_name(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, AuctionName>>, AuctionRowMappingError> {
    match (text, language) {
        (None, None) => Ok(None),
        (Some(text), Some(language)) => {
            let payload = AuctionName::try_from(text.as_str())
                .map_err(|_| AuctionRowMappingError::LocalizedField)?;
            (payload.as_ref() == text)
                .then_some(())
                .ok_or(AuctionRowMappingError::LocalizedField)?;
            let localization =
                Language::from_code(&language).ok_or(AuctionRowMappingError::LocalizedField)?;
            Ok(Some(Localized::new(localization, payload)))
        }
        _ => Err(AuctionRowMappingError::LocalizedField),
    }
}

fn map_localized_description(
    text: Option<String>,
    language: Option<String>,
) -> Result<Option<Localized<Language, AuctionDescription>>, AuctionRowMappingError> {
    match (text, language) {
        (None, None) => Ok(None),
        (Some(text), Some(language)) => {
            let payload = AuctionDescription::try_from(text.as_str())
                .map_err(|_| AuctionRowMappingError::LocalizedField)?;
            (payload.as_ref() == text)
                .then_some(())
                .ok_or(AuctionRowMappingError::LocalizedField)?;
            let localization =
                Language::from_code(&language).ok_or(AuctionRowMappingError::LocalizedField)?;
            Ok(Some(Localized::new(localization, payload)))
        }
        _ => Err(AuctionRowMappingError::LocalizedField),
    }
}

pub(crate) fn map_error(error: AuctionRowMappingError) -> application::error::BoxError {
    box_error(error)
}
