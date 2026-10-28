use super::product_data::ProductListingAuctionData;
use crate::values::{LocalizedTextData, PriceData, ProductListingPriceData};
use domain_primitives::event_id::EventId;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing::ListingSaleObservation,
    product_listing_id::ProductListingId, source_listing_id::SourceListingId,
};
use product_listing_service::use_cases::{
    ProductListingDiscoveryHistory, ProductListingHistoryChange, ProductListingHistoryEntry,
    ProductListingHistoryEntryKind, ProductListingHistoryPrice, ProductListingHistoryPriceDisplay,
    ProductListingHistoryPricing,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use url::Url;

/// Existing optional REST history presentation query, reused by MCP arguments.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingHistoryQueryData {
    #[serde(default, with = "crate::wire::currency::option")]
    #[schemars(schema_with = "crate::wire::currency::option::schema")]
    pub currency: Option<money::Currency>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingHistoryEntryData {
    event_type: &'static str,
    #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[schemars(schema_with = "crate::wire::schema::event_id")]
    event_id: EventId,
    payload: ProductListingHistoryPayloadData,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    timestamp: OffsetDateTime,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
enum ProductListingHistoryPayloadData {
    Discovered(Box<ProductListingDiscoveryHistoryData>),
    Changed(ProductListingChangedHistoryData),
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingDiscoveryHistoryData {
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    listing_source_id: ListingSourceId,
    #[serde(serialize_with = "crate::wire::source_listing_id::serialize")]
    #[schemars(with = "String")]
    source_listing_id: SourceListingId,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<LocalizedTextData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<LocalizedTextData>,
    pricing: ProductListingPricingData,
    #[serde(with = "crate::wire::listing::availability_option")]
    #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
    availability: Option<ListingAvailability>,
    #[schemars(schema_with = "crate::wire::schema::url")]
    url: Url,
    #[schemars(range(max = u64::MAX))]
    image_count: u64,
    auction: Option<ProductListingAuctionData>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingChangedHistoryData {
    changes: Vec<ProductListingHistoryChangeData>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
enum ProductListingHistoryChangeData {
    TitleChanged {
        previous: Option<LocalizedTextData>,
        current: Option<LocalizedTextData>,
    },
    DescriptionChanged {
        previous: Option<LocalizedTextData>,
        current: Option<LocalizedTextData>,
    },
    DeliveryChanged,
    MainPriceChanged {
        previous: Option<ProductListingHistoryPriceData<ProductListingPriceData>>,
        current: Option<ProductListingHistoryPriceData<ProductListingPriceData>>,
    },
    MinimumEstimateChanged {
        previous: Option<ProductListingHistoryPriceData<PriceData>>,
        current: Option<ProductListingHistoryPriceData<PriceData>>,
    },
    MaximumEstimateChanged {
        previous: Option<ProductListingHistoryPriceData<PriceData>>,
        current: Option<ProductListingHistoryPriceData<PriceData>>,
    },
    AvailabilityChanged {
        #[serde(with = "crate::wire::listing::availability_option")]
        #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
        previous: Option<ListingAvailability>,
        #[serde(with = "crate::wire::listing::availability_option")]
        #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
        current: Option<ListingAvailability>,
    },
    UrlChanged {
        #[schemars(schema_with = "crate::wire::schema::url")]
        previous: Url,
        #[schemars(schema_with = "crate::wire::schema::url")]
        current: Url,
    },
    ImagesChanged {
        #[schemars(range(max = u64::MAX))]
        previous_count: u64,
        #[schemars(range(max = u64::MAX))]
        current_count: u64,
    },
    AuctionChanged {
        previous: Option<ProductListingAuctionData>,
        current: Option<ProductListingAuctionData>,
    },
    Withdrawn {
        #[serde(with = "crate::wire::listing::availability_option")]
        #[schemars(schema_with = "crate::wire::listing::availability_option_schema")]
        previous_availability: Option<ListingAvailability>,
    },
    Restored,
    SaleObserved {
        observation: ListingSaleObservationHistoryData,
    },
    SaleObservationRetracted {
        observation: ListingSaleObservationHistoryData,
    },
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingPricingData {
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<ProductListingHistoryPriceData<ProductListingPriceData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_min: Option<ProductListingHistoryPriceData<PriceData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price_estimate_max: Option<ProductListingHistoryPriceData<PriceData>>,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ProductListingHistoryPriceData<T> {
    #[serde(flatten)]
    source: T,
    #[serde(skip_serializing_if = "Option::is_none")]
    display: Option<ProductListingHistoryPriceDisplayData>,
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ProductListingHistoryPriceDisplayData {
    #[serde(flatten)]
    price: PriceData,
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    fx_rate_id: FxRateId,
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    captured_at: OffsetDateTime,
}

impl<T, D: From<T>> From<ProductListingHistoryPrice<T>> for ProductListingHistoryPriceData<D> {
    fn from(value: ProductListingHistoryPrice<T>) -> Self {
        Self {
            source: value.source.into(),
            display: value.display.map(Into::into),
        }
    }
}

impl From<ProductListingHistoryPriceDisplay> for ProductListingHistoryPriceDisplayData {
    fn from(display: ProductListingHistoryPriceDisplay) -> Self {
        Self {
            price: display.price.into(),
            fx_rate_id: display.fx_rate_id,
            captured_at: display.captured_at,
        }
    }
}

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ListingSaleObservationHistoryData {
    #[serde(with = "time::serde::rfc3339")]
    #[schemars(schema_with = "crate::wire::schema::datetime")]
    observed_at: OffsetDateTime,
    #[schemars(schema_with = "crate::wire::schema::fx_rate_id")]
    fx_rate_id: FxRateId,
}

impl From<ProductListingHistoryEntry> for ProductListingHistoryEntryData {
    fn from(entry: ProductListingHistoryEntry) -> Self {
        Self {
            event_type: entry.event_type().as_str(),
            product_listing_id: entry.product_listing_id,
            event_id: entry.event_id,
            payload: entry.kind.into(),
            timestamp: entry.occurred_at,
        }
    }
}

impl From<ProductListingHistoryEntryKind> for ProductListingHistoryPayloadData {
    fn from(value: ProductListingHistoryEntryKind) -> Self {
        match value {
            ProductListingHistoryEntryKind::Discovered(discovery) => {
                Self::Discovered(Box::new((*discovery).into()))
            }
            ProductListingHistoryEntryKind::Changed(changes) => {
                Self::Changed(ProductListingChangedHistoryData {
                    changes: changes.into_inner().into_iter().map(Into::into).collect(),
                })
            }
        }
    }
}

impl From<ProductListingDiscoveryHistory> for ProductListingDiscoveryHistoryData {
    fn from(value: ProductListingDiscoveryHistory) -> Self {
        Self {
            listing_source_id: value.listing_source_id,
            source_listing_id: value.source_listing_id,
            title: value.title.map(Into::into),
            description: value.description.map(Into::into),
            pricing: value.pricing.into(),
            availability: value.availability,
            url: value.url,
            image_count: value.image_count,
            auction: value.auction.map(Into::into),
        }
    }
}

impl From<ProductListingHistoryChange> for ProductListingHistoryChangeData {
    fn from(value: ProductListingHistoryChange) -> Self {
        match value {
            ProductListingHistoryChange::TitleChanged { previous, current } => Self::TitleChanged {
                previous: previous.map(Into::into),
                current: current.map(Into::into),
            },
            ProductListingHistoryChange::DescriptionChanged { previous, current } => {
                Self::DescriptionChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::DeliveryChanged => Self::DeliveryChanged,
            ProductListingHistoryChange::MainPriceChanged { previous, current } => {
                Self::MainPriceChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::MinimumEstimateChanged { previous, current } => {
                Self::MinimumEstimateChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::MaximumEstimateChanged { previous, current } => {
                Self::MaximumEstimateChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::AvailabilityChanged { previous, current } => {
                Self::AvailabilityChanged { previous, current }
            }
            ProductListingHistoryChange::UrlChanged { previous, current } => {
                Self::UrlChanged { previous, current }
            }
            ProductListingHistoryChange::ImagesChanged {
                previous_count,
                current_count,
            } => Self::ImagesChanged {
                previous_count,
                current_count,
            },
            ProductListingHistoryChange::AuctionChanged { previous, current } => {
                Self::AuctionChanged {
                    previous: previous.map(Into::into),
                    current: current.map(Into::into),
                }
            }
            ProductListingHistoryChange::Withdrawn {
                previous_availability,
            } => Self::Withdrawn {
                previous_availability,
            },
            ProductListingHistoryChange::Restored => Self::Restored,
            ProductListingHistoryChange::SaleObserved { observation } => Self::SaleObserved {
                observation: observation.into(),
            },
            ProductListingHistoryChange::SaleObservationRetracted { observation } => {
                Self::SaleObservationRetracted {
                    observation: observation.into(),
                }
            }
        }
    }
}

impl From<ProductListingHistoryPricing> for ProductListingPricingData {
    fn from(pricing: ProductListingHistoryPricing) -> Self {
        Self {
            price: pricing.price.map(Into::into),
            price_estimate_min: pricing.price_estimate_min.map(Into::into),
            price_estimate_max: pricing.price_estimate_max.map(Into::into),
        }
    }
}

impl From<ListingSaleObservation> for ListingSaleObservationHistoryData {
    fn from(observation: ListingSaleObservation) -> Self {
        Self {
            observed_at: observation.observed_at(),
            fx_rate_id: observation.fx_rate_id(),
        }
    }
}

#[cfg(test)]
mod tests {
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
            checked_output(ProductListingHistoryPriceData::<PriceData>::from(projected)).unwrap();
        let expected_display = json!({"currency":"USD", "amount":5, "fxRateId": display.fx_rate_id, "capturedAt":"1970-01-01T00:00:00Z"});
        assert_eq!(
            json!({"currency":"EUR", "amount":3, "display":expected_display}),
            value
        );
        assert_eq!(
            json!({"currency":"EUR", "amount":3}),
            checked_output(ProductListingHistoryPriceData::<PriceData>::from(
                ProductListingHistoryPrice::from(source)
            ))
            .unwrap()
        );
        assert_eq!(
            json!({"type":"MONETARY", "currency":"EUR", "amount":3, "display":expected_display}),
            checked_output(
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
            checked_output(
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
        let baseline = checked_output(ProductListingPricingData::from(
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
        let projected = checked_output(ProductListingPricingData::from(projected_pricing)).unwrap();
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
        let value = checked_output(
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
        let value = checked_output(data)
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

    fn checked_output<T: serde::Serialize + JsonSchema>(
        value: T,
    ) -> Result<serde_json::Value, serde_json::Error> {
        let schema = serde_json::to_value(
            schemars::generate::SchemaSettings::default()
                .for_serialize()
                .into_generator()
                .into_root_schema_for::<T>(),
        )?;
        let value = serde_json::to_value(value)?;
        let validator = jsonschema::validator_for(&schema).unwrap();
        assert!(
            validator.is_valid(&value),
            "{}",
            validator
                .iter_errors(&value)
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        Ok(value)
    }

    #[test]
    fn should_preserve_discovery_and_every_change_in_the_shared_output_schema() {
        use auction_core::AuctionId;
        use localization::{Language, Localized};
        use product_listing_core::{
            description::Description, product_listing::ProductListingAuction, title::Title,
        };

        let at = OffsetDateTime::UNIX_EPOCH;
        let fx_rate_id = FxRateId::new();
        let observation = ListingSaleObservation::new(at, fx_rate_id);
        let auction = ProductListingAuction::new(
            Some(AuctionId::new()),
            None,
            None,
            Some(at),
            Some(at),
            Some(at),
        )
        .unwrap()
        .unwrap();
        let discovery = ProductListingDiscoveryHistory {
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("source-42").unwrap(),
            title: Some(Localized::new(Language::De, Title::from("Schrank"))),
            description: Some(Localized::new(
                Language::En,
                Description::from("Oak cabinet"),
            )),
            pricing: ProductListingHistoryPricing::default(),
            availability: Some(ListingAvailability::Available),
            url: Url::parse("https://example.test/listing").unwrap(),
            image_count: u64::MAX,
            auction: Some(auction.clone()),
        };
        let product_listing_id = ProductListingId::new();
        let event_id = EventId::new();
        let discovered = checked_output(ProductListingHistoryEntryData::from(
            ProductListingHistoryEntry {
                product_listing_id,
                event_id,
                occurred_at: at,
                kind: ProductListingHistoryEntryKind::Discovered(Box::new(discovery.clone())),
            },
        ))
        .unwrap();
        assert_eq!(
            json!({
                "eventType":"PRODUCT_LISTING_DISCOVERED", "productListingId":product_listing_id,
                "eventId":event_id, "timestamp":"1970-01-01T00:00:00Z",
                "payload":{
                    "listingSourceId":discovery.listing_source_id, "sourceListingId":"source-42",
                    "title":{"text":"Schrank","language":"de"},
                    "description":{"text":"Oak cabinet","language":"en"}, "pricing":{},
                    "availability":"AVAILABLE", "url":"https://example.test/listing", "imageCount":u64::MAX,
                    "auction":{
                        "auctionId":auction.auction_id(), "lotNumber":null,"cataloguePosition":null,
                        "biddingOpens":"1970-01-01T00:00:00Z", "scheduledCloses":"1970-01-01T00:00:00Z",
                        "reportedClosedAt":"1970-01-01T00:00:00Z"
                    }
                }
            }),
            discovered
        );
        let changes = vec![
            ProductListingHistoryChange::DeliveryChanged,
            ProductListingHistoryChange::MainPriceChanged {
                previous: None,
                current: Some(ProductListingPrice::OnRequest.into()),
            },
            ProductListingHistoryChange::MinimumEstimateChanged {
                previous: None,
                current: None,
            },
            ProductListingHistoryChange::MaximumEstimateChanged {
                previous: None,
                current: None,
            },
            ProductListingHistoryChange::AvailabilityChanged {
                previous: Some(ListingAvailability::Available),
                current: None,
            },
            ProductListingHistoryChange::UrlChanged {
                previous: discovery.url.clone(),
                current: Url::parse("https://example.test/new").unwrap(),
            },
            ProductListingHistoryChange::ImagesChanged {
                previous_count: 0,
                current_count: u64::MAX,
            },
            ProductListingHistoryChange::AuctionChanged {
                previous: None,
                current: Some(auction),
            },
            ProductListingHistoryChange::Withdrawn {
                previous_availability: Some(ListingAvailability::Available),
            },
            ProductListingHistoryChange::Restored,
            ProductListingHistoryChange::SaleObserved { observation },
            ProductListingHistoryChange::SaleObservationRetracted { observation },
        ];
        let changed = checked_output(ProductListingHistoryEntryData::from(
            ProductListingHistoryEntry {
                product_listing_id,
                event_id: EventId::new(),
                occurred_at: at,
                kind: ProductListingHistoryEntryKind::Changed(changes.try_into().unwrap()),
            },
        ))
        .unwrap();
        let changes = changed["payload"]["changes"].as_array().unwrap();
        assert_eq!(
            vec![
                "DELIVERY_CHANGED",
                "MAIN_PRICE_CHANGED",
                "MINIMUM_ESTIMATE_CHANGED",
                "MAXIMUM_ESTIMATE_CHANGED",
                "AVAILABILITY_CHANGED",
                "URL_CHANGED",
                "IMAGES_CHANGED",
                "AUCTION_CHANGED",
                "WITHDRAWN",
                "RESTORED",
                "SALE_OBSERVED",
                "SALE_OBSERVATION_RETRACTED"
            ],
            changes
                .iter()
                .map(|change| change["type"].as_str().unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(discovered["payload"]["auction"], changes[7]["current"]);
        for index in [10, 11] {
            assert_eq!(
                json!({"observedAt":"1970-01-01T00:00:00Z","fxRateId":fx_rate_id}),
                changes[index]["observation"]
            );
        }
    }

    #[test]
    fn should_describe_the_existing_optional_display_currency_query() {
        let schema = serde_json::to_value(
            schemars::generate::SchemaSettings::default()
                .for_deserialize()
                .into_generator()
                .into_root_schema_for::<ProductListingHistoryQueryData>(),
        )
        .unwrap();
        for (input, expected) in [
            (json!({}), Some(None)),
            (json!({"currency":null}), Some(None)),
            (json!({"currency":"USD"}), Some(Some(Currency::Usd))),
            (json!({"currency":"XXX"}), None),
            (json!({"currency":"usd"}), None),
            (json!({"currency":3}), None),
        ] {
            let result = serde_json::from_value::<ProductListingHistoryQueryData>(input.clone())
                .map(|query| query.currency)
                .ok();
            assert_eq!(expected, result, "Serde {input}");
            assert_eq!(
                expected.is_some(),
                jsonschema::is_valid(&schema, &input),
                "schema {input}"
            );
        }
    }
    #[test]
    fn should_expose_original_localized_text_and_explicit_absence_in_history() {
        use localization::{Language, Localized};
        use product_listing_core::{description::Description, title::Title};
        let title = Localized::new(Language::Fr, Title::from("Nouveau titre"));
        let description = Localized::new(
            Language::De,
            Description::from("Erster Absatz\n\nZweiter Absatz"),
        );
        for (change, expected) in [
            (
                ProductListingHistoryChange::TitleChanged {
                    previous: None,
                    current: Some(title),
                },
                serde_json::json!({"type":"TITLE_CHANGED","previous":null,"current":{"text":"Nouveau titre","language":"fr"}}),
            ),
            (
                ProductListingHistoryChange::DescriptionChanged {
                    previous: Some(description),
                    current: None,
                },
                serde_json::json!({"type":"DESCRIPTION_CHANGED","previous":{"text":"Erster Absatz\n\nZweiter Absatz","language":"de"},"current":null}),
            ),
        ] {
            let entry = ProductListingHistoryEntry {
                product_listing_id: ProductListingId::new(),
                event_id: EventId::new(),
                occurred_at: OffsetDateTime::UNIX_EPOCH,
                kind: ProductListingHistoryEntryKind::Changed(vec![change].try_into().unwrap()),
            };
            let output = checked_output(ProductListingHistoryEntryData::from(entry)).unwrap();
            assert_eq!(output["payload"]["changes"][0], expected);
        }
    }
}
