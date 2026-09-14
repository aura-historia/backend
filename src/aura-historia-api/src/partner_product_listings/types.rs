use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use crate::wire::parse_path_object_id;
use application::patch_field::PatchField;
use auction_core::{
    AuctionDescription, AuctionFormat, AuctionName, AuctionReportedStatus, AuctionTime,
    AuctionTimeZone, ReportedCatalogueLotCount, SourceAuctionId,
};
use auction_service::{EmbeddedAuctionMetadata, validate_embedded_auction_metadata_schedule};
use listing_source_core::ListingSourceId;
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{CataloguePosition, LotNumber, ProductListingPricing};
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_core::title::Title;
use product_listing_service::product_listing_auction_patch::ProductListingAuctionPatch;
use product_listing_service::use_cases::{
    CreateProductListingCommand, UpdateProductListingCommand, UpsertProductListingCommand,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use time::{Date, OffsetDateTime, format_description::well_known::Iso8601};
use url::Url;

pub(super) const MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateProductListingData {
    pub(super) source_listing_id: String,
    pub(super) title: LocalizedTextData,
    pub(super) description: LocalizedTextData,
    #[serde(default)]
    pub(super) price: Option<ProductListingPriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: Option<PriceData>,
    #[serde(default, with = "crate::wire::listing_availability::option")]
    pub(super) availability: Option<ListingAvailability>,
    pub(super) url: Url,
    pub(super) images: Vec<Url>,
    #[serde(default)]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateProductListingData {
    pub(super) source_listing_id: String,
    #[serde(default)]
    pub(super) price: PatchValue<ProductListingPriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::listing_availability::patch::deserialize")]
    pub(super) availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    pub(super) url: PatchValue<Url>,
    #[serde(default)]
    pub(super) images: PatchValue<Vec<Url>>,
    #[serde(default)]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpsertProductListingData {
    pub(super) source_listing_id: String,
    #[serde(default)]
    pub(super) title: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) description: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) price: PatchValue<ProductListingPriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    #[serde(deserialize_with = "crate::wire::listing_availability::patch::deserialize")]
    pub(super) availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    pub(super) url: Option<Url>,
    #[serde(default)]
    pub(super) images: PatchValue<Vec<Url>>,
    #[serde(default)]
    auction: PatchValue<ProductListingAuctionData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductListingAuctionData {
    #[serde(default)]
    source_auction_id: PatchValue<String>,
    #[serde(default)]
    metadata: EmbeddedAuctionMetadataData,
    #[serde(default)]
    lot_number: PatchValue<String>,
    #[serde(default)]
    catalogue_position: PatchValue<u64>,
    #[serde(default)]
    timing: Option<LotAuctionTimingData>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedAuctionMetadataData {
    #[serde(default)]
    name: Option<LocalizedTextData>,
    #[serde(default)]
    description: Option<LocalizedTextData>,
    #[serde(default)]
    catalogue_url: Option<Url>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    reported_status: Option<String>,
    #[serde(default)]
    reported_lot_count: Option<u32>,
    #[serde(default)]
    schedule: EmbeddedAuctionScheduleData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EmbeddedAuctionScheduleData {
    #[serde(default)]
    bidding_opens: Option<AuctionTimeData>,
    #[serde(default)]
    live_starts: Option<AuctionTimeData>,
    #[serde(default)]
    lots_begin_closing: Option<AuctionTimeData>,
    #[serde(default)]
    scheduled_end: Option<AuctionTimeData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LotAuctionTimingData {
    #[serde(default)]
    bidding_opens: PatchValue<AuctionTimeData>,
    #[serde(default)]
    scheduled_closes: PatchValue<AuctionTimeData>,
    #[serde(default, deserialize_with = "patch_rfc3339")]
    reported_closed_at: PatchValue<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "precision",
    rename_all = "SCREAMING_SNAKE_CASE",
    deny_unknown_fields
)]
enum AuctionTimeData {
    Instant {
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
    Date {
        on: String,
        #[serde(default, rename = "sourceTimezone")]
        source_timezone: Option<String>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WithdrawProductListingData {
    pub(super) source_listing_id: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PartnerProductFailureData {
    listing_source_id: ListingSourceId,
    source_listing_id: String,
    error: ApiErrorCode,
}

pub(super) fn parse_listing_source_id(value: &str) -> Result<ListingSourceId, ApiError> {
    parse_path_object_id(value, "listingSourceId", "ListingSource")
}

pub(super) fn parse_partner_product_batch<T: DeserializeOwned>(
    body: &str,
) -> Result<Vec<T>, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }

    let products: Vec<T> = serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    if products.len() > MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!(
            "Body cannot contain more than {MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE} products."
        )));
    }

    Ok(products)
}

impl CreateProductListingData {
    pub(super) fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<CreateProductListingCommand, ApiError> {
        let ProductListingAuctionCommandPatch { auction, metadata } = auction_patch(self.auction)?;
        let auction = match auction {
            PatchField::Set(auction) => Some(auction),
            PatchField::Clear | PatchField::Unchanged => None,
        };
        Ok(CreateProductListingCommand {
            listing_source_id,
            source_listing_id: source_listing_id(self.source_listing_id)?,
            title: Some(title(self.title)),
            description: Some(description(self.description)),
            pricing: ProductListingPricing {
                price: self.price.map(product_listing_price),
                price_estimate_min: self.price_estimate_min.map(price),
                price_estimate_max: self.price_estimate_max.map(price),
            },
            availability: self.availability,
            url: self.url,
            images: product_images(self.images),
            auction,
            auction_metadata: metadata,
        })
    }
}

impl UpdateProductListingData {
    pub(super) fn into_key_and_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<(ProductListingKey, UpdateProductListingCommand), ApiError> {
        let product_key = ProductListingKey::new(
            listing_source_id,
            source_listing_id(self.source_listing_id)?,
        );
        let auction = auction_patch(self.auction)?;
        let command = UpdateProductListingCommand {
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: non_nullable_patch(self.url, "url")?,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction: auction.auction,
            auction_metadata: auction.metadata,
        };
        Ok((product_key, command))
    }
}

impl UpsertProductListingData {
    pub(super) fn into_command(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<UpsertProductListingCommand, ApiError> {
        let auction = auction_patch(self.auction)?;
        Ok(UpsertProductListingCommand {
            listing_source_id,
            source_listing_id: source_listing_id(self.source_listing_id)?,
            title: self.title.map(title),
            description: self.description.map(description),
            price: clearable(self.price.map(product_listing_price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: self.url,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction: auction.auction,
            auction_metadata: auction.metadata,
        })
    }
}

struct ProductListingAuctionContext {
    context: ProductListingAuctionPatch,
    metadata: EmbeddedAuctionMetadata,
}

struct ProductListingAuctionCommandPatch {
    auction: PatchField<ProductListingAuctionPatch>,
    metadata: EmbeddedAuctionMetadata,
}

impl ProductListingAuctionData {
    fn into_core(self) -> Result<ProductListingAuctionContext, ApiError> {
        Ok(ProductListingAuctionContext {
            context: ProductListingAuctionPatch {
                source_auction_id: source_auction_id_patch(self.source_auction_id)?,
                lot_number: lot_number_patch(self.lot_number)?,
                catalogue_position: catalogue_position_patch(self.catalogue_position)?,
                ..self
                    .timing
                    .map(LotAuctionTimingData::into_core)
                    .transpose()?
                    .unwrap_or_default()
            },
            metadata: self.metadata.into_core()?,
        })
    }
}

impl EmbeddedAuctionMetadataData {
    fn into_core(self) -> Result<EmbeddedAuctionMetadata, ApiError> {
        Ok(EmbeddedAuctionMetadata {
            name: self.name.map(auction_name).transpose()?,
            description: self.description.map(auction_description).transpose()?,
            catalogue_url: self.catalogue_url,
            format: self.format.map(auction_format).transpose()?,
            reported_status: self.reported_status.map(auction_status).transpose()?,
            reported_lot_count: self.reported_lot_count.map(ReportedCatalogueLotCount::new),
            ..self.schedule.into_core()?
        })
    }
}

impl EmbeddedAuctionScheduleData {
    fn into_core(self) -> Result<EmbeddedAuctionMetadata, ApiError> {
        let metadata = EmbeddedAuctionMetadata {
            bidding_opens: self
                .bidding_opens
                .map(AuctionTimeData::into_core)
                .transpose()?,
            live_starts: self
                .live_starts
                .map(AuctionTimeData::into_core)
                .transpose()?,
            lots_begin_closing: self
                .lots_begin_closing
                .map(AuctionTimeData::into_core)
                .transpose()?,
            scheduled_end: self
                .scheduled_end
                .map(AuctionTimeData::into_core)
                .transpose()?,
            ..EmbeddedAuctionMetadata::default()
        };
        validate_embedded_auction_metadata_schedule(&metadata).map_err(|_| {
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_detail("auction schedule has invalid comparable bounds.")
        })?;
        Ok(metadata)
    }
}

impl LotAuctionTimingData {
    fn into_core(self) -> Result<ProductListingAuctionPatch, ApiError> {
        Ok(ProductListingAuctionPatch {
            bidding_opens: auction_time_patch(self.bidding_opens)?,
            scheduled_closes: auction_time_patch(self.scheduled_closes)?,
            reported_closed_at: patch_value(self.reported_closed_at),
            ..Default::default()
        })
    }
}

impl AuctionTimeData {
    fn into_core(self) -> Result<AuctionTime, ApiError> {
        match self {
            Self::Instant {
                at,
                source_timezone,
            } => Ok(AuctionTime::instant(
                at,
                source_timezone.map(timezone).transpose()?,
            )),
            Self::Date {
                on,
                source_timezone,
            } => {
                let on = Date::parse(&on, &Iso8601::DATE).map_err(|_| {
                    ApiError::bad_request(BAD_BODY_VALUE)
                        .with_detail("auction timing date must use YYYY-MM-DD.")
                })?;
                Ok(AuctionTime::date(
                    on,
                    source_timezone.map(timezone).transpose()?,
                ))
            }
        }
    }
}

impl WithdrawProductListingData {
    pub(super) fn into_product_key(
        self,
        listing_source_id: ListingSourceId,
    ) -> Result<ProductListingKey, ApiError> {
        Ok(ProductListingKey::new(
            listing_source_id,
            source_listing_id(self.source_listing_id)?,
        ))
    }
}

impl PartnerProductFailureData {
    pub(super) fn new(
        listing_source_id: ListingSourceId,
        source_listing_id: String,
        error: ApiErrorCode,
    ) -> Self {
        Self {
            listing_source_id,
            source_listing_id,
            error,
        }
    }
}

fn title(value: LocalizedTextData) -> localization::Localized<localization::Language, Title> {
    value.into_localized()
}

fn description(
    value: LocalizedTextData,
) -> localization::Localized<localization::Language, Description> {
    value.into_localized()
}

fn price(value: PriceData) -> Price {
    value.into()
}

fn product_listing_price(
    value: ProductListingPriceData,
) -> product_listing_core::product_listing_price::ProductListingPrice {
    value.into()
}

fn product_images(values: Vec<Url>) -> indexmap::IndexSet<ProductListingImage> {
    values.into_iter().map(ProductListingImage::new).collect()
}

fn auction_patch(
    value: PatchValue<ProductListingAuctionData>,
) -> Result<ProductListingAuctionCommandPatch, ApiError> {
    match value {
        PatchValue::Omitted => Ok(ProductListingAuctionCommandPatch {
            auction: PatchField::Unchanged,
            metadata: EmbeddedAuctionMetadata::default(),
        }),
        PatchValue::Null => Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(
            "auction cannot be null in an ordinary update; use the dedicated correction operation.",
        )),
        PatchValue::Value(value) => {
            let value = value.into_core()?;
            Ok(ProductListingAuctionCommandPatch {
                auction: PatchField::Set(value.context),
                metadata: value.metadata,
            })
        }
    }
}

fn source_auction_id_patch(
    value: PatchValue<String>,
) -> Result<PatchField<SourceAuctionId>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(
            "auction.sourceAuctionId cannot be null; omit it to preserve membership.",
        )),
        PatchValue::Value(value) => source_auction_id(value).map(PatchField::Set),
    }
}

