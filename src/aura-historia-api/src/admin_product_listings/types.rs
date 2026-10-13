use crate::{
    error::{ApiError, BAD_BODY_VALUE, INVALID_OBJECT_ID},
    patch_value::{PatchValue, clearable, non_nullable_patch},
    product_listing_fields::{ProductListingAuctionData, auction_patch, product_images},
    values::{LocalizedTextData, PriceData, ProductListingPriceData},
};
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing_id::ProductListingId,
};
use product_listing_service::{
    ports::{ProductListingStorageVersion, admin_product_listing_reader::*},
    use_cases::commands::{
        admin_product_listings::*, update_product_listing::UpdateProductListingCommand,
    },
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateData {
    expected_version: u64,
    #[serde(default)]
    price: PatchValue<AdminPriceData>,
    #[serde(default)]
    price_estimate_min: PatchValue<MonetaryPriceData>,
    #[serde(default)]
    price_estimate_max: PatchValue<MonetaryPriceData>,
    #[serde(
        default,
        deserialize_with = "crate::wire::listing_availability::patch::deserialize"
    )]
    availability: PatchValue<ListingAvailability>,
    #[serde(default)]
    url: PatchValue<Url>,
    #[serde(default)]
    images: PatchValue<Vec<Url>>,
    #[serde(default)]
    auction: PatchValue<ProductListingAuctionData>,
}

// A struct variant enforces unknown-field rejection for ON_REQUEST as well.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE", deny_unknown_fields)]
enum AdminPriceData {
    Monetary {
        #[serde(with = "crate::wire::currency")]
        currency: money::Currency,
        #[serde(deserialize_with = "storage_amount")]
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonetaryPriceData {
    #[serde(with = "crate::wire::currency")]
    currency: money::Currency,
    #[serde(deserialize_with = "storage_amount")]
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
    pub(super) fn into_command(
        self,
        id: ProductListingId,
    ) -> Result<UpdateAdminProductListingCommand, ApiError> {
        Ok(UpdateAdminProductListingCommand {
            product_listing_id: id,
            expected_version: version(self.expected_version)?,
            patch: UpdateProductListingCommand {
                price: clearable(self.price.map(Into::into)),
                price_estimate_min: clearable(self.price_estimate_min.map(Into::into)),
                price_estimate_max: clearable(self.price_estimate_max.map(Into::into)),
                availability: clearable(self.availability),
                url: non_nullable_patch(self.url, "url")
                    .map_err(|error| error.with_body_field("url"))?,
                images: non_nullable_patch(self.images.map(product_images), "images")
                    .map_err(|error| error.with_body_field("images"))?,
                auction: auction_patch(self.auction).map_err(|error| {
                    let field = if error.code() == INVALID_OBJECT_ID {
                        "auction.auctionId"
                    } else {
                        "auction"
                    };
                    error.with_body_field(field)
                })?,
            },
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct VersionData {
    pub(super) expected_version: u64,
}
pub(super) fn version(value: u64) -> Result<ProductListingStorageVersion, ApiError> {
    i64::try_from(value)
        .ok()
        .and_then(|value| ProductListingStorageVersion::try_from(value).ok())
        .ok_or_else(|| {
            ApiError::bad_request(BAD_BODY_VALUE)
                .with_body_field("expectedVersion")
                .with_detail("expectedVersion must be between 1 and 9223372036854775807.")
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MutationData {
    product_listing_id: String,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SummaryData {
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
    expected_version: u64,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
}
#[derive(Serialize)]
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
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DetailsData {
    #[serde(flatten)]
    summary: SummaryData,
    description: Option<LocalizedTextData>,
    url: Url,
    images: Vec<Url>,
    auction: Option<AuctionData>,
    sale_observation: Option<SaleObservationData>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuctionData {
    auction_id: Option<String>,
    lot_number: Option<String>,
    catalogue_position: Option<u32>,
    timing: TimingData,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TimingData {
    #[serde(with = "time::serde::rfc3339::option")]
    bidding_opens: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    scheduled_closes: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    reported_closed_at: Option<OffsetDateTime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SaleObservationData {
    #[serde(with = "time::serde::rfc3339")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use application::patch_field::PatchField;
    use serde_json::json;

    #[test]
    fn requires_storage_representable_version_and_rejects_read_only_fields() {
        for body in [
            json!({}),
            json!({"expectedVersion":0}),
            json!({"expectedVersion":u64::MAX}),
            json!({"expectedVersion":null}),
            json!({"expectedVersion":1,"title":null}),
            json!({"expectedVersion":1,"lifecycle":"ACTIVE"}),
            json!({"expectedVersion":1,"productListingId":"x"}),
            json!({"expectedVersion":1,"saleObservation":null}),
            json!({"expectedVersion":1,"description":"x"}),
        ] {
            assert!(
                serde_json::from_value::<UpdateData>(body)
                    .and_then(|data| data
                        .into_command(ProductListingId::new())
                        .map_err(serde::de::Error::custom))
                    .is_err()
            );
        }
    }
    #[test]
    fn rejects_unknown_nested_fields_and_nonnullable_containers() {
        for body in [
            json!({"price":{"type":"ON_REQUEST","amount":1}}),
            json!({"priceEstimateMin":{"currency":"EUR","amount":1,"extra":true}}),
            json!({"priceEstimateMin":{"type":"ON_REQUEST"}}),
            json!({"price":{"type":"MONETARY","currency":"EUR","amount":u64::MAX}}),
            json!({"auction":{"other":1}}),
            json!({"auction":{"timing":{"other":1}}}),
            json!({"auction":null}),
            json!({"images":null}),
            json!({"images":[null]}),
            json!({"url":null}),
        ] {
            let mut body = body;
            body["expectedVersion"] = json!(1);
            assert!(
                serde_json::from_value::<UpdateData>(body)
                    .and_then(|data| data
                        .into_command(ProductListingId::new())
                        .map_err(serde::de::Error::custom))
                    .is_err()
            );
        }
    }
    #[test]
    fn preserves_omissions_and_clears_only_explicit_optional_facts() {
        let command = serde_json::from_value::<UpdateData>(json!({"expectedVersion":1,"price":null,"priceEstimateMin":null,"availability":null,"images":[],"auction":{"auctionId":null,"timing":null}})).unwrap().into_command(ProductListingId::new()).unwrap();
        assert!(matches!(command.patch.price, PatchField::Clear));
        assert!(matches!(
            command.patch.price_estimate_min,
            PatchField::Clear
        ));
        assert!(matches!(
            command.patch.price_estimate_max,
            PatchField::Unchanged
        ));
        assert!(matches!(command.patch.availability, PatchField::Clear));
        assert!(matches!(command.patch.url, PatchField::Unchanged));
        assert!(matches!(command.patch.images, PatchField::Set(ref values) if values.is_empty()));
        let PatchField::Set(auction) = command.patch.auction else {
            panic!("auction patch")
        };
        assert!(matches!(auction.auction_id, PatchField::Clear));
        assert!(matches!(auction.lot_number, PatchField::Unchanged));
        assert!(matches!(auction.bidding_opens, PatchField::Unchanged));
    }
}
