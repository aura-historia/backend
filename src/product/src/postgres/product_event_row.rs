use crate::core::product_event::domain::{
    ProductCreatedDomainEventPayload, ProductDomainEventPayload,
    ProductEstimatePriceChangeDomainEventPayload, ProductImagesChangeDomainEventPayload,
    ProductPriceChangeDomainEventPayload,
};
use crate::core::product_event::{ProductEvent, ProductEventPayload};
use crate::core::product_image::ProductImage;
use common::actor::domain::Actor;
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::localized::Localized;
use common::price::domain::{MonetaryAmount, Price};
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use geo::core::address::{GeoAddress, StructuredAddress};
use indexmap::IndexSet;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use time::OffsetDateTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProductEventGroup {
    Domain,
    Enrichment,
    Policy,
    Lifecycle,
}

impl ProductEventGroup {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Domain => "DOMAIN",
            Self::Enrichment => "ENRICHMENT",
            Self::Policy => "POLICY",
            Self::Lifecycle => "LIFECYCLE",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductEventRow {
    pub event_id: EventId,
    pub product_id: ProductId,
    pub shop_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub event_type: String,
    pub event_group: ProductEventGroup,
    pub event_type_schema_version: i32,
    pub payload: Value,
    pub event_time: OffsetDateTime,
    pub created_by: Actor,
}

impl ProductEventRow {
    pub fn from_event(event: ProductEvent) -> Self {
        let key = event.payload.key();
        let event_type = event.payload.event_type().to_owned();
        let event_group = event_group(&event.payload);
        let payload = payload_json(&event.payload);

        Self {
            event_id: event.event_id,
            product_id: event.aggregate_id,
            shop_id: key.shop_id,
            shops_product_id: key.shops_product_id,
            event_type,
            event_group,
            event_type_schema_version: 1,
            payload,
            event_time: event.timestamp,
            created_by: Actor::System,
        }
    }
}

impl From<ProductEvent> for ProductEventRow {
    fn from(event: ProductEvent) -> Self {
        Self::from_event(event)
    }
}

fn event_group(payload: &ProductEventPayload) -> ProductEventGroup {
    match payload {
        ProductEventPayload::ProductDomainEvent(_) => ProductEventGroup::Domain,
        ProductEventPayload::ProductEnrichmentEvent(_) => ProductEventGroup::Enrichment,
        ProductEventPayload::ProductPolicyEvent(_) => ProductEventGroup::Policy,
        ProductEventPayload::ProductLifecycleEvent(_) => ProductEventGroup::Lifecycle,
    }
}

fn payload_json(payload: &ProductEventPayload) -> Value {
    match payload {
        ProductEventPayload::ProductDomainEvent(payload) => domain_payload_json(payload),
        ProductEventPayload::ProductEnrichmentEvent(payload) => match payload {
            crate::core::product_event::enrichment::ProductEnrichmentEventPayload::TranslatedTitle(payload) => json!({
                "shop_id": payload.shop_id.to_string(),
                "seller_id": payload.seller_id.to_string(),
                "shops_product_id": payload.shops_product_id.to_string(),
                "source_language": payload.source_language.as_str(),
                "target_language": payload.target_language.as_str(),
                "target": payload.target.as_ref(),
            }),
            crate::core::product_event::enrichment::ProductEnrichmentEventPayload::Embedded(payload) => json!({
                "shop_id": payload.shop_id.to_string(),
                "seller_id": payload.seller_id.to_string(),
                "shops_product_id": payload.shops_product_id.to_string(),
                "embedding": payload.embedding,
                "native_title": payload.native_title.as_ref().map(localized_title_json),
            }),
        },
        ProductEventPayload::ProductLifecycleEvent(payload) => match payload {
            crate::core::product_event::lifecycle::ProductLifecycleEventPayload::Deleted(payload) => json!({
                "shop_id": payload.shop_id.to_string(),
                "seller_id": payload.seller_id.to_string(),
                "shops_product_id": payload.shops_product_id.to_string(),
                "old_lifecycle": product_lifecycle_str(payload.old_lifecycle),
                "new_lifecycle": product_lifecycle_str(payload.new_lifecycle),
            }),
        },
        ProductEventPayload::ProductPolicyEvent(payload) => match payload {
            crate::core::product_event::policy::ProductPolicyEventPayload::ProhibitedContentDecision(payload) => json!({
                "shop_id": payload.shop_id.to_string(),
                "seller_id": payload.seller_id.to_string(),
                "shops_product_id": payload.shops_product_id.to_string(),
                "decision": payload.decision.as_str(),
                "reason": payload.reason.as_str(),
            }),
        },
    }
}

fn domain_payload_json(payload: &ProductDomainEventPayload) -> Value {
    match payload {
        ProductDomainEventPayload::Created(payload) => created_payload_json(payload),
        ProductDomainEventPayload::StateChanged(payload) => json!({
            "shop_id": payload.shop_id.to_string(),
            "seller_id": payload.seller_id.to_string(),
            "shops_product_id": payload.shops_product_id.to_string(),
            "old_state": product_state_str(payload.old_state),
            "new_state": product_state_str(payload.new_state),
        }),
        ProductDomainEventPayload::PriceChanged(payload) => price_changed_payload_json(payload),
        ProductDomainEventPayload::EstimatePriceChanged(payload) => {
            estimate_price_changed_payload_json(payload)
        }
        ProductDomainEventPayload::UrlChanged(payload) => json!({
            "shop_id": payload.shop_id.to_string(),
            "seller_id": payload.seller_id.to_string(),
            "shops_product_id": payload.shops_product_id.to_string(),
            "url": payload.url.as_str(),
            "view_url": payload.view_url.as_str(),
        }),
        ProductDomainEventPayload::ImagesChanged(payload) => images_changed_payload_json(payload),
        ProductDomainEventPayload::AuctionTimeChanged(payload) => json!({
            "shop_id": payload.shop_id.to_string(),
            "seller_id": payload.seller_id.to_string(),
            "shops_product_id": payload.shops_product_id.to_string(),
            "auction_start": payload.auction_start,
            "auction_end": payload.auction_end,
        }),
    }
}

fn created_payload_json(payload: &ProductCreatedDomainEventPayload) -> Value {
    json!({
        "product_slug_id": payload.product_slug_id.to_string(),
        "shop_slug_id": payload.shop_slug_id.to_string(),
        "seller_slug_id": payload.seller_slug_id.to_string(),
        "shop_id": payload.shop_id.to_string(),
        "seller_id": payload.seller_id.to_string(),
        "shops_product_id": payload.shops_product_id.to_string(),
        "shop_name": payload.shop_name.to_string(),
        "seller_name": payload.seller_name.to_string(),
        "shop_type": shop_type_str(payload.shop_type),
        "structured_address": payload.structured_address.as_ref().map(structured_address_json),
        "geo_address": payload.geo_address.as_ref().map(geo_address_json),
        "native_title": localized_title_json(&payload.native_title),
        "native_description": payload.native_description.as_ref().map(localized_description_json),
        "native_price": payload.native_price.as_ref().map(price_json),
        "other_price": money_map_json(&payload.other_price),
        "native_price_estimate_min": payload.native_price_estimate_min.as_ref().map(price_json),
        "other_price_estimate_min": money_map_json(&payload.other_price_estimate_min),
        "native_price_estimate_max": payload.native_price_estimate_max.as_ref().map(price_json),
        "other_price_estimate_max": money_map_json(&payload.other_price_estimate_max),
        "state": product_state_str(payload.state),
        "url": payload.url.as_str(),
        "view_url": payload.view_url.as_str(),
        "images": images_json(&payload.images),
        "auction_start": payload.auction_start,
        "auction_end": payload.auction_end,
    })
}

fn price_changed_payload_json(payload: &ProductPriceChangeDomainEventPayload) -> Value {
    json!({
        "shop_id": payload.shop_id.to_string(),
        "seller_id": payload.seller_id.to_string(),
        "shops_product_id": payload.shops_product_id.to_string(),
        "old_native_price": payload.old_native_price.as_ref().map(price_json),
        "old_other_price": money_map_json(&payload.old_other_price),
        "new_native_price": payload.new_native_price.as_ref().map(price_json),
        "new_other_price": money_map_json(&payload.new_other_price),
    })
}

fn estimate_price_changed_payload_json(
    payload: &ProductEstimatePriceChangeDomainEventPayload,
) -> Value {
    json!({
        "shop_id": payload.shop_id.to_string(),
        "seller_id": payload.seller_id.to_string(),
        "shops_product_id": payload.shops_product_id.to_string(),
        "native_price_estimate_min": payload.native_price_estimate_min.as_ref().map(price_json),
        "other_price_estimate_min": money_map_json(&payload.other_price_estimate_min),
        "native_price_estimate_max": payload.native_price_estimate_max.as_ref().map(price_json),
        "other_price_estimate_max": money_map_json(&payload.other_price_estimate_max),
    })
}

fn images_changed_payload_json(payload: &ProductImagesChangeDomainEventPayload) -> Value {
    json!({
        "shop_id": payload.shop_id.to_string(),
        "seller_id": payload.seller_id.to_string(),
        "shops_product_id": payload.shops_product_id.to_string(),
        "images": images_json(&payload.images),
    })
}

fn localized_title_json(
    localized: &Localized<common::language::domain::Language, crate::core::title::Title>,
) -> Value {
    json!({
        "language": localized.localization.as_str(),
        "text": localized.payload.as_ref(),
    })
}

fn localized_description_json(
    localized: &Localized<
        common::language::domain::Language,
        crate::core::description::Description,
    >,
) -> Value {
    json!({
        "language": localized.localization.as_str(),
        "text": localized.payload.as_ref(),
    })
}

fn structured_address_json(address: &StructuredAddress) -> Value {
    json!({
        "addressline": address.addressline,
        "addressline_extra": address.addressline_extra,
        "locality": address.locality,
        "region": address.region,
        "postal_code": address.postal_code,
        "country": address.country.map(|country| country.alpha2()),
    })
}

fn geo_address_json(address: &GeoAddress) -> Value {
    json!({
        "lat": address.lat,
        "lon": address.lon,
    })
}

fn price_json(price: &Price) -> Value {
    json!({
        "amount": u64::from(price.monetary_amount),
        "currency": price.currency.as_str(),
    })
}

fn money_map_json(values: &HashMap<Currency, MonetaryAmount>) -> Value {
    let object = values
        .iter()
        .map(|(currency, amount)| (currency.as_str().to_owned(), json!(u64::from(*amount))))
        .collect();
    Value::Object(object)
}

fn images_json(images: &IndexSet<ProductImage>) -> Value {
    Value::Array(
        images
            .iter()
            .map(|image| {
                json!({
                    "url": image.url.as_str(),
                    "prohibited_content": image.prohibited_content.as_str(),
                })
            })
            .collect(),
    )
}

fn shop_type_str(value: shop::core::shop_type::ShopType) -> &'static str {
    match value {
        shop::core::shop_type::ShopType::AuctionHouse => "AUCTION_HOUSE",
        shop::core::shop_type::ShopType::AuctionPlatform => "AUCTION_PLATFORM",
        shop::core::shop_type::ShopType::CommercialDealer => "COMMERCIAL_DEALER",
        shop::core::shop_type::ShopType::Marketplace => "MARKETPLACE",
    }
}

