use super::*;
use auction_core::AuctionId;
use localization::Language;

#[test]
fn top_level_nulls_and_omissions_leave_saved_filter_fields_unchanged()
-> Result<(), Box<dyn std::error::Error>> {
    for body in [
        serde_json::json!({}),
        serde_json::json!({"name": null}),
        serde_json::json!({"notifications": null}),
        serde_json::json!({"state": null}),
        serde_json::json!({"search": null}),
        serde_json::json!({"acceptShippingDestination": null}),
        serde_json::json!({
            "name": null,
            "notifications": null,
            "state": null,
            "search": null,
            "acceptShippingDestination": null
        }),
    ] {
        let data: UpdateSearchFilterData = serde_json::from_value(body)?;
        assert!(data.accept_shipping_destination.is_none());
        assert_eq!(
            (
                PatchField::Unchanged,
                PatchField::Unchanged,
                PatchField::Unchanged,
                ProductListingSearchPatch::default(),
            ),
            data.into_fields()?
        );
    }
    Ok(())
}

#[test]
fn null_top_level_members_preserve_concrete_updates_and_nested_clears()
-> Result<(), Box<dyn std::error::Error>> {
    let data: UpdateSearchFilterData = serde_json::from_value(serde_json::json!({
        "name": null,
        "notifications": false,
        "state": "INACTIVE_BY_USER",
        "search": {"geography": null, "shipping": null}
    }))?;
    let (name, notifications, state, search) = data.into_fields()?;
    assert_eq!(PatchField::Unchanged, name);
    assert_eq!(PatchField::Set(false), notifications);
    assert_eq!(PatchField::Set(SearchFilterState::InactiveByUser), state);
    assert_eq!(PatchField::Clear, search.geography);
    assert_eq!(PatchField::Clear, search.shipping);

    for body in [
        serde_json::json!({"search": {"language": null}}),
        serde_json::json!({"search": {"currency": null}}),
        serde_json::json!({"search": {"productQuery": null}}),
    ] {
        let data: UpdateSearchFilterData = serde_json::from_value(body)?;
        assert_eq!(
            crate::error::BAD_BODY_VALUE,
            data.into_fields().unwrap_err().code()
        );
    }
    Ok(())
}

#[test]
fn saved_filter_creation_honors_explicit_notifications_and_defaults_to_enabled()
-> Result<(), serde_json::Error> {
    for (notifications, expected) in [(None, true), (Some(true), true), (Some(false), false)] {
        let mut body = serde_json::json!({"name": "saved", "search": {}});
        if let Some(value) = notifications {
            body["notifications"] = serde_json::json!(value);
        }
        let data: CreateSearchFilterData = serde_json::from_value(body)?;
        assert_eq!(expected, data.notifications);
    }
    Ok(())
}

#[test]
fn geographic_patches_replace_clear_or_preserve_private_snapshots()
-> Result<(), Box<dyn std::error::Error>> {
    let omitted: UpdateSearchFilterData =
        serde_json::from_value(serde_json::json!({"name":"renamed"}))?;
    assert_eq!(PatchField::Unchanged, omitted.into_fields()?.3.geography);
    let set: UpdateSearchFilterData = serde_json::from_value(
        serde_json::json!({"search":{"geography":{"origin":{"latitude":48.137,"longitude":11.575},"radius":{"amount":25,"unit":"KILOMETERS"}}}}),
    )?;
    let patch = set.into_fields()?.3;
    let PatchField::Set(query) = patch.geography else {
        panic!("expected coordinate snapshot");
    };
    assert_eq!(Some(25.0), query.radius().map(geo::Distance::amount));
    assert_eq!(48.137, query.origin().unwrap().latitude_degrees());
    let cleared: UpdateSearchFilterData =
        serde_json::from_value(serde_json::json!({"search":{"geography":null}}))?;
    assert_eq!(PatchField::Clear, cleared.into_fields()?.3.geography);
    Ok(())
}

