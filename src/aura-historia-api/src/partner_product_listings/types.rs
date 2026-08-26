use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::values::{LocalizedTextData, PriceData};
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use money::Price;
use product_listing_core::description::Description;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingKey;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_core::prohibited_content::ProhibitedContent;
use product_listing_core::shop_listing_id::ShopListingId;
use product_listing_core::title::Title;
use product_listing_service::use_cases::{
    CreateProductListingCommand, UpdateProductListingCommand, UpsertProductListingCommand,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use shop_core::shop_id::ShopId;
use time::OffsetDateTime;
use url::Url;

pub(super) const MAX_PARTNER_PRODUCT_LISTING_BATCH_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateProductListingData {
    #[serde(rename = "shopListingId")]
    pub(super) shop_listing_id: ShopListingId,
    pub(super) title: LocalizedTextData,
    pub(super) description: LocalizedTextData,
    #[serde(default)]
    pub(super) price: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: Option<PriceData>,
    #[serde(default, with = "crate::wire::listing_availability::option")]
    pub(super) availability: Option<ListingAvailability>,
    pub(super) url: Url,
    pub(super) images: Vec<Url>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) auction_start: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub(super) auction_end: Option<OffsetDateTime>,
    #[serde(default)]
    pub(super) structured_address: Option<StructuredAddressData>,
    #[serde(default)]
    pub(super) geo_address: Option<GeoAddressData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateProductListingData {
    #[serde(rename = "shopListingId")]
    pub(super) shop_listing_id: ShopListingId,
    #[serde(default)]
    pub(super) price: PatchValue<PriceData>,
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
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_start: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_end: PatchValue<OffsetDateTime>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpsertProductListingData {
    #[serde(rename = "shopListingId")]
    pub(super) shop_listing_id: ShopListingId,
    #[serde(default)]
    pub(super) title: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) description: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) price: PatchValue<PriceData>,
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
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_start: PatchValue<OffsetDateTime>,
    #[serde(default, deserialize_with = "crate::patch_value::rfc3339::deserialize")]
    pub(super) auction_end: PatchValue<OffsetDateTime>,
    #[serde(default)]
    pub(super) structured_address: Option<StructuredAddressData>,
    #[serde(default)]
    pub(super) geo_address: Option<GeoAddressData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct WithdrawProductListingData {
    #[serde(rename = "shopListingId")]
    pub(super) shop_listing_id: ShopListingId,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PartnerProductFailureData {
    shop_id: ShopId,
    #[serde(rename = "shopListingId")]
    shop_listing_id: ShopListingId,
    error: ApiErrorCode,
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
    pub(super) fn into_command(self, shop_id: ShopId) -> CreateProductListingCommand {
        CreateProductListingCommand {
            shop_id,
            seller_id: shop_id,
            shop_listing_id: self.shop_listing_id,
            address: product_address(self.structured_address, self.geo_address),
            title: Some(title(self.title)),
            description: Some(description(self.description)),
            pricing: ProductListingPricing {
                price: self.price.map(price),
                price_estimate_min: self.price_estimate_min.map(price),
                price_estimate_max: self.price_estimate_max.map(price),
            },
            availability: self.availability,
            url: self.url,
            images: product_images(self.images),
            auction: ProductListingAuction {
                start: self.auction_start,
                end: self.auction_end,
            },
        }
    }
}

impl UpdateProductListingData {
    pub(super) fn into_key_and_command(
        self,
        shop_id: ShopId,
    ) -> Result<(ProductListingKey, UpdateProductListingCommand), ApiError> {
        let product_key = ProductListingKey::new(shop_id, self.shop_listing_id);
        let command = UpdateProductListingCommand {
            price: clearable(self.price.map(price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: non_nullable_patch(self.url, "url")?,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction_start: clearable(self.auction_start.map(Some)),
            auction_end: clearable(self.auction_end.map(Some)),
            ..Default::default()
        };
        Ok((product_key, command))
    }
}

impl UpsertProductListingData {
    pub(super) fn into_command(
        self,
        shop_id: ShopId,
    ) -> Result<UpsertProductListingCommand, ApiError> {
        Ok(UpsertProductListingCommand {
            shop_id,
            seller_id: shop_id,
            shop_listing_id: self.shop_listing_id,
            address: product_address(self.structured_address, self.geo_address),
            title: self.title.map(title),
            description: self.description.map(description),
            price: clearable(self.price.map(price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            availability: clearable(self.availability),
            url: self.url,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction_start: clearable(self.auction_start),
            auction_end: clearable(self.auction_end),
        })
    }
}

impl WithdrawProductListingData {
    pub(super) fn into_product_key(self, shop_id: ShopId) -> ProductListingKey {
        ProductListingKey::new(shop_id, self.shop_listing_id)
    }
}

impl PartnerProductFailureData {
    pub(super) fn new(
        shop_id: ShopId,
        shop_listing_id: ShopListingId,
        error: ApiErrorCode,
    ) -> Self {
        Self {
            shop_id,
            shop_listing_id,
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

fn product_address(
    structured: Option<StructuredAddressData>,
    geo: Option<GeoAddressData>,
) -> ProductListingAddress {
    ProductListingAddress {
        structured: structured.map(Into::into),
        geo: geo.map(Into::into),
    }
}

fn product_images(values: Vec<Url>) -> indexmap::IndexSet<ProductListingImage> {
    values
        .into_iter()
        .map(|url| ProductListingImage {
            url,
            prohibited_content: ProhibitedContent::Unknown,
        })
        .collect()
}