fn product_state_str(value: common::product_state::domain::ProductState) -> &'static str {
    match value {
        common::product_state::domain::ProductState::Listed => "LISTED",
        common::product_state::domain::ProductState::Available => "AVAILABLE",
        common::product_state::domain::ProductState::Reserved => "RESERVED",
        common::product_state::domain::ProductState::Sold => "SOLD",
        common::product_state::domain::ProductState::Removed => "REMOVED",
        common::product_state::domain::ProductState::Unknown => "UNKNOWN",
    }
}

fn product_lifecycle_str(
    value: common::product_lifecycle::domain::ProductLifecycle,
) -> &'static str {
    match value {
        common::product_lifecycle::domain::ProductLifecycle::Active => "ACTIVE",
        common::product_lifecycle::domain::ProductLifecycle::Deleted => "DELETED",
    }
}

trait ProductEventKey {
    fn key(&self) -> ProductKey;
}

impl ProductEventKey for ProductEventPayload {
    fn key(&self) -> ProductKey {
        use common::has_key::HasKey;
        HasKey::key(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product::Product;
    use crate::core::title::Title;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::product_state::domain::ProductState;
    use common::shop_name::ShopName;
    use shop::core::shop_type::ShopType;
    use url::Url;

    #[test]
    fn should_map_domain_event_payload_without_transport_fields() {
        let event = Product::create(
            ShopId::new(),
            ShopId::new(),
            ShopsProductId::from("external-1"),
            ShopName::from("Shop One"),
            ShopName::from("Shop One"),
            ShopType::AuctionHouse,
            None,
            None,
            Localized::new(Language::En, Title::from("A vase")),
            None,
            None,
            Default::default(),
            None,
            Default::default(),
            None,
            Default::default(),
            ProductState::Listed,
            Url::parse("https://shop.example.com/products/external-1").unwrap(),
            Url::parse("https://aura.example.com/shops/shop-one/products/product-one").unwrap(),
            [],
            None,
            None,
        );
        let row = ProductEventRow::from_event(ProductEvent {
            aggregate_id: event.aggregate_id,
            event_id: event.event_id,
            timestamp: event.timestamp,
            payload: ProductEventPayload::ProductDomainEvent(event.payload),
        });

        assert_eq!(ProductEventGroup::Domain, row.event_group);
        assert_eq!("DOMAIN_CREATED", row.event_type);
        assert!(row.payload.get("pk").is_none());
        assert!(row.payload.get("event_id").is_none());
        assert!(row.payload.get("product_slug_id").is_some());
    }
}