#[test]
fn should_map_only_supplied_nested_search_fields_to_product_search_patch()
-> Result<(), Box<dyn std::error::Error>> {
    let data: UpdateSearchFilterData = serde_json::from_str(
        r#"{
            "search": {
                "language": "de",
                "productQuery": ["cabinet"],
                "price": { "min": 10, "max": 20 },
                "availability": ["AVAILABLE"],
                "orderability": ["ORDERABLE_NOW"],
                "includeUnspecifiedAvailability": true
            }
        }"#,
    )?;

    let (_, _, _, patch) = data.into_fields()?;

    assert!(matches!(patch.language, PatchField::Set(Language::De)));
    assert!(matches!(patch.currency, PatchField::Unchanged));
    assert!(matches!(
        patch.enhanced_search_description,
        PatchField::Unchanged
    ));
    let values = match patch.product_listing_query {
        PatchField::Set(values) => values,
        PatchField::Unchanged | PatchField::Clear => {
            return Err(std::io::Error::other("product query was not set").into());
        }
    };
    assert_eq!("cabinet", values[0].as_ref());
    assert!(matches!(patch.price_query, PatchField::Set(_)));
    assert_eq!(
        PatchField::Set(ListingAvailabilityQuery {
            any_of: HashSet::from([ListingAvailability::Available]).into(),
            orderability: HashSet::from([ListingOrderability::OrderableNow]).into(),
            include_unspecified: true,
        }),
        patch.availability_query
    );
    assert!(matches!(
        patch.listing_source_id_query,
        PatchField::Unchanged
    ));
    assert!(matches!(patch.auction_id_query, PatchField::Unchanged));
    Ok(())
}

#[test]
fn should_round_trip_and_validate_saved_filter_auction_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let auction_id = AuctionId::new();
    let search = ProductListingSearch::new(Language::En, Currency::Eur)
        .with_auction_id_query(HashSet::from([auction_id]).into());

    let value = serde_json::to_value(ProductListingSearchData::from(search.clone()))?;
    assert_eq!(
        Some(&serde_json::json!([auction_id.to_string()])),
        value.get("auctionId")
    );
    let decoded = ProductListingSearch::try_from(serde_json::from_value::<
        ProductListingSearchData,
    >(value)?)?;
    assert_eq!(search, decoded);

    for invalid in [
        ProductListingId::new().to_string(),
        auction_id.as_uuid().to_string(),
    ] {
        let data: ProductListingSearchData = serde_json::from_value(serde_json::json!({
            "auctionId": [invalid]
        }))?;
        let error = ProductListingSearch::try_from(data)
            .err()
            .ok_or("invalid ID accepted")?;
        assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
    }
    Ok(())
}

#[test]
fn should_round_trip_and_validate_saved_filter_excluded_product_listing_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let product_listing_id = ProductListingId::new();
    let search = ProductListingSearch::new(Language::En, Currency::Eur)
        .with_exclude_product_listing_id_query(HashSet::from([product_listing_id]).into());

    let value = serde_json::to_value(ProductListingSearchData::from(search.clone()))?;
    assert_eq!(
        Some(&serde_json::json!([product_listing_id.to_string()])),
        value.get("excludeProductListingId")
    );
    assert!(value.get("excludeProductId").is_none());
    let decoded = ProductListingSearch::try_from(serde_json::from_value::<
        ProductListingSearchData,
    >(value)?)?;
    assert_eq!(search, decoded);

    let data: ProductListingSearchData = serde_json::from_value(serde_json::json!({
        "excludeProductListingId": [product_listing_id.as_uuid().to_string()]
    }))?;
    let error = ProductListingSearch::try_from(data)
        .err()
        .ok_or("invalid ID accepted")?;
    assert_eq!(crate::error::INVALID_OBJECT_ID, error.code());
    Ok(())
}