fn lot_number_patch(value: PatchValue<String>) -> Result<PatchField<LotNumber>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => LotNumber::try_from(value)
            .map(PatchField::Set)
            .map_err(|_| {
                ApiError::bad_request(BAD_BODY_VALUE).with_detail(
                    "auction.lotNumber must be nonblank, NUL-free, and at most 128 UTF-8 bytes.",
                )
            }),
    }
}

fn catalogue_position_patch(
    value: PatchValue<u64>,
) -> Result<PatchField<CataloguePosition>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => CataloguePosition::try_from(value)
            .map(PatchField::Set)
            .map_err(|_| {
                ApiError::bad_request(BAD_BODY_VALUE)
                    .with_detail("auction.cataloguePosition must be a positive 32-bit integer.")
            }),
    }
}

fn auction_time_patch(
    value: PatchValue<AuctionTimeData>,
) -> Result<PatchField<AuctionTime>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => value.into_core().map(PatchField::Set),
    }
}

fn patch_value<T>(value: PatchValue<T>) -> PatchField<T> {
    match value {
        PatchValue::Omitted => PatchField::Unchanged,
        PatchValue::Null => PatchField::Clear,
        PatchValue::Value(value) => PatchField::Set(value),
    }
}

fn patch_rfc3339<'de, D>(deserializer: D) -> Result<PatchValue<OffsetDateTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = PatchValue::<String>::deserialize(deserializer)?;
    match value {
        PatchValue::Omitted => Ok(PatchValue::Omitted),
        PatchValue::Null => Ok(PatchValue::Null),
        PatchValue::Value(value) => {
            OffsetDateTime::parse(&value, &time::format_description::well_known::Rfc3339)
                .map(PatchValue::Value)
                .map_err(serde::de::Error::custom)
        }
    }
}

