use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use crate::wire::parse_path_object_id;
use application::patch_field::PatchField;
use auction_core::AuctionId;
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
use time::OffsetDateTime;
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
    auction_id: PatchValue<String>,
    #[serde(default)]
    lot_number: PatchValue<String>,
    #[serde(default)]
    catalogue_position: PatchValue<u64>,
    #[serde(default)]
    timing: Option<LotAuctionTimesData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LotAuctionTimesData {
    #[serde(default, deserialize_with = "patch_rfc3339")]
    bidding_opens: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "patch_rfc3339")]
    scheduled_closes: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "patch_rfc3339")]
    reported_closed_at: PatchValue<OffsetDateTime>,
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
        let auction = match auction_patch(self.auction)? {
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
            auction,
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
            auction,
        })
    }
}

impl ProductListingAuctionData {
    fn into_core(self) -> Result<ProductListingAuctionPatch, ApiError> {
        Ok(ProductListingAuctionPatch {
            auction_id: auction_id_patch(self.auction_id)?,
            lot_number: lot_number_patch(self.lot_number)?,
            catalogue_position: catalogue_position_patch(self.catalogue_position)?,
            ..self
                .timing
                .map(LotAuctionTimesData::into_core)
                .transpose()?
                .unwrap_or_default()
        })
    }
}

impl LotAuctionTimesData {
    fn into_core(self) -> Result<ProductListingAuctionPatch, ApiError> {
        Ok(ProductListingAuctionPatch {
            bidding_opens: patch_value(self.bidding_opens),
            scheduled_closes: patch_value(self.scheduled_closes),
            reported_closed_at: patch_value(self.reported_closed_at),
            ..Default::default()
        })
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
) -> Result<PatchField<ProductListingAuctionPatch>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Err(ApiError::bad_request(BAD_BODY_VALUE)
            .with_detail("auction cannot be null; omit it or clear auction.auctionId.")),
        PatchValue::Value(value) => value.into_core().map(PatchField::Set),
    }
}

fn auction_id_patch(value: PatchValue<String>) -> Result<PatchField<AuctionId>, ApiError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => {
            parse_path_object_id(&value, "auctionId", "Auction").map(PatchField::Set)
        }
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
    fn should_map_existing_auction_id_and_listing_owned_leaf_patches() {
        let auction_id = auction_core::AuctionId::new();
        let data: UpdateProductListingData = serde_json::from_str(&format!(
            r#"{{"sourceListingId":"SKU-1","auction":{{"auctionId":"{auction_id}","lotNumber":null,"timing":{{"biddingOpens":null,"reportedClosedAt":"2026-05-01T12:00:00Z"}}}}}}"#
        ))
        .unwrap_or_else(|error| panic!("valid update JSON: {error}"));

        let (_, command) = data
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let PatchField::Set(auction) = command.auction else {
            panic!("auction patch should be asserted");
        };
        assert_eq!(PatchField::Set(auction_id), auction.auction_id);
        assert_eq!(PatchField::Clear, auction.lot_number);
        assert_eq!(PatchField::Clear, auction.bidding_opens);
        assert!(matches!(auction.reported_closed_at, PatchField::Set(_)));
    }

    #[test]
    fn should_reject_date_only_and_timezone_less_lot_timestamps() {
        for timestamp in ["2026-05-01", "2026-05-01T12:00:00"] {
            assert!(serde_json::from_str::<UpdateProductListingData>(&format!(
                r#"{{"sourceListingId":"SKU-1","auction":{{"timing":{{"biddingOpens":"{timestamp}"}}}}}}"#
            ))
            .is_err());
        }
    }

    #[test]
    fn should_clear_membership_with_null_auction_id_and_reject_retired_fields() {
        let update: UpdateProductListingData =
            serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":{"auctionId":null}}"#)
                .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
        let (_, command) = update
            .into_key_and_command(ListingSourceId::new())
            .unwrap_or_else(|error| panic!("valid update command: {error}"));
        let PatchField::Set(auction) = command.auction else {
            panic!("auction patch should be asserted");
        };
        assert_eq!(PatchField::Clear, auction.auction_id);

        assert!(
            serde_json::from_str::<UpdateProductListingData>(
                r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":"sale-42"}}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<UpdateProductListingData>(
                r#"{"sourceListingId":"SKU-1","auction":{"metadata":{"name":"sale"}}}"#
            )
            .is_err()
        );
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
