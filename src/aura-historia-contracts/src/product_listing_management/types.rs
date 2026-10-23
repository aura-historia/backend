use super::MutationInputError;
use crate::{
    patch_value::{PatchValue, clearable},
    product_listing_fields::{ProductListingAuctionData, auction_patch, product_images},
    values::{LocalizedTextData, PriceData, ProductListingPriceData},
};
use application::patch_field::PatchField;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
};
use product_listing_service::{
    ports::{ProductListingStorageVersion, admin_product_listing_reader::*},
    use_cases::commands::{
        admin_product_listings::*, update_product_listing::UpdateProductListingCommand,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateData {
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    expected_version: u64,
    #[serde(default)]
    price: PatchValue<AdminPriceData>,
    #[serde(default)]
    price_estimate_min: PatchValue<MonetaryPriceData>,
    #[serde(default)]
    price_estimate_max: PatchValue<MonetaryPriceData>,
    #[serde(default, deserialize_with = "crate::wire::listing::availability_patch")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: PatchValue<Url>,
    #[serde(default)]
    #[schemars(schema_with = "crate::product_listing_fields::image_urls_schema")]
    images: PatchValue<Vec<Url>>,
    #[serde(default)]
    #[schemars(with = "ProductListingAuctionData")]
    auction: PatchValue<ProductListingAuctionData>,
}

// A struct variant enforces unknown-field rejection for ON_REQUEST as well.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum AdminPriceData {
    Monetary {
        #[serde(with = "crate::wire::currency")]
        #[schemars(schema_with = "crate::wire::currency::schema")]
        currency: money::Currency,
        #[serde(deserialize_with = "storage_amount")]
        #[schemars(range(max = i64::MAX as u64))]
        amount: u64,
    },
    OnRequest {},
}
impl From<AdminPriceData> for product_listing_core::product_listing_price::ProductListingPrice {
    fn from(value: AdminPriceData) -> Self {
        match value {
            AdminPriceData::Monetary { currency, amount } => {
                ProductListingPriceData::Monetary { currency, amount }.into()
            }
            AdminPriceData::OnRequest {} => ProductListingPriceData::OnRequest.into(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MonetaryPriceData {
    #[serde(with = "crate::wire::currency")]
    #[schemars(schema_with = "crate::wire::currency::schema")]
    currency: money::Currency,
    #[serde(deserialize_with = "storage_amount")]
    #[schemars(range(max = i64::MAX as u64))]
    amount: u64,
}
impl From<MonetaryPriceData> for money::Price {
    fn from(value: MonetaryPriceData) -> Self {
        Self::new(value.amount.into(), value.currency)
    }
}
fn storage_amount<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let value = u64::deserialize(deserializer)?;
    i64::try_from(value)
        .map_err(|_| serde::de::Error::custom("amount exceeds the supported storage range"))?;
    Ok(value)
}

impl UpdateData {
    pub fn into_command(
        self,
        id: ProductListingId,
    ) -> Result<UpdateAdminProductListingCommand, MutationInputError> {
        Ok(UpdateAdminProductListingCommand {
            product_listing_id: id,
            expected_version: version(self.expected_version)?,
            patch: UpdateProductListingCommand {
                price: clearable(self.price.map(Into::into)),
                price_estimate_min: clearable(self.price_estimate_min.map(Into::into)),
                price_estimate_max: clearable(self.price_estimate_max.map(Into::into)),
                availability: clearable(self.availability),
                url: non_nullable_patch(self.url, "url")?,
                images: non_nullable_patch(self.images.map(product_images), "images")?,
                auction: auction_patch(self.auction)?,
            },
        })
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VersionData {
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    pub expected_version: u64,
}
pub fn version(value: u64) -> Result<ProductListingStorageVersion, MutationInputError> {
    i64::try_from(value)
        .ok()
        .and_then(|value| ProductListingStorageVersion::try_from(value).ok())
        .ok_or_else(|| {
            MutationInputError::invalid(
                "expectedVersion",
                "expectedVersion must be between 1 and 9223372036854775807.",
            )
        })
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MutationData {
    product_listing_id: String,
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    expected_version: u64,
    outcome: &'static str,
}
impl From<AdminProductListingMutationResult> for MutationData {
    fn from(result: AdminProductListingMutationResult) -> Self {
        Self {
            product_listing_id: result.product_listing_id.to_string(),
            expected_version: result.expected_version.into(),
            outcome: if result.outcome.changed() {
                "CHANGED"
            } else {
                "UNCHANGED"
            },
        }
    }
}

#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SummaryData {
    product_listing_id: String,
    product_listing_title_slug_id: String,
    source: SourceData,
    source_listing_id: String,
    title: Option<LocalizedTextData>,
    price: Option<ProductListingPriceData>,
    price_estimate_min: Option<PriceData>,
    price_estimate_max: Option<PriceData>,
    availability: Option<&'static str>,
    lifecycle: &'static str,
    auction_id: Option<String>,
    #[schemars(range(min = 1, max = i64::MAX as u64))]
    expected_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    updated: OffsetDateTime,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SourceData {
    listing_source_id: String,
    name: String,
    slug_id: String,
}
impl From<AdminProductListingSummary> for SummaryData {
    fn from(value: AdminProductListingSummary) -> Self {
        Self {
            product_listing_id: value.product_listing_id.to_string(),
            product_listing_title_slug_id: value.product_listing_title_slug_id.to_string(),
            source: SourceData {
                listing_source_id: value.source.listing_source_id.to_string(),
                name: value.source.name.to_string(),
                slug_id: value.source.slug_id.to_string(),
            },
            source_listing_id: value.source_listing_id.into(),
            title: value.title.map(Into::into),
            price: value.pricing.price.map(Into::into),
            price_estimate_min: value.pricing.price_estimate_min.map(Into::into),
            price_estimate_max: value.pricing.price_estimate_max.map(Into::into),
            availability: value.availability.map(|v| v.as_str()),
            lifecycle: value.lifecycle.as_str(),
            auction_id: value.auction_id.map(|id| id.to_string()),
            expected_version: value.version.into(),
            created: value.created,
            updated: value.updated,
        }
    }
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DetailsData {
    #[serde(flatten)]
    summary: SummaryData,
    description: Option<LocalizedTextData>,
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: Url,
    #[schemars(schema_with = "crate::product_listing_fields::image_urls_schema")]
    images: Vec<Url>,
    auction: Option<AuctionData>,
    sale_observation: Option<SaleObservationData>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct AuctionData {
    auction_id: Option<String>,
    lot_number: Option<String>,
    #[schemars(range(max = u32::MAX))]
    catalogue_position: Option<u32>,
    timing: TimingData,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct TimingData {
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    scheduled_closes: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    #[schemars(schema_with = "crate::wire::schema::optional_datetime")]
    reported_closed_at: Option<OffsetDateTime>,
}
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct SaleObservationData {
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    observed_at: OffsetDateTime,
    fx_rate_id: String,
}
impl From<AdminProductListingDetails> for DetailsData {
    fn from(value: AdminProductListingDetails) -> Self {
        Self {
            summary: value.summary.into(),
            description: value.description.map(Into::into),
            url: value.url,
            images: value
                .images
                .into_iter()
                .map(|image| image.url().clone())
                .collect(),
            auction: value.auction.map(|auction| AuctionData {
                auction_id: auction.auction_id().map(|id| id.to_string()),
                lot_number: auction.lot_number().map(|v| v.to_string()),
                catalogue_position: auction.catalogue_position().map(|value| value.value()),
                timing: TimingData {
                    bidding_opens: auction.bidding_opens(),
                    scheduled_closes: auction.scheduled_closes(),
                    reported_closed_at: auction.reported_closed_at(),
                },
            }),
            sale_observation: value.sale_observation.map(|sale| SaleObservationData {
                observed_at: sale.observed_at(),
                fx_rate_id: sale.fx_rate_id().to_string(),
            }),
        }
    }
}

fn non_nullable_patch<T>(
    value: PatchValue<T>,
    field: &'static str,
) -> Result<PatchField<T>, MutationInputError> {
    match value {
        PatchValue::Omitted => Ok(PatchField::Unchanged),
        PatchValue::Value(value) => Ok(PatchField::Set(value)),
        PatchValue::Null => Err(MutationInputError::invalid(
            field,
            format!("Body field '{field}' must not be null."),
        )),
    }
}