fn auction_name(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionName>, ApiError> {
    AuctionName::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE).with_detail("auction.metadata.name.text must be nonblank, NUL-free, and at most 512 UTF-8 bytes."))
}

fn auction_description(
    value: LocalizedTextData,
) -> Result<localization::Localized<localization::Language, AuctionDescription>, ApiError> {
    AuctionDescription::try_from(value.text)
        .map(|payload| localization::Localized::new(value.language, payload))
        .map_err(|_| ApiError::bad_request(BAD_BODY_VALUE).with_detail("auction.metadata.description.text must be valid nonblank sanitized text of at most 65536 UTF-8 bytes."))
}

fn auction_format(value: String) -> Result<AuctionFormat, ApiError> {
    value.parse().map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction.metadata.format must be LIVE or TIMED.")
    })
}

fn auction_status(value: String) -> Result<AuctionReportedStatus, ApiError> {
    value.parse().map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction.metadata.reportedStatus must be SCHEDULED, IN_PROGRESS, ENDED, POSTPONED, or CANCELLED.")
    })
}

fn source_auction_id(value: String) -> Result<SourceAuctionId, ApiError> {
    SourceAuctionId::try_from(value).map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE).with_detail(
            "auction.sourceAuctionId must be nonblank, NUL-free, and at most 512 UTF-8 bytes.",
        )
    })
}

