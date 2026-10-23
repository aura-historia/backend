use crate::patch_value::{PatchValue, clearable};
use crate::product_listing_management::MutationInputError;
use application::patch_field::PatchField;
use auction_core::AuctionId;
use product_listing_core::{
    product_listing::{CataloguePosition, LotNumber},
    product_listing_image::ProductListingImage,
};
use product_listing_service::product_listing_auction_patch::ProductListingAuctionPatch;
use schemars::JsonSchema;
use serde::Deserialize;
use time::OffsetDateTime;
use url::Url;
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductListingAuctionData {
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::optional_auction_id")]
    auction_id: PatchValue<String>,
    #[serde(default)]
    lot_number: PatchValue<String>,
    #[serde(default)]
    #[schemars(schema_with = "catalogue_position_patch_schema")]
    catalogue_position: PatchValue<u64>,
    #[serde(default)]
    timing: Option<LotAuctionTimesData>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LotAuctionTimesData {
    #[serde(default, deserialize_with = "patch_rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    bidding_opens: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "patch_rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    scheduled_closes: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "patch_rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    reported_closed_at: PatchValue<OffsetDateTime>,
}

impl ProductListingAuctionData {
    fn into_core(self) -> Result<ProductListingAuctionPatch, MutationInputError> {
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
    fn into_core(self) -> Result<ProductListingAuctionPatch, MutationInputError> {
        Ok(ProductListingAuctionPatch {
            bidding_opens: clearable(self.bidding_opens),
            scheduled_closes: clearable(self.scheduled_closes),
            reported_closed_at: clearable(self.reported_closed_at),
            ..Default::default()
        })
    }
}

pub fn product_images(values: Vec<Url>) -> indexmap::IndexSet<ProductListingImage> {
    values.into_iter().map(ProductListingImage::new).collect()
}

pub fn auction_patch(
    value: PatchValue<ProductListingAuctionData>,
) -> Result<PatchField<ProductListingAuctionPatch>, MutationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Err(MutationInputError::invalid(
            "auction",
            "auction cannot be null; omit it or clear auction.auctionId.",
        )),
        PatchValue::Value(value) => value.into_core().map(PatchField::Set),
    }
}

fn auction_id_patch(
    value: PatchValue<String>,
) -> Result<PatchField<AuctionId>, MutationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => {
            value
                .parse()
                .map(PatchField::Set)
                .map_err(|source| MutationInputError {
                    invalid_object_id: true,
                    ..MutationInputError::caused(
                        "auction.auctionId",
                        "must be a valid Auction ID",
                        source,
                    )
                })
        }
    }
}

fn lot_number_patch(
    value: PatchValue<String>,
) -> Result<PatchField<LotNumber>, MutationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => LotNumber::try_from(value).map(PatchField::Set).map_err(
            |source| {
                MutationInputError::caused(
                    "auction",
                    "auction.lotNumber must be nonblank, NUL-free, and at most 128 UTF-8 bytes.",
                    source,
                )
            },
        ),
    }
}

fn catalogue_position_patch(
    value: PatchValue<u64>,
) -> Result<PatchField<CataloguePosition>, MutationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Null => Ok(PatchField::Clear),
        PatchValue::Value(value) => CataloguePosition::try_from(value)
            .map(PatchField::Set)
            .map_err(|source| {
                MutationInputError::caused(
                    "auction",
                    "auction.cataloguePosition must be a positive 32-bit integer.",
                    source,
                )
            }),
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

fn catalogue_position_patch_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"anyOf":[{"type":"integer","minimum":1,"maximum":u32::MAX},{"type":"null"}]})
}
