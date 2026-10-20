use application::pagination::{Cursor, CursoredResult};
use auction_core::{
    AuctionFormat, AuctionId, AuctionName, AuctionReportedStatus, AuctionSchedule,
    AuctionSchedulePoint, ReportedCatalogueLotCount,
};
use auction_service::ports::{
    AuctionDirectoryCursor, AuctionDirectoryScope, PublicAuctionDetails, PublicAuctionSourceSummary,
};
use aura_historia_contracts::auctions::*;
use domain_primitives::sort::SortOrder;
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use localization::{Language, Localized};
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use time::OffsetDateTime;

fn input_schema<T: JsonSchema>() -> Value {
    serde_json::to_value(
        SchemaSettings::default()
            .for_deserialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap()
}
fn output_schema<T: JsonSchema>() -> Value {
    serde_json::to_value(
        SchemaSettings::default()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<T>(),
    )
    .unwrap()
}

#[test]
fn should_preserve_all_directory_fields_and_validate_schedule_points()
-> Result<(), Box<dyn std::error::Error>> {
    let source = ListingSourceId::new();
    let args = json!({
        "listingSourceId": source,
        "format": "TIMED",
        "reportedStatus": "IN_PROGRESS",
        "timeRole": "LOTS_BEGIN_CLOSING",
        "sort": "scheduled",
        "order": "desc",
        "from": "2026-10-01T00:00:00Z",
        "to": "2026-11-01T00:00:00Z",
        "pageSize": 73,
    });
    assert!(jsonschema::is_valid(
        &input_schema::<DirectoryQuery>(),
        &args
    ));
    let request = serde_json::from_value::<DirectoryQuery>(args)?.try_into_request()?;
    assert_eq!(Some(source), request.listing_source_id);
    assert_eq!(Some(AuctionFormat::Timed), request.format);
    assert_eq!(
        Some(AuctionReportedStatus::InProgress),
        request.reported_status
    );
    assert_eq!(Some(AuctionSchedulePoint::LotsBeginClosing), request.sort);
    assert_eq!(SortOrder::Desc, request.order());
    let range = request.schedule.ok_or("missing schedule")?;
    assert_eq!(AuctionSchedulePoint::LotsBeginClosing, range.role);
    assert_eq!(
        Some(time::macros::datetime!(2026-10-01 00:00 UTC)),
        range.range.min
    );
    assert_eq!(
        Some(time::macros::datetime!(2026-11-01 00:00 UTC)),
        range.range.max
    );
    assert_eq!(73, request.cursor.ok_or("missing cursor")?.size);
    for invalid in [
        json!({"sort":"scheduled"}),
        json!({"sort":"bogus"}),
        json!({"order":"up"}),
        json!({"timeRole":"LIVE_STARTS"}),
        json!({"timeRole":"LIVE_STARTS","from":"2026-01-01T00:00:00Z"}),
        json!({"sort":"scheduled","timeRole":"LIVE_STARTS","to":"2026-01-01T00:00:00Z"}),
        json!({"listingSourceId":"wrong"}),
        json!({"format":"timed"}),
        json!({"reportedStatus":"bogus"}),
        json!({"timeRole":"bogus"}),
    ] {
        assert!(
            serde_json::from_value::<DirectoryQuery>(invalid.clone())?
                .try_into_request()
                .is_err(),
            "{invalid}"
        );
    }
    Ok(())
}

#[test]
fn should_preserve_created_and_nullable_scheduled_directory_cursors()
-> Result<(), Box<dyn std::error::Error>> {
    for (sort, order, scheduled) in [
        (None, None, None),
        (None, Some(SortOrder::Asc), None),
        (
            Some(AuctionSchedulePoint::LiveStarts),
            Some(SortOrder::Asc),
            None,
        ),
        (
            Some(AuctionSchedulePoint::ScheduledEnd),
            Some(SortOrder::Desc),
            Some(OffsetDateTime::UNIX_EPOCH),
        ),
    ] {
        let scope = AuctionDirectoryScope {
            listing_source_id: Some(ListingSourceId::new()),
            format: Some(AuctionFormat::Live),
            reported_status: None,
            schedule: None,
            sort,
            order,
        };
        let expected = AuctionDirectoryCursor {
            created: OffsetDateTime::UNIX_EPOCH,
            auction_id: AuctionId::new(),
            scheduled,
            scope: scope.clone(),
        };
        let data = PublicAuctionDirectoryData::from(CursoredResult {
            items: vec![],
            cursor: Cursor {
                size: 42,
                search_after: Some(expected.clone()),
            },
            total: None,
        });
        let value = serde_json::to_value(data)?;
        assert!(
            jsonschema::is_valid(&output_schema::<PublicAuctionDirectoryData>(), &value),
            "{value}"
        );
        let cursor = value["searchAfter"].clone();
        assert_eq!(expected, parse_directory_cursor(cursor.to_string())?);
        if sort.is_some() {
            assert!(cursor.get("scheduled").is_some());
            let mut missing = cursor.clone();
            missing
                .as_object_mut()
                .ok_or("not object")?
                .remove("scheduled");
            assert!(parse_directory_cursor(missing.to_string()).is_err());
        } else {
            assert!(cursor.get("scheduled").is_none());
            if order.is_none() {
                assert!(cursor["scope"].get("sort").is_none());
                assert!(cursor["scope"].get("order").is_none());
            }
        }
        let request =
            serde_json::from_value::<DirectoryQuery>(json!({"searchAfter":cursor.to_string()}))?
                .try_into_request()?;
        assert_eq!(Some(expected), request.cursor.and_then(|c| c.search_after));
    }
    Ok(())
}

#[test]
fn should_preserve_catalogue_defaults_aliases_and_position_cursor()
-> Result<(), Box<dyn std::error::Error>> {
    let auction = AuctionId::new();
    let listing = product_listing_core::product_listing_id::ProductListingId::new();
    let cursor = json!({"auctionId":auction,"cataloguePosition":7,"productListingId":listing});
    let args =
        json!({"language":"de-DE","currency":"USD","pageSize":33,"searchAfter":cursor.to_string()});
    assert!(jsonschema::is_valid(
        &input_schema::<CatalogueQuery>(),
        &args
    ));
    let request = serde_json::from_value::<CatalogueQuery>(args)?.try_into_request(auction)?;
    assert_eq!(Language::De, request.language);
    assert_eq!(money::Currency::Usd, request.currency);
    let actual = request.cursor.ok_or("missing cursor")?;
    assert_eq!(33, actual.size);
    assert_eq!(
        Some(product_listing_service::ports::AuctionCatalogueCursor {
            auction_id: auction,
            catalogue_position: Some(7),
            product_listing_id: listing
        }),
        actual.search_after
    );
    let default = serde_json::from_value::<CatalogueQuery>(json!({}))?.try_into_request(auction)?;
    assert_eq!(Language::default(), default.language);
    assert_eq!(money::Currency::default(), default.currency);
    assert_eq!(
        Some(Cursor {
            size: 21,
            search_after: None
        }),
        default.cursor
    );
    for invalid in [
        json!({"language":"unknown"}),
        json!({"currency":"usd"}),
        json!({"pageSize":-1}),
        json!({"extra":true}),
    ] {
        assert!(!jsonschema::is_valid(
            &input_schema::<CatalogueQuery>(),
            &invalid
        ));
        assert!(serde_json::from_value::<CatalogueQuery>(invalid).is_err());
    }
    for cursor in [
        json!({"auctionId":"invalid","productListingId":listing}),
        json!({"auctionId":auction,"productListingId":"invalid"}),
        json!({"auctionId":auction,"productListingId":listing,"private":true}),
    ] {
        assert!(parse_catalogue_cursor(cursor.to_string()).is_err());
    }
    Ok(())
}

#[test]
fn should_emit_complete_public_auction_metadata_and_direct_schedule_without_admin_fields()
-> Result<(), Box<dyn std::error::Error>> {
    let auction = AuctionId::new();
    let source = ListingSourceId::new();
    let view = PublicAuctionDetails {
        auction_id: auction,
        source: PublicAuctionSourceSummary {
            listing_source_id: source,
            name: ListingSourceName::try_from("Source")?,
            slug_id: ListingSourceSlugId::raw("source")?,
            referral_configuration: None,
        },
        name: Some(Localized::new(
            Language::En,
            AuctionName::try_from("Autumn")?,
        )),
        catalogue_url: Some(url::Url::parse("https://example.test/catalogue")?),
        format: Some(AuctionFormat::Timed),
        schedule: AuctionSchedule::new(None, Some(OffsetDateTime::UNIX_EPOCH), None, None)?,
        reported_status: Some(AuctionReportedStatus::Scheduled),
        reported_lot_count: Some(ReportedCatalogueLotCount::new(53)),
        visible_active_assigned_listing_count: 42,
    };
    let value = serde_json::to_value(PublicAuctionData::try_from(view)?)?;
    assert!(jsonschema::is_valid(
        &output_schema::<PublicAuctionData>(),
        &value
    ));
    assert_eq!(
        json!({"listingSourceId":source,"name":"Source","slugId":"source"}),
        value["listingSource"]
    );
    assert_eq!(json!({"text":"Autumn","language":"en"}), value["name"]);
    assert_eq!(
        json!({"biddingOpens":null,"liveStarts":"1970-01-01T00:00:00Z","lotsBeginClosing":null,"scheduledEnd":null}),
        value["schedule"]
    );
    assert_eq!("TIMED", value["format"]);
    assert_eq!("SCHEDULED", value["reportedStatus"]);
    assert_eq!(53, value["reportedLotCount"]);
    assert_eq!(42, value["visibleListingCount"]);
    assert!(
        value["viewUrl"]
            .as_str()
            .ok_or("no URL")?
            .contains("utm_source=aura_historia")
    );
    assert_eq!("https://example.test/catalogue", value["catalogueUrl"]);
    for private in [
        "sourceAuctionId",
        "description",
        "evidence",
        "expectedVersion",
        "created",
        "updated",
    ] {
        assert!(value.get(private).is_none(), "{private}");
    }
    Ok(())
}

#[test]
fn should_describe_supported_directory_codes_and_identifiers_in_schema() {
    let schema = input_schema::<DirectoryQuery>();
    for invalid in [
        json!({"format": "timed"}),
        json!({"reportedStatus": "unknown"}),
        json!({"timeRole": "LIVE_START"}),
        json!({"sort": "other"}),
        json!({"order": "up"}),
        json!({"listingSourceId": "invalid"}),
    ] {
        assert!(!jsonschema::is_valid(&schema, &invalid), "{invalid}");
        assert!(
            serde_json::from_value::<DirectoryQuery>(invalid)
                .unwrap()
                .try_into_request()
                .is_err()
        );
    }
}