fn timezone(value: String) -> Result<AuctionTimeZone, ApiError> {
    AuctionTimeZone::try_from(value).map_err(|_| {
        ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction timing sourceTimezone must be a valid IANA timezone identifier.")
    })
}

fn source_listing_id(value: String) -> Result<SourceListingId, ApiError> {
    SourceListingId::try_from(value)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        CreateProductListingData, UpdateProductListingData, UpsertProductListingData,
        WithdrawProductListingData, parse_listing_source_id, source_listing_id,
    };
    use crate::error::{BAD_BODY_VALUE, INVALID_OBJECT_ID};
    use application::patch_field::PatchField;
    use listing_source_core::ListingSourceId;
    use product_listing_core::product_listing_id::ProductListingId;

    #[test]
    fn should_reject_null_auction_in_partner_writes() {
        let listing_source_id = ListingSourceId::new();
        let create: CreateProductListingData = serde_json::from_str(
            r#"{
                "sourceListingId":"SKU-1",
                "title":{"text":"Listing","language":"en"},
                "description":{"text":"Description","language":"en"},
                "url":"https://example.com/listing",
                "images":[],
                "auction":null
            }"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let upsert: UpsertProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
                .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        assert_eq!(
            BAD_BODY_VALUE,
            create
                .into_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .code()
        );
        assert_eq!(
            BAD_BODY_VALUE,
            update
                .into_key_and_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .code()
        );
        assert_eq!(
            BAD_BODY_VALUE,
            upsert
                .into_command(listing_source_id)
                .err()
                .unwrap_or_else(|| panic!("null auction must fail"))
                .code()
        );
    }

    #[test]
    fn should_reject_invalid_shared_schedule_for_each_partner_write_codec() {
        let listing_source_id = ListingSourceId::new();
        let create: CreateProductListingData = serde_json::from_str(
            r#"{
                "sourceListingId":"SKU-1",
                "title":{"text":"Listing","language":"en"},
                "description":{"text":"Description","language":"en"},
                "url":"https://example.com/listing",
                "images":[],
                "auction":{"sourceAuctionId":"sale-42","metadata":{"schedule":{
                    "biddingOpens":{"precision":"INSTANT","at":"2026-10-19T10:00:00Z"},
                    "scheduledEnd":{"precision":"INSTANT","at":"2026-10-18T10:00:00Z"}
                }}}
            }"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON shape: {error}"));
        let update: UpdateProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":"sale-42","metadata":{"schedule":{"biddingOpens":{"precision":"INSTANT","at":"2026-10-19T10:00:00Z"},"scheduledEnd":{"precision":"INSTANT","at":"2026-10-18T10:00:00Z"}}}}}"#,
        )
        .unwrap_or_else(|error| panic!("valid update JSON shape: {error}"));
        let upsert: UpsertProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":"sale-42","metadata":{"schedule":{"biddingOpens":{"precision":"INSTANT","at":"2026-10-19T10:00:00Z"},"scheduledEnd":{"precision":"INSTANT","at":"2026-10-18T10:00:00Z"}}}}}"#,
        )
        .unwrap_or_else(|error| panic!("valid upsert JSON shape: {error}"));

        for result in [
            create.into_command(listing_source_id).map(|_| ()),
            update.into_key_and_command(listing_source_id).map(|_| ()),
            upsert.into_command(listing_source_id).map(|_| ()),
        ] {
            let error = result
                .err()
                .unwrap_or_else(|| panic!("invalid comparable schedule must fail"));
            assert_eq!(BAD_BODY_VALUE, error.code());
        }
    }

    #[test]
    fn should_map_reliable_auction_key_and_embedded_metadata_for_partner_create() {
        let data: CreateProductListingData = serde_json::from_str(
            r#"{
                "sourceListingId":"SKU-1",
                "title":{"text":"Listing","language":"en"},
                "description":{"text":"Description","language":"en"},
                "url":"https://example.com/listing",
                "images":[],
                "auction":{
                    "sourceAuctionId":" sale-42 ",
                    "metadata":{
                        "name":{"text":"Spring sale","language":"en"},
                        "format":"TIMED",
                        "reportedLotCount":100,
                        "schedule":{"scheduledEnd":{"precision":"INSTANT","at":"2026-05-01T12:00:00Z"}}
                    },
                    "lotNumber":"42"
                }
            }"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));

        let command = data
            .into_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid create command: {error}"));

        assert!(matches!(
            command.auction.as_ref(),
            Some(auction)
                if matches!(&auction.source_auction_id, PatchField::Set(value) if value.as_ref() == "sale-42")
        ));
        assert_eq!(
            command
                .auction_metadata
                .name
                .as_ref()
                .map(|value| value.payload.as_ref()),
            Some("Spring sale")
        );
        assert!(command.auction_metadata.scheduled_end.is_some());
        assert!(command.auction.is_some());
    }

    #[test]
    fn should_map_nested_partner_auction_leaf_patch_actions() {
        let update: UpdateProductListingData = serde_json::from_str(
            r#"{
                "sourceListingId":"SKU-1",
                "auction":{
                    "sourceAuctionId":"sale-42",
                    "lotNumber":null,
                    "timing":{
                        "biddingOpens":null,
                        "reportedClosedAt":"2026-05-01T12:00:00Z"
                    }
                }
            }"#,
        )
        .unwrap_or_else(|error| panic!("valid update JSON: {error}"));

        let (_, command) = update
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let PatchField::Set(auction) = command.auction else {
            panic!("auction patch should be asserted");
        };
        assert!(matches!(
            auction.source_auction_id,
            PatchField::Set(value) if value.as_ref() == "sale-42"
        ));
        assert_eq!(PatchField::Clear, auction.lot_number);
        assert_eq!(PatchField::Unchanged, auction.catalogue_position);
        assert_eq!(PatchField::Clear, auction.bidding_opens);
        assert_eq!(PatchField::Unchanged, auction.scheduled_closes);
        assert!(matches!(auction.reported_closed_at, PatchField::Set(_)));
    }

    #[test]
    fn should_reject_null_source_auction_id_in_partner_auction_patch() {
        let update: UpdateProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":null}}"#,
        )
        .unwrap_or_else(|error| panic!("valid update JSON: {error}"));

        let error = update
            .into_key_and_command(ListingSourceId::new())
            .err()
            .unwrap_or_else(|| panic!("null source auction ID must fail"));
        assert_eq!(BAD_BODY_VALUE, error.code());
    }

    #[test]
    fn should_reject_invalid_reliable_auction_key_and_unknown_metadata_fields() {
        let invalid_key: UpdateProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":" \t"}}"#,
        )
        .unwrap_or_else(|error| panic!("valid JSON shape: {error}"));
        let error = invalid_key
            .into_key_and_command(ListingSourceId::new())
            .err()
            .unwrap_or_else(|| panic!("blank source auction ID must fail"));
        assert_eq!(error.code(), BAD_BODY_VALUE);

        let unknown_metadata = serde_json::from_str::<UpdateProductListingData>(
            r#"{"sourceListingId":"SKU-1","auction":{"metadata":{"unknown":true}}}"#,
        );
        assert!(unknown_metadata.is_err());
    }

    #[test]
    fn should_leave_auction_unchanged_when_partner_update_or_upsert_omits_it() {
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let upsert: UpsertProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
                .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        let (_, update) = update
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let upsert = upsert
            .into_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid upsert command: {error}"));

        assert_eq!(update.auction, PatchField::Unchanged);
        assert_eq!(upsert.auction, PatchField::Unchanged);
    }

    #[test]
    fn should_parse_source_listing_id_without_slugifying_it() {
        let data: WithdrawProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"\u2003SKU  #42/Blue\u2002"}"#)
                .unwrap_or_else(|error| panic!("valid request data: {error}"));

        let key = data
            .into_product_key(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

        assert_eq!(key.source_listing_id.as_ref(), "SKU  #42/Blue");
    }

    #[test]
    fn should_parse_only_canonical_listing_source_object_ids() {
        let listing_source_id = ListingSourceId::new();
        assert!(matches!(
            parse_listing_source_id(&listing_source_id.to_string()),
            Ok(parsed) if parsed == listing_source_id
        ));

        for invalid_id in [
            ProductListingId::new().to_string(),
            listing_source_id.as_uuid().to_string(),
            "ls_not-a-typeid".to_owned(),
        ] {
            let error = parse_listing_source_id(&invalid_id)
                .err()
                .unwrap_or_else(|| panic!("noncanonical ListingSource ID was accepted"));
            assert_eq!(INVALID_OBJECT_ID, error.code());
        }
    }

    #[test]
    fn should_reject_blank_source_listing_id_at_api_mapping() {
        let error = source_listing_id("\u{2003}\t".to_owned())
            .err()
            .unwrap_or_else(|| panic!("blank source listing ID must fail"));

        assert_eq!(error.code(), BAD_BODY_VALUE);
    }
}
