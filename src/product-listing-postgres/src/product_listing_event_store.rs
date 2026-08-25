#![allow(dead_code)]

use domain_primitives::event_id::EventId;
use product_listing_core::product_listing::{
    ProductListingAddress, ProductListingAuction, ProductListingDomainEvent,
    ProductListingDomainEventPayload, ProductListingPricing,
};
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_image::ProductListingImage;
use product_listing_service::ports::product_listing_event_store::{
    ProductListingEventStore, ProductListingEventStoreError, ProductListingEventStoreFactory,
};
use serde_json::{Value, json};
use sqlx::PgConnection;

#[derive(Debug, Clone, Copy, Default)]
pub struct SqlxProductListingEventStoreFactory;

struct SqlxProductListingEventStore<'tx> {
    connection: &'tx mut PgConnection,
}

impl SqlxProductListingEventStoreFactory {
    pub fn new() -> Self {
        Self
    }
}

impl ProductListingEventStoreFactory<platform_postgres::SqlxTransaction>
    for SqlxProductListingEventStoreFactory
{
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut platform_postgres::SqlxTransaction,
    ) -> impl ProductListingEventStore + 'tx {
        SqlxProductListingEventStore {
            connection: tx.connection(),
        }
    }
}

#[async_trait::async_trait]
impl ProductListingEventStore for SqlxProductListingEventStore<'_> {
    async fn append(
        &mut self,
        event: &ProductListingDomainEvent,
    ) -> Result<(), ProductListingEventStoreError> {
        sqlx::query(
            r#"
            INSERT INTO product_events (
                event_id, product_id, event_type, event_group, payload, event_time
            ) VALUES ($1, $2, $3, 'DOMAIN', $4, $5)
            "#,
        )
        .bind(uuid::Uuid::from(event.event_id))
        .bind(uuid::Uuid::from(event.aggregate_id))
        .bind(event.payload.event_type())
        .bind(event_payload_json(&event.payload))
        .bind(event.timestamp)
        .execute(&mut *self.connection)
        .await
        .map_err(ProductListingEventAppendSqlxError)?;

        Ok(())
    }

    async fn find_current_event_id(
        &mut self,
        product_id: ProductListingId,
    ) -> Result<Option<EventId>, ProductListingEventStoreError> {
        let event_id = sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT event_id FROM product_events WHERE product_id = $1 ORDER BY event_time DESC, event_id DESC LIMIT 1",
        )
        .bind(uuid::Uuid::from(product_id))
        .fetch_optional(&mut *self.connection)
        .await
        .map_err(ProductListingCurrentEventLookupSqlxError)?;

        Ok(event_id.map(EventId::from))
    }
}

struct ProductListingEventAppendSqlxError(sqlx::Error);
struct ProductListingCurrentEventLookupSqlxError(sqlx::Error);

impl From<ProductListingEventAppendSqlxError> for ProductListingEventStoreError {
    fn from(value: ProductListingEventAppendSqlxError) -> Self {
        let ProductListingEventAppendSqlxError(error) = value;
        match &error {
            sqlx::Error::Database(db_error) if db_error.is_unique_violation() => {
                Self::ProductListingEventAlreadyExists
            }
            _ => Self::ProductListingEventAppendFailed,
        }
    }
}

impl From<ProductListingCurrentEventLookupSqlxError> for ProductListingEventStoreError {
    fn from(value: ProductListingCurrentEventLookupSqlxError) -> Self {
        let ProductListingCurrentEventLookupSqlxError(_error) = value;
        Self::CurrentProductListingEventLookupFailed
    }
}

