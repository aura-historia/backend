use common::has_key::HasKey;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use common::price::domain::Price;
use common::product_id::ProductKey;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use product::core::product_image::ProductImage;
use product::core::prohibited_content::ProhibitedContent;
use product::data::patch_product_data::PatchProductData;
use product::data::post_product_data::PostProductData;
use product::data::product_state_data::ProductStateData;
use product::data::put_product_data::PutProductData;
use product::service::product_command::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AsyncProductCommandData {
    Create(CreateAsyncProductCommandData),
    Update(UpdateAsyncProductCommandData),
    Upsert(UpsertAsyncProductCommandData),
}

impl AsyncProductCommandData {
    pub fn key(&self) -> ProductKey {
        match self {
            AsyncProductCommandData::Create(cmd) => cmd.key(),
            AsyncProductCommandData::Update(cmd) => cmd.key(),
            AsyncProductCommandData::Upsert(cmd) => cmd.key(),
        }
    }

    pub fn shops_product_id(&self) -> &ShopsProductId {
        match self {
            AsyncProductCommandData::Create(cmd) => &cmd.shops_product_id,
            AsyncProductCommandData::Update(cmd) => &cmd.shops_product_id,
            AsyncProductCommandData::Upsert(cmd) => &cmd.shops_product_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAsyncProductCommandData {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub title: LocalizedTextData,
    pub description: LocalizedTextData,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,
    pub state: ProductStateData,
    pub url: Url,
    pub images: Vec<Url>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_end: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seller_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<GeoAddressData>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAsyncProductCommandData {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ProductStateData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<Url>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_end: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertAsyncProductCommandData {
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_min: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price_estimate_max: Option<PriceData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub state: Option<ProductStateData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub images: Option<Vec<Url>>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_start: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        with = "time::serde::rfc3339::option"
    )]
    pub auction_end: Option<OffsetDateTime>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub seller_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<GeoAddressData>,
}

impl HasKey for CreateAsyncProductCommandData {
    type Key = ProductKey;
    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}
impl HasKey for UpdateAsyncProductCommandData {
    type Key = ProductKey;
    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}
impl HasKey for UpsertAsyncProductCommandData {
    type Key = ProductKey;
    fn key(&self) -> Self::Key {
        ProductKey::new(self.shop_id, self.shops_product_id.clone())
    }
}

impl From<(ShopId, PostProductData)> for CreateAsyncProductCommandData {
    fn from((shop_id, data): (ShopId, PostProductData)) -> Self {
        Self {
            shop_id,
            shops_product_id: data.shops_product_id,
            title: data.title,
            description: data.description,
            price: data.price,
            price_estimate_min: data.price_estimate_min,
            price_estimate_max: data.price_estimate_max,
            state: data.state,
            url: data.url,
            images: data.images,
            auction_start: data.auction_start,
            auction_end: data.auction_end,
            seller_name: data.seller_name,
            structured_address: data.structured_address,
            geo_address: data.geo_address,
        }
    }
}
impl From<(ShopId, PatchProductData)> for UpdateAsyncProductCommandData {
    fn from((shop_id, data): (ShopId, PatchProductData)) -> Self {
        Self {
            shop_id,
            shops_product_id: data.shops_product_id,
            price: data.price,
            state: data.state,
            price_estimate_min: data.price_estimate_min,
            price_estimate_max: data.price_estimate_max,
            url: data.url,
            images: data.images,
            auction_start: data.auction_start,
            auction_end: data.auction_end,
        }
    }
}
impl From<(ShopId, PutProductData)> for UpsertAsyncProductCommandData {
    fn from((shop_id, data): (ShopId, PutProductData)) -> Self {
        Self {
            shop_id,
            shops_product_id: data.shops_product_id,
            title: data.title,
            description: data.description,
            price: data.price,
            price_estimate_min: data.price_estimate_min,
            price_estimate_max: data.price_estimate_max,
            state: data.state,
            url: data.url,
            images: data.images,
            auction_start: data.auction_start,
            auction_end: data.auction_end,
            seller_name: data.seller_name,
            structured_address: data.structured_address,
            geo_address: data.geo_address,
        }
    }
}

impl From<CreateAsyncProductCommandData> for CreateProductCommand {
    fn from(data: CreateAsyncProductCommandData) -> Self {
        CreateProductCommand {
            shop_id: data.shop_id,
            shops_product_id: data.shops_product_id,
            seller_name_raw: data.seller_name,
            structured_address: data.structured_address.map(Into::into),
            geo_address: data.geo_address.map(Into::into),
            native_title: data.title.into(),
            other_title: HashMap::new(),
            native_description: Some(data.description.into()),
            native_price: data.price.map(Price::from),
            other_price: HashMap::new(),
            native_price_estimate_min: data.price_estimate_min.map(Price::from),
            other_price_estimate_min: HashMap::new(),
            native_price_estimate_max: data.price_estimate_max.map(Price::from),
            other_price_estimate_max: HashMap::new(),
            state: data.state.into(),
            url: data.url,
            images: data
                .images
                .into_iter()
                .map(|url| ProductImage {
                    url,
                    prohibited_content: ProhibitedContent::default(),
                })
                .collect(),
            auction_start: data.auction_start,
            auction_end: data.auction_end,
        }
    }
}
impl From<UpdateAsyncProductCommandData> for (ProductKey, UpdateProductCommand) {
    fn from(data: UpdateAsyncProductCommandData) -> Self {
        let key = data.key();
        let cmd = UpdateProductCommand {
            native_price: data.price.map(Price::from),
            state: data.state.map(Into::into),
            native_price_estimate_min: data.price_estimate_min.map(Price::from),
            native_price_estimate_max: data.price_estimate_max.map(Price::from),
            url: data.url,
            images: data.images.map(|images| {
                images
                    .into_iter()
                    .map(|url| ProductImage {
                        url,
                        prohibited_content: ProhibitedContent::default(),
                    })
                    .collect()
            }),
            auction_start: data.auction_start,
            auction_end: data.auction_end,
            embedding: None,
            translated_titles: None,
        };
        (key, cmd)
    }
}
impl From<UpsertAsyncProductCommandData> for UpsertProductCommand {
    fn from(data: UpsertAsyncProductCommandData) -> Self {
        UpsertProductCommand {
            shop_id: data.shop_id,
            shops_product_id: data.shops_product_id,
            seller_name_raw: data.seller_name,
            structured_address: data.structured_address.map(Into::into),
            geo_address: data.geo_address.map(Into::into),
            native_title: data.title.map(Into::into),
            native_description: data.description.map(Into::into),
            native_price: data.price.map(Price::from),
            native_price_estimate_min: data.price_estimate_min.map(Price::from),
            native_price_estimate_max: data.price_estimate_max.map(Price::from),
            state: data.state.map(ProductState::from),
            url: data.url,
            images: data
                .images
                .unwrap_or_default()
                .into_iter()
                .map(|url| ProductImage {
                    url,
                    prohibited_content: ProhibitedContent::default(),
                })
                .collect(),
            auction_start: data.auction_start,
            auction_end: data.auction_end,
        }
    }
}
impl From<UpsertProductCommand> for UpsertAsyncProductCommandData {
    fn from(cmd: UpsertProductCommand) -> Self {
        Self {
            shop_id: cmd.shop_id,
            shops_product_id: cmd.shops_product_id,
            title: cmd.native_title.map(LocalizedTextData::from),
            description: cmd.native_description.map(LocalizedTextData::from),
            price: cmd.native_price.map(PriceData::from),
            price_estimate_min: cmd.native_price_estimate_min.map(PriceData::from),
            price_estimate_max: cmd.native_price_estimate_max.map(PriceData::from),
            state: cmd.state.map(ProductStateData::from),
            url: cmd.url,
            images: Some(cmd.images.into_iter().map(|image| image.url).collect()),
            auction_start: cmd.auction_start,
            auction_end: cmd.auction_end,
            seller_name: cmd.seller_name_raw,
            structured_address: cmd.structured_address.map(Into::into),
            geo_address: cmd.geo_address.map(Into::into),
        }
    }
}