#[test]
fn should_patch_set_and_clear_saved_filter_auction_ids()
-> Result<(), Box<dyn std::error::Error>> {
    let auction_id = AuctionId::new();
    let set: UpdateSearchFilterData = serde_json::from_value(serde_json::json!({
        "search": { "auctionId": [auction_id.to_string()] }
    }))?;
    let (_, _, _, patch) = set.into_fields()?;
    assert!(matches!(patch.auction_id_query, PatchField::Set(_)));

    let clear: UpdateSearchFilterData =
        serde_json::from_str(r#"{ "search": { "auctionId": null } }"#)?;
    let (_, _, _, patch) = clear.into_fields()?;
    assert_eq!(PatchField::Clear, patch.auction_id_query);
    Ok(())
}

#[test]
fn should_preserve_absent_and_configured_empty_availability_queries()
-> Result<(), Box<dyn std::error::Error>> {
    let absent =
        ProductListingSearchData::from(ProductListingSearch::new(Language::En, Currency::Eur));
    let absent_value = serde_json::to_value(absent)?;
    assert!(absent_value.get("availability").is_none());
    assert!(absent_value.get("orderability").is_none());
    assert!(absent_value.get("includeUnspecifiedAvailability").is_none());

    let mut configured_empty = ProductListingSearch::new(Language::En, Currency::Eur);
    configured_empty.availability_query = Some(ListingAvailabilityQuery {
        any_of: Default::default(),
        orderability: Default::default(),
        include_unspecified: false,
    });
    let value = serde_json::to_value(ProductListingSearchData::from(configured_empty))?;
    assert_eq!(Some(&serde_json::json!([])), value.get("availability"));
    assert_eq!(Some(&serde_json::json!([])), value.get("orderability"));
    assert_eq!(
        Some(&serde_json::json!(false)),
        value.get("includeUnspecifiedAvailability")
    );
    let data: ProductListingSearchData = serde_json::from_value(value)?;
    let decoded = ProductListingSearch::try_from(data)?;
    assert_eq!(
        Some(ListingAvailabilityQuery {
            any_of: Default::default(),
            orderability: Default::default(),
            include_unspecified: false,
        }),
        decoded.availability_query
    );
    Ok(())
}

#[test]
fn should_require_an_atomic_availability_query_patch() -> Result<(), Box<dyn std::error::Error>>
{
    let clear: UpdateSearchFilterData = serde_json::from_str(
        r#"{ "search": {
            "availability": null,
            "orderability": null,
            "includeUnspecifiedAvailability": null
        } }"#,
    )?;
    let (_, _, _, patch) = clear.into_fields()?;
    assert_eq!(PatchField::Clear, patch.availability_query);

    let partial: UpdateSearchFilterData =
        serde_json::from_str(r#"{ "search": { "orderability": null } }"#)?;
    assert!(partial.into_fields().is_err());

    let omitted: UpdateSearchFilterData = serde_json::from_str(r#"{ "search": {} }"#)?;
    let (_, _, _, patch) = omitted.into_fields()?;
    assert_eq!(PatchField::Unchanged, patch.availability_query);
    Ok(())
}
#[test]
fn shipping_patch_omission_replacement_null_and_explicit_none_are_distinct() {
    for value in [
        serde_json::json!({"search":{"shipping":null}}),
        serde_json::json!({"search":{"shipping":{"destination":{"kind":"NONE"}}}}),
    ] {
        let data: UpdateSearchFilterData = serde_json::from_value(value).unwrap();
        assert_eq!(PatchField::Clear, data.into_fields().unwrap().3.shipping);
    }
    let unchanged: UpdateSearchFilterData =
        serde_json::from_value(serde_json::json!({"search":{"currency":"USD"}})).unwrap();
    assert_eq!(
        PatchField::Unchanged,
        unchanged.into_fields().unwrap().3.shipping
    );
    let data: UpdateSearchFilterData = serde_json::from_value(serde_json::json!({"search":{"shipping":{"destination":{"kind":"COUNTRY","country":"DE"},"maximumPerItem":{"amount":9000,"currency":"EUR"}}}})).unwrap();
    let PatchField::Set(shipping) = data.into_fields().unwrap().3.shipping else {
        panic!("shipping replacement must be set")
    };
    assert_eq!(Some(geo::CountryCode::DEU), shipping.country());
    assert_eq!(
        Some(money::Price::new(9000_u64.into(), Currency::Eur)),
        shipping.maximum_per_item()
    );
}
