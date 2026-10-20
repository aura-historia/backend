use super::*;
use product_listing_core::listing_availability::ListingAvailability;
use product_listing_service::use_cases::ProductListingHistoryChanges;
use serde_json::json;

use money::{Currency, MonetaryAmount, Price};
use product_listing_core::{
    product_listing::ProductListingPricing, product_listing_price::ProductListingPrice,
};

#[test]
fn should_preserve_source_price_shapes_and_add_optional_historical_display() {
    let source = Price::new(MonetaryAmount::from(3_u64), Currency::Eur);
    let display = ProductListingHistoryPriceDisplay {
        price: Price::new(MonetaryAmount::from(5_u64), Currency::Usd),
        fx_rate_id: FxRateId::new(),
        captured_at: OffsetDateTime::UNIX_EPOCH,
    };
    let projected = ProductListingHistoryPrice {
        source,
        display: Some(display),
    };
    let value =
        serde_json::to_value(ProductListingHistoryPriceData::<PriceData>::from(projected))
            .unwrap();
    let expected_display = json!({"currency":"USD", "amount":5, "fxRateId": display.fx_rate_id, "capturedAt":"1970-01-01T00:00:00Z"});
    assert_eq!(
        json!({"currency":"EUR", "amount":3, "display":expected_display}),
        value
    );
    assert_eq!(
        json!({"currency":"EUR", "amount":3}),
        serde_json::to_value(ProductListingHistoryPriceData::<PriceData>::from(
            ProductListingHistoryPrice::from(source)
        ))
        .unwrap()
    );
    assert_eq!(
        json!({"type":"MONETARY", "currency":"EUR", "amount":3, "display":expected_display}),
        serde_json::to_value(
            ProductListingHistoryPriceData::<ProductListingPriceData>::from(
                ProductListingHistoryPrice {
                    source: ProductListingPrice::Monetary(source),
                    display: Some(display)
                }
            )
        )
        .unwrap()
    );
    assert_eq!(
        json!({"type":"ON_REQUEST"}),
        serde_json::to_value(
            ProductListingHistoryPriceData::<ProductListingPriceData>::from(
                ProductListingHistoryPrice::from(ProductListingPrice::OnRequest)
            )
        )
        .unwrap()
    );

    let pricing = ProductListingPricing {
        price: Some(ProductListingPrice::Monetary(source)),
        price_estimate_min: Some(source),
        price_estimate_max: Some(source),
    };
    let baseline = serde_json::to_value(ProductListingPricingData::from(
        ProductListingHistoryPricing::from(pricing),
    ))
    .unwrap();
    assert_eq!(
        json!({"price":{"type":"MONETARY", "currency":"EUR", "amount":3}, "priceEstimateMin":{"currency":"EUR", "amount":3}, "priceEstimateMax":{"currency":"EUR", "amount":3}}),
        baseline
    );
    let mut projected_pricing = ProductListingHistoryPricing::from(pricing);
    projected_pricing.price.as_mut().unwrap().display = Some(display);
    projected_pricing
        .price_estimate_min
        .as_mut()
        .unwrap()
        .display = Some(display);
    projected_pricing
        .price_estimate_max
        .as_mut()
        .unwrap()
        .display = Some(display);
    let projected =
        serde_json::to_value(ProductListingPricingData::from(projected_pricing)).unwrap();
    for key in ["price", "priceEstimateMin", "priceEstimateMax"] {
        assert_eq!(expected_display, projected[key]["display"]);
        assert_eq!(baseline[key]["amount"], projected[key]["amount"]);
        assert_eq!(baseline[key]["currency"], projected[key]["currency"]);
    }
    let changes = vec![
        ProductListingHistoryChange::MainPriceChanged {
            previous: Some(ProductListingPrice::OnRequest.into()),
            current: None,
        },
        ProductListingHistoryChange::MinimumEstimateChanged {
            previous: Some(ProductListingHistoryPrice {
                source,
                display: Some(display),
            }),
            current: None,
        },
        ProductListingHistoryChange::MaximumEstimateChanged {
            previous: None,
            current: Some(ProductListingHistoryPrice {
                source,
                display: Some(display),
            }),
        },
    ];
    let value = serde_json::to_value(
        changes
            .into_iter()
            .map(ProductListingHistoryChangeData::from)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert_eq!(
        json!({"type":"MAIN_PRICE_CHANGED", "previous":{"type":"ON_REQUEST"}, "current":null}),
        value[0]
    );
    assert_eq!(expected_display, value[1]["previous"]["display"]);
    assert_eq!(expected_display, value[2]["current"]["display"]);
    assert!(value[1]["current"].is_null());
    assert!(value[2]["previous"].is_null());
}

#[test]
fn should_serialize_one_changed_history_entry_with_ordered_changes() {
    let entry = ProductListingHistoryEntry {
        product_listing_id: ProductListingId::new(),
        event_id: EventId::new(),
        occurred_at: OffsetDateTime::now_utc(),
        kind: ProductListingHistoryEntryKind::Changed(
            ProductListingHistoryChanges::try_from(vec![
                ProductListingHistoryChange::AvailabilityChanged {
                    previous: Some(ListingAvailability::Available),
                    current: Some(ListingAvailability::SoldOut),
                },
                ProductListingHistoryChange::ImagesChanged {
                    previous_count: 2,
                    current_count: 2,
                },
                ProductListingHistoryChange::Restored,
            ])
            .unwrap_or_else(|error| panic!("non-empty history changes: {error}")),
        ),
    };

    let data = ProductListingHistoryEntryData::from(entry);
    let value = serde_json::to_value(data)
        .unwrap_or_else(|error| panic!("serialize ProductListing history entry: {error}"));

    assert_eq!(json!("PRODUCT_LISTING_CHANGED"), value["eventType"]);
    assert_eq!(
        json!([
            {
                "type": "AVAILABILITY_CHANGED",
                "previous": "AVAILABLE",
                "current": "SOLD_OUT"
            },
            {
                "type": "IMAGES_CHANGED",
                "previousCount": 2,
                "currentCount": 2
            },
            {
                "type": "RESTORED"
            }
        ]),
        value["payload"]["changes"]
    );
}
