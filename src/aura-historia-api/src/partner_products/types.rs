use crate::error::{ApiError, ApiErrorCode, BAD_BODY_VALUE};
use crate::patch_value::{PatchValue, clearable, non_nullable_patch};
use crate::values::{LocalizedTextData, PriceData};
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use money::Price;
use product_core::description::Description;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_id::ProductKey;
use product_core::product_image::ProductImage;
use product_core::product_state::ProductState;
use product_core::prohibited_content::ProhibitedContent;
use product_core::shops_product_id::ShopsProductId;
use product_core::title::Title;
use product_service::use_cases::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use shop_core::shop_id::ShopId;
use time::OffsetDateTime;
use url::Url;

pub(super) const MAX_PARTNER_PRODUCT_BATCH_SIZE: usize = 100;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateProductData {
    pub(super) shops_product_id: ShopsProductId,
    pub(super) title: LocalizedTextData,
    pub(super) description: LocalizedTextData,
    #[serde(default)]
    pub(super) price: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: Option<PriceData>,
    pub(super) state: ProductStateData,
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
pub(super) struct UpdateProductData {
    pub(super) shops_product_id: ShopsProductId,
    #[serde(default)]
    pub(super) price: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: PatchValue<PriceData>,
    #[serde(default)]
    pub(super) state: PatchValue<ProductStateData>,
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
pub(super) struct UpsertProductData {
    pub(super) shops_product_id: ShopsProductId,
    #[serde(default)]
    pub(super) title: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) description: Option<LocalizedTextData>,
    #[serde(default)]
    pub(super) price: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_min: Option<PriceData>,
    #[serde(default)]
    pub(super) price_estimate_max: Option<PriceData>,
    #[serde(default)]
    pub(super) state: Option<ProductStateData>,
    #[serde(default)]
    pub(super) url: Option<Url>,
    #[serde(default)]
    pub(super) images: Option<Vec<Url>>,
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
pub(super) struct DeleteProductData {
    pub(super) shops_product_id: ShopsProductId,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PartnerProductFailureData {
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    error: ApiErrorCode,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(super) enum ProductStateData {
    Listed,
    Available,
    Reserved,
    Sold,
    Removed,
    Unknown,
}

pub(super) fn parse_partner_product_batch<T: DeserializeOwned>(
    body: &str,
) -> Result<Vec<T>, ApiError> {
    if body.trim().is_empty() {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail("Body cannot be empty."));
    }

    let products: Vec<T> = serde_json::from_str(body)
        .map_err(|error| ApiError::bad_request(BAD_BODY_VALUE).with_detail(error.to_string()))?;
    if products.len() > MAX_PARTNER_PRODUCT_BATCH_SIZE {
        return Err(ApiError::bad_request(BAD_BODY_VALUE).with_detail(format!(
            "Body cannot contain more than {MAX_PARTNER_PRODUCT_BATCH_SIZE} products."
        )));
    }

    Ok(products)
}

impl CreateProductData {
    pub(super) fn into_command(self, shop_id: ShopId) -> CreateProductCommand {
        CreateProductCommand {
            shop_id,
            seller_id: shop_id,
            shops_product_id: self.shops_product_id,
            address: product_address(self.structured_address, self.geo_address),
            title: Some(title(self.title)),
            description: Some(description(self.description)),
            pricing: ProductPricing {
                price: self.price.map(price),
                price_estimate_min: self.price_estimate_min.map(price),
                price_estimate_max: self.price_estimate_max.map(price),
            },
            state: self.state.into(),
            url: self.url,
            images: product_images(self.images),
            auction: ProductAuction {
                start: self.auction_start,
                end: self.auction_end,
            },
        }
    }
}

impl UpdateProductData {
    pub(super) fn into_key_and_command(
        self,
        shop_id: ShopId,
    ) -> Result<(ProductKey, UpdateProductCommand), ApiError> {
        let product_key = ProductKey::new(shop_id, self.shops_product_id);
        let command = UpdateProductCommand {
            price: clearable(self.price.map(price)),
            price_estimate_min: clearable(self.price_estimate_min.map(price)),
            price_estimate_max: clearable(self.price_estimate_max.map(price)),
            state: non_nullable_patch(self.state.map(Into::into), "state")?,
            url: non_nullable_patch(self.url, "url")?,
            images: non_nullable_patch(self.images.map(product_images), "images")?,
            auction_start: clearable(self.auction_start.map(Some)),
            auction_end: clearable(self.auction_end.map(Some)),
            ..Default::default()
        };
        Ok((product_key, command))
    }
}

impl UpsertProductData {
    pub(super) fn into_command(self, shop_id: ShopId) -> UpsertProductCommand {
        UpsertProductCommand {
            shop_id,
            seller_id: shop_id,
            shops_product_id: self.shops_product_id,
            address: product_address(self.structured_address, self.geo_address),
            title: self.title.map(title),
            description: self.description.map(description),
            price: self.price.map(price),
            price_estimate_min: self.price_estimate_min.map(price),
            price_estimate_max: self.price_estimate_max.map(price),
            state: self.state.map(Into::into),
            url: self.url,
            images: product_images(self.images.unwrap_or_default()),
            auction_start: self.auction_start,
            auction_end: self.auction_end,
        }
    }
}

impl DeleteProductData {
    pub(super) fn into_product_key(self, shop_id: ShopId) -> ProductKey {
        ProductKey::new(shop_id, self.shops_product_id)
    }
}

impl PartnerProductFailureData {
    pub(super) fn new(
        shop_id: ShopId,
        shops_product_id: ShopsProductId,
        error: ApiErrorCode,
    ) -> Self {
        Self {
            shop_id,
            shops_product_id,
            error,
        }
    }
}

impl From<ProductStateData> for ProductState {
    fn from(value: ProductStateData) -> Self {
        match value {
            ProductStateData::Listed => Self::Listed,
            ProductStateData::Available => Self::Available,
            ProductStateData::Reserved => Self::Reserved,
            ProductStateData::Sold => Self::Sold,
            ProductStateData::Removed => Self::Removed,
            ProductStateData::Unknown => Self::Unknown,
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
) -> ProductAddress {
    ProductAddress {
        structured: structured.map(Into::into),
        geo: geo.map(Into::into),
    }
}

fn product_images(values: Vec<Url>) -> indexmap::IndexSet<ProductImage> {
    values
        .into_iter()
        .map(|url| ProductImage {
            url,
            prohibited_content: ProhibitedContent::Unknown,
        })
        .collect()
}