fn event_payload_json(payload: &ProductListingDomainEventPayload) -> Value {
    match payload {
        ProductListingDomainEventPayload::Created(payload) => json!({
            "kind": "created",
            "title": payload.title.as_ref().map(localized_title_json),
            "description": payload.description.as_ref().map(localized_description_json),
            "address": address_json(&payload.address),
            "pricing": pricing_json(payload.pricing),
            "saleValuation": sale_valuation_json(payload.sale_valuation),
            "state": format!("{:?}", payload.state),
            "url": payload.url.as_str(),
            "images": images_json(&payload.images),
            "auction": auction_json(payload.auction),
        }),
        ProductListingDomainEventPayload::StateChanged(payload) => json!({
            "kind": "stateChanged",
            "oldState": format!("{:?}", payload.old_state),
            "newState": format!("{:?}", payload.new_state),
            "saleValuation": sale_valuation_json(payload.sale_valuation),
        }),
        ProductListingDomainEventPayload::AddressChanged(payload) => json!({
            "kind": "addressChanged",
            "address": address_json(&payload.address),
        }),
        ProductListingDomainEventPayload::PriceChanged(payload) => json!({
            "kind": "priceChanged",
            "oldPricing": pricing_json(payload.old_pricing),
            "newPricing": pricing_json(payload.new_pricing),
        }),
        ProductListingDomainEventPayload::UrlChanged(payload) => json!({
            "kind": "urlChanged",
            "oldUrl": payload.old_url.as_str(),
            "newUrl": payload.new_url.as_str(),
        }),
        ProductListingDomainEventPayload::ImagesChanged(payload) => json!({
            "kind": "imagesChanged",
            "images": images_json(&payload.images),
        }),
        ProductListingDomainEventPayload::AuctionChanged(payload) => json!({
            "kind": "auctionChanged",
            "auction": auction_json(payload.auction),
        }),
        ProductListingDomainEventPayload::Deleted(payload) => json!({
            "kind": "deleted",
            "oldLifecycle": format!("{:?}", payload.old_lifecycle),
            "newLifecycle": format!("{:?}", payload.new_lifecycle),
        }),
    }
}

fn localized_title_json(
    title: &localization::Localized<localization::Language, product_listing_core::title::Title>,
) -> Value {
    json!({
        "language": title.localization.as_str(),
        "text": title.payload.as_ref(),
    })
}

fn localized_description_json(
    description: &localization::Localized<
        localization::Language,
        product_listing_core::description::Description,
    >,
) -> Value {
    json!({
        "language": description.localization.as_str(),
        "text": description.payload.as_ref(),
    })
}

fn address_json(address: &ProductListingAddress) -> Value {
    json!({
        "structured": address.structured.as_ref().map(|structured| json!({
            "addressline": structured.addressline,
            "addresslineExtra": structured.addressline_extra,
            "locality": structured.locality,
            "region": structured.region,
            "postalCode": structured.postal_code,
            "country": structured.country.map(|country| country.alpha3()),
        })),
        "geo": address.geo.map(|geo| json!({
            "lat": geo.lat,
            "lon": geo.lon,
        })),
    })
}

fn pricing_json(pricing: ProductListingPricing) -> Value {
    json!({
        "price": pricing.price.map(price_json),
        "priceEstimateMin": pricing.price_estimate_min.map(price_json),
        "priceEstimateMax": pricing.price_estimate_max.map(price_json),
    })
}

fn sale_valuation_json(
    valuation: Option<product_listing_core::product_listing::ProductSaleValuation>,
) -> Value {
    valuation.map_or(Value::Null, |valuation| {
        json!({
            "soldAt": valuation.sold_at.to_string(),
            "fxRateId": valuation.fx_rate_id.to_string(),
        })
    })
}

fn price_json(price: money::Price) -> Value {
    json!({
        "amount": u64::from(price.monetary_amount),
        "currency": price.currency.as_str(),
    })
}

fn images_json(images: &indexmap::IndexSet<ProductListingImage>) -> Value {
    Value::Array(
        images
            .iter()
            .map(|image| {
                json!({
                    "url": image.url.as_str(),
                    "prohibitedContent": image.prohibited_content.as_str(),
                })
            })
            .collect(),
    )
}

