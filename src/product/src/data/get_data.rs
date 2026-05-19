use crate::core::product::LocalizedProductView;
use crate::data::auction_data::AuctionData;
use crate::data::price_composite_data::{PriceEstimateData, PricingData};
use crate::data::product_image_data::ProductImageData;
use crate::data::product_state_data::ProductStateData;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::language::data::LocalizedTextData;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use geo::data::address_data::{GeoAddressData, StructuredAddressData};
use serde::{Deserialize, Serialize};
use shop::data::shop_type_data::ShopTypeData;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProductData {
    pub product_id: ProductId,
    pub product_slug_id: SlugId<6>,
    pub shop_slug_id: SlugId<0>,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: String,
    pub seller_name: String,
    pub shop_type: ShopTypeData,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub structured_address: Option<StructuredAddressData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub geo_address: Option<GeoAddressData>,
    pub title: LocalizedTextData,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub price: Option<PricingData>,
    pub state: ProductStateData,
    pub url: Url,
    pub view_url: Url,
    #[serde(default)]
    pub images: Vec<ProductImageData>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub auction: Option<AuctionData>,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

impl HasKey for GetProductData {
    type Key = ProductKey;

    fn key(&self) -> Self::Key {
        ProductKey {
            shop_id: self.shop_id,
            shops_product_id: self.shops_product_id.clone(),
        }
    }
}

impl GetProductData {
    pub fn from_view(product_view: LocalizedProductView, prohibited_content_consent: bool) -> Self {
        let estimate = if product_view.price_estimate_min.is_some()
            || product_view.price_estimate_max.is_some()
        {
            Some(PriceEstimateData {
                min: product_view.price_estimate_min.map(Into::into),
                max: product_view.price_estimate_max.map(Into::into),
            })
        } else {
            None
        };
        let price = match product_view.price {
            Some(offer) => Some(PricingData {
                offer: Some(offer.into()),
                estimate,
            }),
            None => estimate.map(|estimate| PricingData {
                offer: None,
                estimate: Some(estimate),
            }),
        };

        GetProductData {
            product_id: product_view.product_id,
            product_slug_id: product_view.product_slug_id,
            shop_slug_id: product_view.shop_slug_id,
            event_id: product_view.event_id,
            shop_id: product_view.shop_id,
            shops_product_id: product_view.shops_product_id,
            shop_name: product_view.shop_name.into(),
            seller_name: product_view.seller_name.into(),
            shop_type: product_view.shop_type.into(),
            structured_address: product_view
                .structured_address
                .map(StructuredAddressData::from),
            geo_address: product_view.geo_address.map(GeoAddressData::from),
            title: product_view.title.into(),
            description: product_view.description.map(LocalizedTextData::from),
            price,
            state: product_view.state.into(),
            url: product_view.url,
            view_url: product_view.view_url,
            images: product_view
                .images
                .into_iter()
                .map(|img| ProductImageData::from_with_consent(img, prohibited_content_consent))
                .collect(),
            auction: match (product_view.auction_start, product_view.auction_end) {
                (start, end @ Some(_)) => Some(AuctionData { start, end }),
                (start @ Some(_), end) => Some(AuctionData { start, end }),
                _ => None,
            },
            created: product_view.created,
            updated: product_view.updated,
        }
    }
}

impl From<LocalizedProductView> for GetProductData {
    fn from(product_view: LocalizedProductView) -> Self {
        GetProductData::from_view(product_view, false)
    }
}
