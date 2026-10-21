use application::pagination::Cursor;
use domain_primitives::event_id::EventId;
use domain_primitives::query::range_query::RangeQuery;
use fxrate_core::FxRateId;
use listing_source_core::ListingSourceId;
use money::Currency;
use money::MonetaryAmount;
use opensearch::IndexParts;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_core::product_listing_price::ProductListingPrice;
use product_listing_core::product_listing_search::ProductListingSearch;
use product_listing_core::product_listing_slug_id::ProductListingSlugId;
use product_listing_core::source_listing_id::SourceListingId;
use product_listing_service::ports::{
    CompiledProductListingSearch, ProductListingPriceFilterPlan, ProductListingSearchReadRequest,
    ProductListingSearchReader,
};
use product_listing_service::use_cases::queries::search_product_listings::ProductListingSearchReadResult;
use serde_json::{Value, json};

use std::io::{Error as IoError, ErrorKind};
use strum::IntoEnumIterator;
use test_api::{
    IntegrationTestService, OpenSearch, aura_integration_test, get_opensearch_client, refresh_index,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

#[path = "support/search_fixture.rs"]
mod search_fixture;
use search_fixture::*;

fn products_index() -> String {
    test_api::TestContext::current()
        .expect("active OpenSearch test")
        .product_listings_index()
        .expect("scoped product listings index")
}

fn geographic_query(
    origin: Option<(f64, f64)>,
    radius: Option<f64>,
    approximate: bool,
    ordered: bool,
) -> geo::core::discovery::GeographicQuery {
    use geo::core::discovery::*;
    GeographicQuery::new(
        geo::reference::ReferenceRelease::CURRENT,
        std::collections::HashSet::from([geo::CountryCode::DEU]),
        std::collections::HashSet::from([geo::SubdivisionCode::new("DE-BY").unwrap()]),
        origin.map(|(lat, lon)| geo::GeoPoint::new(lat, lon).unwrap()),
        radius.map(|amount| geo::Distance::new(amount, geo::DistanceUnit::Kilometers).unwrap()),
        if approximate {
            ProximityMode::Approximate
        } else {
            ProximityMode::Strict
        },
        ordered.then_some(DistanceOrder::Asc),
    )
    .unwrap()
}
fn geographic_document(lat: f64, lon: f64, precision: &str) -> Value {
    json!({"locationId": party_core::party_location_id::PartyLocationId::new(), "basis":"SOURCE_DEFAULT_DERIVED", "referenceRelease":geo::reference::ReferenceRelease::CURRENT.as_str(), "country":"DE", "subdivision":"DE-BY", "point":{"lat":lat,"lon":lon},"precision":precision})
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_search_only_inventory_eligible_geography_with_precision_aware_proximity() {
    async fn run() -> TestResult {
        let mut business_only =
            product_listing_document(ProductListingSeed::active("business-only", 100))?.document;
        business_only["businessGeography"] = geographic_document(48.137, 11.575, "PREMISES");
        let business_id = business_only["productListingId"]
            .as_str()
            .unwrap()
            .parse::<ProductListingId>()?;
        let mut inventory =
            product_listing_document(ProductListingSeed::active("inventory", 100))?.document;
        inventory["businessGeography"] = geographic_document(48.237, 11.575, "PREMISES");
        inventory["inventoryGeography"] = inventory["businessGeography"].clone();
        let inventory_id = inventory["productListingId"]
            .as_str()
            .unwrap()
            .parse::<ProductListingId>()?;
        let mut coarse =
            product_listing_document(ProductListingSeed::active("coarse", 100))?.document;
        coarse["inventoryGeography"] = geographic_document(48.138, 11.575, "LOCALITY");
        let mut far = product_listing_document(ProductListingSeed::active("far", 100))?.document;
        far["inventoryGeography"] = geographic_document(49.137, 11.575, "PREMISES");
        let missing =
            product_listing_document(ProductListingSeed::active("missing", 100))?.document;
        let mut country_only =
            product_listing_document(ProductListingSeed::active("advertised-country", 100))?
                .document;
        country_only["inventoryGeography"] = geographic_document(48.137, 11.575, "PREMISES");
        country_only["inventoryGeography"]["point"] = Value::Null;
        country_only["inventoryGeography"]["precision"] = Value::Null;
        let country_id = country_only["productListingId"]
            .as_str()
            .unwrap()
            .parse::<ProductListingId>()?;
        index_products([business_only, inventory, coarse, far, missing, country_only]).await?;
        let query = ProductListingSearch {
            geography: Some(geographic_query(None, None, false, false)),
            ..Default::default()
        };
        let result = search(query, price_filter(None)?).await?;
        assert_eq!(Some(4), result.total);
        assert!(
            result
                .items
                .iter()
                .any(|i| i.product_listing_id == inventory_id)
        );
        assert!(
            result
                .items
                .iter()
                .find(|i| i.product_listing_id == country_id)
                .unwrap()
                .geography
                .as_ref()
                .unwrap()
                .geography
                .position
                .is_none()
        );
        let mut query = ProductListingSearch {
            geography: Some(geographic_query(
                Some((48.137, 11.575)),
                Some(25.0),
                false,
                true,
            )),
            ..Default::default()
        };
        let result = search(query.clone(), price_filter(None)?).await?;
        assert_eq!(Some(1), result.total);
        assert_eq!(inventory_id, result.items[0].product_listing_id);
        assert!(
            result
                .items
                .iter()
                .all(|p| p.product_listing_id != business_id)
        );
        assert_eq!(
            geo::SpatialPrecision::Premises,
            result.items[0]
                .geography
                .as_ref()
                .unwrap()
                .geography
                .position
                .unwrap()
                .precision()
        );
        // Filtering and ordering use the same inventory-eligible point.
        assert!(
            result.cursor.search_after.as_ref().unwrap()[0]
                .as_f64()
                .unwrap()
                < 25.0
        );
        query.geography = Some(geographic_query(
            Some((48.137, 11.575)),
            Some(25.0),
            true,
            true,
        ));
        assert_eq!(
            Some(2),
            search(query.clone(), price_filter(None)?).await?.total
        );
        query.geography = Some(geographic_query(
            Some((11.575, 48.137)),
            Some(25.0),
            true,
            true,
        ));
        assert_eq!(Some(0), search(query, price_filter(None)?).await?.total);
        Ok(())
    }
    assert_ok(run().await);
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_handle_antimeridian_zero_distance_and_maximum_radius_without_missing_point_sentinels()
 {
    async fn run() -> TestResult {
        let mut across =
            product_listing_document(ProductListingSeed::active("across", 100))?.document;
        across["inventoryGeography"] = geographic_document(0.0, -179.9, "PREMISES");
        let mut zero = product_listing_document(ProductListingSeed::active("zero", 100))?.document;
        zero["inventoryGeography"] = geographic_document(0.0, 0.0, "PREMISES");
        let missing =
            product_listing_document(ProductListingSeed::active("missing", 100))?.document;
        index_products([across, zero, missing]).await?;
        let mut query = ProductListingSearch {
            geography: Some(geographic_query(
                Some((0.0, 179.9)),
                Some(25.0),
                false,
                true,
            )),
            ..Default::default()
        };
        assert_eq!(
            Some(1),
            search(query.clone(), price_filter(None)?).await?.total
        );
        query.geography = Some(geographic_query(Some((0.0, 0.0)), Some(1.0), false, true));
        assert_eq!(
            Some(1),
            search(query.clone(), price_filter(None)?).await?.total
        );
        query.geography = Some(geographic_query(
            Some((0.0, 0.0)),
            Some(20_000.0),
            false,
            true,
        ));
        // The antipodal point lies beyond the documented maximum, not worldwide coverage.
        assert_eq!(Some(1), search(query, price_filter(None)?).await?.total);
        Ok(())
    }
    assert_ok(run().await);
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_preserve_inclusive_radius_boundaries_and_polar_coordinates() {
    async fn run() -> TestResult {
        let mut boundary =
            product_listing_document(ProductListingSeed::active("boundary", 100))?.document;
        boundary["inventoryGeography"] = geographic_document(0.1, 0.0, "PREMISES");
        let mut pole = product_listing_document(ProductListingSeed::active("pole", 100))?.document;
        pole["inventoryGeography"] = geographic_document(90.0, 180.0, "PREMISES");
        index_products([boundary, pole]).await?;
        let mut q = ProductListingSearch {
            geography: Some(geographic_query(Some((0.0, 0.0)), Some(25.0), false, true)),
            ..Default::default()
        };
        let result = search(q.clone(), price_filter(None)?).await?;
        assert_eq!(Some(1), result.total);
        let distance = result.cursor.search_after.as_ref().unwrap()[0]
            .as_f64()
            .unwrap();
        q.geography = Some(geographic_query(
            Some((0.0, 0.0)),
            Some(distance),
            false,
            true,
        ));
        assert_eq!(Some(1), search(q.clone(), price_filter(None)?).await?.total);
        q.geography = Some(geographic_query(
            Some((0.0, 0.0)),
            Some(distance - 0.001),
            false,
            true,
        ));
        assert_eq!(Some(0), search(q.clone(), price_filter(None)?).await?.total);
        q.geography = Some(geographic_query(
            Some((90.0, -180.0)),
            Some(1.0),
            false,
            true,
        ));
        assert_eq!(Some(1), search(q, price_filter(None)?).await?.total);
        Ok(())
    }
    assert_ok(run().await);
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_apply_native_radius_units_and_match_equivalent_physical_distances() {
    async fn run() -> TestResult {
        use geo::core::discovery::{DistanceOrder, GeographicQuery};
        use geo::{Distance, DistanceUnit};
        use strum::IntoEnumIterator;
        let mut near = product_listing_document(ProductListingSeed::active("near", 100))?.document;
        near["inventoryGeography"] = geographic_document(0.1, 0.0, "PREMISES");
        let near_id = near["productListingId"]
            .as_str()
            .unwrap()
            .parse::<ProductListingId>()?;
        let mut farther =
            product_listing_document(ProductListingSeed::active("farther", 100))?.document;
        farther["inventoryGeography"] = geographic_document(0.3, 0.0, "PREMISES");
        index_products([near, farther]).await?;
        for unit in DistanceUnit::iter() {
            let factor = Distance::new(1.0, unit)?.to_kilometers();
            let query = ProductListingSearch {
                geography: Some(GeographicQuery::new(
                    geo::reference::ReferenceRelease::CURRENT,
                    Default::default(),
                    Default::default(),
                    Some(geo::GeoPoint::new(0.0, 0.0)?),
                    Some(Distance::new(25.0 / factor, unit)?),
                    Default::default(),
                    Some(DistanceOrder::Asc),
                )?),
                ..Default::default()
            };
            let result = search(query, price_filter(None)?).await?;
            assert_eq!(Some(1), result.total, "radius unit {unit:?}");
            assert_eq!(near_id, result.items[0].product_listing_id);
        }
        let query = ProductListingSearch {
            geography: Some(GeographicQuery::new(
                geo::reference::ReferenceRelease::CURRENT,
                Default::default(),
                Default::default(),
                Some(geo::GeoPoint::new(0.0, 0.0)?),
                Some(Distance::new(25.0, DistanceUnit::Miles)?),
                Default::default(),
                Some(DistanceOrder::Asc),
            )?),
            ..Default::default()
        };
        assert_eq!(Some(2), search(query, price_filter(None)?).await?.total);
        Ok(())
    }
    assert_ok(run().await);
}

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_resume_server_issued_score_and_date_cursors_unchanged() {
    async fn run() -> TestResult {
        use domain_primitives::sort::{Sort, SortOrder};
        use product_listing_core::sort_product_listing_field::SortProductListingField;
        use product_listing_service::ports::ProductListingSearchMode;

        let documents = ["cursor-first", "cursor-second", "cursor-third"]
            .into_iter()
            .map(|slug| product_listing_document(ProductListingSeed::active(slug, 100)))
            .collect::<Result<Vec<_>, _>>()?;
        index_products(documents.into_iter().map(|listing| listing.document)).await?;
        let reader = product_listing_opensearch::OpenSearchProductListingSearchReader::with_index(
            get_opensearch_client().await.clone(),
            &products_index(),
        );
        for sort in [
            SortProductListingField::Score,
            SortProductListingField::Created,
            SortProductListingField::Updated,
        ] {
            for order in [SortOrder::Asc, SortOrder::Desc] {
                let mut request = ProductListingSearchReadRequest {
                    compiled_search: CompiledProductListingSearch::new(
                        ProductListingSearch::default(),
                        price_filter(None)?,
                    )?,
                    sort: Some(Sort { sort, order }),
                    cursor: Some(Cursor {
                        size: 1,
                        search_after: None,
                    }),
                };
                let mut found = std::collections::HashSet::new();
                for _ in 0..3 {
                    let page = reader.search(&request).await?;
                    assert_eq!(Some(3), page.total);
                    assert_eq!(1, page.items.len());
                    assert!(found.insert(page.items[0].product_listing_id));
                    let cursor = page.cursor.search_after.expect("nonempty page cursor");
                    assert_eq!(
                        ProductListingSearchMode::Lexical,
                        reader.continuation_mode(&cursor, sort)?
                    );
                    request.cursor.as_mut().unwrap().search_after = Some(cursor);
                }
                assert!(reader.search(&request).await?.items.is_empty());
            }
        }
        Ok(())
    }
    assert_ok(run().await);
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_search_active_and_sold_products_with_one_pinned_price_plan() {
    assert_ok(should_search_active_and_sold_products_with_one_pinned_price_plan_impl().await);
}

async fn should_search_active_and_sold_products_with_one_pinned_price_plan_impl() -> TestResult {
    let active = product_listing_document(ProductListingSeed::active("active", 100))?;
    let sold = product_listing_document(ProductListingSeed::sold("sold", 110))?;
    index_products([active.document, sold.document]).await?;

    let result = search(
        ProductListingSearch::new(localization::Language::En, Currency::Usd).with_price_query(
            RangeQuery {
                min: Some(MonetaryAmount::from(110_u64)),
                max: Some(MonetaryAmount::from(110_u64)),
            },
        ),
        price_filter(Some(RangeQuery {
            min: Some(MonetaryAmount::from(110_u64)),
            max: Some(MonetaryAmount::from(110_u64)),
        }))?,
    )
    .await?;

    assert_eq!(Some(2), result.total);
    assert_eq!(2, result.items.len());
    let tie_break = result
        .cursor
        .search_after
        .as_ref()
        .and_then(Value::as_array)
        .and_then(|sort| sort.last())
        .and_then(Value::as_str)
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "typed ID sort value missing"))?;
    let parsed_tie_break = tie_break.parse::<ProductListingId>()?;
    assert_eq!(tie_break, parsed_tie_break.to_string());
    assert!(result.items.iter().all(|item| {
        item.display_price
            == Some(ProductListingPrice::Monetary(money::Price::new(
                MonetaryAmount::from(110_u64),
                Currency::Usd,
            )))
    }));
    Ok(())
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search()
 {
    assert_ok(
        should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search_impl()
            .await,
    );
}

async fn should_return_sold_product_without_main_price_for_non_price_search_and_exclude_it_from_price_search_impl()
-> TestResult {
    let sold_without_price =
        product_listing_document(ProductListingSeed::sold_without_main_price("sold-no-price"))?;
    let product_listing_id = sold_without_price.document["productListingId"]
        .as_str()
        .ok_or_else(|| IoError::new(ErrorKind::InvalidData, "productListingId missing"))?
        .to_owned();
    index_products([sold_without_price.document]).await?;

    let non_price_result = search(
        ProductListingSearch::new(localization::Language::En, Currency::Usd),
        price_filter(None)?,
    )
    .await?;
    assert_eq!(Some(1), non_price_result.total);
    assert_eq!(1, non_price_result.items.len());
    assert_eq!(
        product_listing_id,
        non_price_result.items[0].product_listing_id.to_string()
    );
    assert_eq!(None, non_price_result.items[0].display_price);
    assert!(matches!(
        non_price_result.items[0].price_valuation,
        product_listing_service::use_cases::ProductListingSummaryPriceValuation::SaleObservation { .. }
    ));

    let price_result = search(
        ProductListingSearch::new(localization::Language::En, Currency::Usd).with_price_query(
            RangeQuery {
                min: Some(MonetaryAmount::from(1_u64)),
                max: Some(MonetaryAmount::from(1_000_u64)),
            },
        ),
        price_filter(Some(RangeQuery {
            min: Some(MonetaryAmount::from(1_u64)),
            max: Some(MonetaryAmount::from(1_000_u64)),
        }))?,
    )
    .await?;
    assert_eq!(Some(0), price_result.total);
    assert!(price_result.items.is_empty());
    Ok(())
}

#[aura_integration_test(services = [OpenSearch()], isolated = true)]
async fn shipping_search_agrees_with_pure_membership_and_preserves_nested_rows_and_pages() {
    async fn run() -> TestResult {
        use geo::{CountryCode, reference::ReferenceRelease};
        use money::Price;
        use product_listing_core::shipping_search::ShippingSearch;
        use product_listing_service::shipping_projection::{
            CountryShippingOffer, EffectiveListingShipping,
        };
        use shipping_core::terms::{ShippingEligibility, ShippingServiceCode};
        let offer = |country, currency, amount: Option<u64>| CountryShippingOffer {
            country,
            service: ShippingServiceCode::new("gallery-delivery".into()).unwrap(),
            per_item_fixed: amount.map(|amount: u64| Price::new(amount.into(), currency)),
        };
        let summary = |collection, offers| EffectiveListingShipping {
            collection,
            prohibited: false,
            conditional: true,
            country_offers: offers,
        };
        let mut alternative = offer(CountryCode::DEU, Currency::Eur, Some(100));
        alternative.service = ShippingServiceCode::new("other".into()).unwrap();
        let samples = vec![
            summary(
                Some(ShippingEligibility::Offered),
                vec![offer(CountryCode::DEU, Currency::Eur, Some(8000))],
            ),
            EffectiveListingShipping {
                collection: Some(ShippingEligibility::Offered),
                prohibited: true,
                conditional: false,
                country_offers: vec![],
            },
            summary(
                None,
                vec![
                    offer(CountryCode::DEU, Currency::Eur, Some(9500)),
                    offer(CountryCode::FRA, Currency::Eur, Some(100)),
                ],
            ),
            summary(
                Some(ShippingEligibility::SubjectToConfirmation),
                vec![offer(CountryCode::DEU, Currency::Eur, Some(0))],
            ),
            summary(
                Some(ShippingEligibility::NotOffered),
                vec![offer(CountryCode::DEU, Currency::Usd, Some(8000))],
            ),
            summary(None, vec![offer(CountryCode::DEU, Currency::Eur, None)]),
            EffectiveListingShipping::default(),
            summary(None, vec![offer(CountryCode::DEU, Currency::Jpy, Some(80))]),
            summary(
                None,
                vec![
                    offer(CountryCode::DEU, Currency::Eur, Some(9500)),
                    alternative,
                ],
            ),
        ];
        let mut documents = Vec::new();
        let mut ids = Vec::new();
        for (i, s) in samples.iter().enumerate() {
            let mut document = product_listing_document(ProductListingSeed::active(
                &format!("shipping-{i}"),
                100,
            ))?
            .document;
            ids.push(
                document["productListingId"]
                    .as_str()
                    .unwrap()
                    .parse::<ProductListingId>()?,
            );
            document["shipping"] = json!({"collection":s.collection.map(|c| c.as_str()),"prohibited":s.prohibited,"conditional":s.conditional,
                "countryOffers":s.country_offers.iter().map(|o| {
                    let mut value = json!({"country":o.country.alpha2(),"service":o.service.as_str()});
                    if let Some(p) = o.per_item_fixed { value["perItemFixed"] = json!({"currency":p.currency.as_str(),"amountMinor":u64::from(p.monetary_amount)}); }
                    value
                }).collect::<Vec<_>>()});
            document["inventoryGeography"] = geographic_document(48.137, 11.575, "PREMISES");
            documents.push(document);
        }
        index_products(documents.clone()).await?;
        for country in [None, Some(CountryCode::DEU), Some(CountryCode::FRA)] {
            for service in [None, Some("gallery-delivery"), Some("other")] {
                for maximum in [
                    None,
                    Some((0_u64, Currency::Eur)),
                    Some((9000, Currency::Eur)),
                    Some((9500, Currency::Eur)),
                    Some((9000, Currency::Usd)),
                    Some((90, Currency::Jpy)),
                ] {
                    for collection in [false, true] {
                        let Ok(q) = ShippingSearch::new(
                            ReferenceRelease::CURRENT,
                            country,
                            service.map(|s| ShippingServiceCode::new(s.into()).unwrap()),
                            maximum.map(|(a, c)| Price::new(a.into(), c)),
                            collection,
                        ) else {
                            continue;
                        };
                        for fx_mode in [false, true] {
                            if fx_mode && maximum.is_none() {
                                continue;
                            }
                            let q = if fx_mode {
                                q.clone().with_fx_conversion()?
                            } else {
                                q.clone()
                            };
                            let plan = price_filter(None)?;
                            let query = ProductListingSearch {
                                shipping: Some(q.clone()),
                                ..Default::default()
                            };
                            let result = search(query, plan.clone()).await?;
                            let expected = ids
                            .iter()
                            .zip(&samples)
                            .filter(|(_, s)| {
                                (!q.collection_offered()
                                    || s.collection == Some(ShippingEligibility::Offered))
                                    && q.country().is_none_or(|country| {
                                        s.country_offers.iter().any(|offer| {
                                            offer.country == country
                                                && q.service()
                                                    .is_none_or(|service| service == &offer.service)
                                                && q.maximum_per_item().is_none_or(|maximum| {
                                                    offer.per_item_fixed.is_some_and(|price| {
                                                        if fx_mode {
                                                            plan.snapshot().convert(price, maximum.currency, fxrate_core::RoundingMode::HalfUp).unwrap().monetary_amount <= maximum.monetary_amount
                                                        } else {
                                                            price.currency == maximum.currency && price.monetary_amount <= maximum.monetary_amount
                                                        }
                                                    })
                                                })
                                        })
                                    })
                            })
                            .map(|(id, _)| *id)
                            .collect::<std::collections::HashSet<_>>();
                            assert_eq!(Some(expected.len() as u64), result.total);
                            assert_eq!(
                                expected,
                                result.items.iter().map(|i| i.product_listing_id).collect()
                            );
                        }
                    }
                }
            }
        }
        let france_from_german_inventory = ProductListingSearch {
            geography: Some(geographic_query(None, None, false, false)),
            shipping: Some(ShippingSearch::new(
                ReferenceRelease::CURRENT,
                Some(CountryCode::FRA),
                None,
                Some(Price::new(100_u64.into(), Currency::Eur)),
                false,
            )?),
            currency: Currency::Usd,
            ..Default::default()
        };
        let independent = search(france_from_german_inventory, price_filter(None)?).await?;
        assert_eq!(Some(1), independent.total);
        assert_eq!(ids[2], independent.items[0].product_listing_id);
        // Gallery acceptance and DE-expensive / FR-cheap counterexample, with item location independent.
        let mut query = ProductListingSearch {
            shipping: Some(ShippingSearch::new(
                ReferenceRelease::CURRENT,
                Some(CountryCode::DEU),
                None,
                Some(Price::new(9000_u64.into(), Currency::Eur)),
                true,
            )?),
            geography: Some(geographic_query(None, None, false, false)),
            ..Default::default()
        };
        let result = search(query.clone(), price_filter(None)?).await?;
        assert_eq!(Some(1), result.total);
        assert_eq!(ids[0], result.items[0].product_listing_id);
        documents[0]["shipping"]["countryOffers"][0]["perItemFixed"]["amountMinor"] = json!(9500);
        index_products([documents[0].clone()]).await?;
        assert_eq!(
            Some(0),
            search(query.clone(), price_filter(None)?).await?.total
        );
        query.shipping = Some(ShippingSearch::new(
            ReferenceRelease::CURRENT,
            None,
            None,
            None,
            true,
        )?);
        let reader = product_listing_opensearch::OpenSearchProductListingSearchReader::with_index(
            get_opensearch_client().await.clone(),
            &products_index(),
        );
        let mut request = ProductListingSearchReadRequest {
            compiled_search: CompiledProductListingSearch::new(query, price_filter(None)?)?,
            sort: None,
            cursor: Some(Cursor {
                size: 1,
                search_after: None,
            }),
        };
        let first = reader.search(&request).await?;
        assert_eq!(Some(2), first.total);
        request.cursor.as_mut().unwrap().search_after = first.cursor.search_after;
        let second = reader.search(&request).await?;
        assert_eq!(Some(2), second.total);
        assert_ne!(
            first.items[0].product_listing_id,
            second.items[0].product_listing_id
        );
        assert_eq!(
            std::collections::HashSet::from([ids[0], ids[1]]),
            std::collections::HashSet::from([
                first.items[0].product_listing_id,
                second.items[0].product_listing_id
            ])
        );

        // A fresh FX basis changes membership; the original plan keeps exact pages and totals.
        let fx_query = ProductListingSearch {
            shipping: Some(
                ShippingSearch::new(
                    ReferenceRelease::CURRENT,
                    Some(CountryCode::DEU),
                    Some(ShippingServiceCode::new("gallery-delivery".into())?),
                    Some(Price::new(9000_u64.into(), Currency::Usd)),
                    false,
                )?
                .with_fx_conversion()?,
            ),
            ..Default::default()
        };
        let mut pinned = ProductListingSearchReadRequest {
            compiled_search: CompiledProductListingSearch::new(
                fx_query.clone(),
                price_filter(None)?,
            )?,
            sort: None,
            cursor: Some(Cursor {
                size: 1,
                search_after: None,
            }),
        };
        let first = reader.search(&pinned).await?;
        assert_eq!(Some(3), first.total);
        let mut found = std::collections::HashSet::from([first.items[0].product_listing_id]);
        pinned.cursor.as_mut().unwrap().search_after = first.cursor.search_after;
        let changed_snapshot = fxrate_core::NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
            fxrate_core::FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                fxrate_core::FxRateQuote::new(
                    currency,
                    if currency == Currency::Usd {
                        2_000_000
                    } else {
                        fxrate_core::FX_RATE_SCALE
                    },
                )
            }),
        )?
        .into_persisted(2_i64.try_into()?);
        let changed_plan =
            ProductListingPriceFilterPlan::compile(changed_snapshot, Currency::Usd, None)?;
        let fresh = reader
            .search(&ProductListingSearchReadRequest {
                compiled_search: CompiledProductListingSearch::new(fx_query, changed_plan)?,
                cursor: Some(Cursor {
                    size: 20,
                    search_after: None,
                }),
                sort: None,
            })
            .await?;
        assert_eq!(Some(2), fresh.total);
        for _ in 1..3 {
            let page = reader.search(&pinned).await?;
            assert_eq!(Some(3), page.total);
            assert_eq!(1, page.items.len());
            assert!(found.insert(page.items[0].product_listing_id));
            pinned.cursor.as_mut().unwrap().search_after = page.cursor.search_after;
        }
        assert_eq!(
            std::collections::HashSet::from([ids[3], ids[4], ids[7]]),
            found
        );
        Ok(())
    }
    assert_ok(run().await);
}