fn auction_json(auction: ProductListingAuction) -> Value {
    json!({
        "start": auction.start.map(|value| value.to_string()),
        "end": auction.end.map(|value| value.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use money::Currency;

    use indexmap::IndexSet;
    use localization::Language;
    use localization::Localized;
    use money::{MonetaryAmount, Price};
    use product_listing_core::description::Description;
    use product_listing_core::product_lifecycle::ProductLifecycle;
    use product_listing_core::product_listing::{
        ProductListingAddressChanged, ProductListingAuctionChanged, ProductListingCreated,
        ProductListingDeleted, ProductListingImagesChanged, ProductListingPriceChanged,
        ProductListingUrlChanged, ProductStateChanged,
    };
    use product_listing_core::product_listing_image::ProductListingImage;
    use product_listing_core::product_state::ProductState;
    use product_listing_core::prohibited_content::ProhibitedContent;
    use product_listing_core::title::Title;
    use time::OffsetDateTime;
    use url::Url;

    fn url(value: &str) -> Url {
        match Url::parse(value) {
            Ok(url) => url,
            Err(error) => panic!("invalid test URL: {error}"),
        }
    }

    fn price(amount: u64, currency: Currency) -> Price {
        Price::new(MonetaryAmount::from(amount), currency)
    }

    fn pricing() -> ProductListingPricing {
        ProductListingPricing {
            price: Some(price(1_200, Currency::Eur)),
            price_estimate_min: Some(price(1_000, Currency::Eur)),
            price_estimate_max: Some(price(1_400, Currency::Eur)),
        }
    }

    fn images() -> IndexSet<ProductListingImage> {
        [ProductListingImage {
            url: url("https://shop.example/image.jpg"),
            prohibited_content: ProhibitedContent::None,
        }]
        .into_iter()
        .collect()
    }

    #[test]
    fn should_write_lossless_created_payload() {
        let payload = ProductListingDomainEventPayload::Created(Box::new(ProductListingCreated {
            title: Some(Localized::new(Language::En, Title::from("Bronze vase"))),
            description: Some(Localized::new(Language::En, Description::from("Ancient"))),
            address: ProductListingAddress {
                structured: None,
                geo: Some(geo::core::address::GeoAddress {
                    lat: 47.0,
                    lon: 8.0,
                }),
            },
            pricing: pricing(),
            sale_valuation: None,
            state: ProductState::Listed,
            url: url("https://shop.example/products/1"),
            images: images(),
            auction: ProductListingAuction {
                start: Some(OffsetDateTime::UNIX_EPOCH),
                end: None,
            },
        }));

        let json = event_payload_json(&payload);

        assert_eq!(Some("created"), json.get("kind").and_then(Value::as_str));
        assert_eq!(
            Some("Bronze vase"),
            json.pointer("/title/text").and_then(Value::as_str)
        );
        assert_eq!(
            Some(1_200),
            json.pointer("/pricing/price/amount")
                .and_then(Value::as_i64)
        );
        assert_eq!(
            Some("EUR"),
            json.pointer("/pricing/price/currency")
                .and_then(Value::as_str)
        );

        assert_eq!(
            Some("https://shop.example/image.jpg"),
            json.pointer("/images/0/url").and_then(Value::as_str)
        );
        assert_eq!(
            Some(47.0),
            json.pointer("/address/geo/lat").and_then(Value::as_f64)
        );
    }

    #[test]
    fn should_write_old_and_new_source_pricing_snapshots() {
        let payload = ProductListingDomainEventPayload::PriceChanged(ProductListingPriceChanged {
            old_pricing: pricing(),
            new_pricing: ProductListingPricing {
                price: Some(price(1_500, Currency::Usd)),
                price_estimate_min: None,
                price_estimate_max: None,
            },
        });

        let json = event_payload_json(&payload);

        assert_eq!(
            Some(1_200),
            json.pointer("/oldPricing/price/amount")
                .and_then(Value::as_i64)
        );
        assert_eq!(
            Some("USD"),
            json.pointer("/newPricing/price/currency")
                .and_then(Value::as_str)
        );
    }

    #[test]
    fn should_write_payload_for_every_product_event_type() {
        let event_types = [
            ProductListingDomainEventPayload::StateChanged(ProductStateChanged {
                old_state: ProductState::Listed,
                new_state: ProductState::Available,
                sale_valuation: None,
            }),
            ProductListingDomainEventPayload::AddressChanged(ProductListingAddressChanged {
                address: ProductListingAddress::default(),
            }),
            ProductListingDomainEventPayload::UrlChanged(ProductListingUrlChanged {
                old_url: url("https://shop.example/products/1"),
                new_url: url("https://shop.example/products/2"),
            }),
            ProductListingDomainEventPayload::ImagesChanged(Box::new(
                ProductListingImagesChanged { images: images() },
            )),
            ProductListingDomainEventPayload::AuctionChanged(ProductListingAuctionChanged {
                auction: ProductListingAuction::default(),
            }),
            ProductListingDomainEventPayload::Deleted(ProductListingDeleted {
                old_lifecycle: ProductLifecycle::Active,
                new_lifecycle: ProductLifecycle::Deleted,
            }),
        ];

        for event in event_types {
            let json = event_payload_json(&event);

            assert!(json.get("kind").and_then(Value::as_str).is_some());
        }
    }
}
