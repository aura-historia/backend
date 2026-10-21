use application::pagination::{Cursor, CursoredResult};
use aura_historia_contracts::{
    product_listings::presentation::ProductListingPresentationData,
    watchlist::{
        CursoredWatchlistData, ListWatchlistQueryData, PatchWatchlistData, PostWatchlistData,
        WatchlistEntryData, parse_watchlist_cursor, watchlist_cursor, watchlist_cursor_value,
    },
};
use localization::Language;
use money::Currency;
use product_listing_core::product_listing_id::ProductListingId;
use product_listing_service::ports::ProductListingWatchlistDetailsCursor;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::{Value, json};
use time::OffsetDateTime;
use user_core::user_id::UserId;
use watchlist_core::{
    NewWatchlistProductListing, WatchlistProductListing, watchlist_state::WatchlistState,
};
use watchlist_service::ports::WatchlistProductListingView;

fn schema<T: JsonSchema>(input: bool) -> Value {
    let settings = if input {
        SchemaSettings::default().for_deserialize()
    } else {
        SchemaSettings::default().for_serialize()
    };
    serde_json::to_value(settings.into_generator().into_root_schema_for::<T>()).unwrap()
}

#[test]
fn should_preserve_add_defaults_null_values_and_identifier_validation() {
    let user = UserId::new();
    let product = ProductListingId::new();
    let schema = schema::<PostWatchlistData>(true);
    for (input, notifications) in [
        (json!({"productListingId":product}), true),
        (
            json!({"productListingId":product,"notifications":null}),
            true,
        ),
        (
            json!({"productListingId":product,"notifications":true}),
            true,
        ),
        (
            json!({"productListingId":product,"notifications":false}),
            false,
        ),
    ] {
        assert!(jsonschema::is_valid(&schema, &input));
        let command = serde_json::from_value::<PostWatchlistData>(input)
            .unwrap()
            .into_command(user)
            .unwrap();
        assert_eq!(user, command.user_id);
        assert_eq!(product, command.product_listing_id);
        assert_eq!(notifications, command.notifications);
    }
    for input in [
        json!({}),
        json!({"productListingId":user}),
        json!({"productListingId":product.as_uuid()}),
        json!({"productListingId":product,"notifications":"true"}),
    ] {
        let accepted = serde_json::from_value::<PostWatchlistData>(input.clone())
            .ok()
            .is_some_and(|data| data.into_command(user).is_ok());
        assert!(!accepted, "{input}");
        assert!(!jsonschema::is_valid(&schema, &input), "{input}");
    }
}

#[test]
fn should_preserve_patch_omission_and_reject_null_or_system_managed_state() {
    let schema = schema::<PatchWatchlistData>(true);
    let user = UserId::new();
    let product = ProductListingId::new();
    for (input, valid) in [
        (json!({}), true),
        (json!({"notifications":false}), true),
        (json!({"state":"ACTIVE","notifications":true}), true),
        (json!({"state":"INACTIVE_BY_USER"}), true),
        (json!({"state":"INACTIVE_BY_RESTRICTED_PLAN"}), false),
        (json!({"state":null}), false),
        (json!({"notifications":null}), false),
        (json!({"notifications":0}), false),
    ] {
        let accepted = serde_json::from_value::<PatchWatchlistData>(input.clone())
            .ok()
            .is_some_and(|data| {
                data.into_command(user, product, ProductListingPresentationData::default())
                    .is_ok()
            });
        assert_eq!(valid, accepted, "input {input}");
        assert_eq!(
            valid,
            jsonschema::is_valid(&schema, &input),
            "schema {input}"
        );
    }
    let omitted = serde_json::from_value::<PatchWatchlistData>(json!({}))
        .unwrap()
        .into_command(user, product, ProductListingPresentationData::default())
        .unwrap();
    assert_eq!(None, omitted.notifications);
    assert_eq!(None, omitted.state);
    assert_eq!(Language::En, omitted.language);
    assert_eq!(Currency::Eur, omitted.currency);
}

#[test]
fn should_preserve_list_defaults_clamping_and_timestamp_identifier_cursor() {
    let product = ProductListingId::new();
    let user = UserId::new();
    let query: ListWatchlistQueryData = serde_json::from_value(json!({})).unwrap();
    let request = query.into_request(user).unwrap();
    assert_eq!(user, request.user_id);
    assert_eq!(21, request.cursor.size);
    assert_eq!(Language::En, request.language);
    assert_eq!(Currency::Eur, request.currency);
    let cursor = ProductListingWatchlistDetailsCursor {
        watchlist_created: OffsetDateTime::UNIX_EPOCH,
        product_listing_id: product,
    };
    let value = watchlist_cursor_value(cursor).unwrap();
    let query: ListWatchlistQueryData = serde_json::from_value(json!({"language":"de-DE","currency":"USD","size":u64::MAX,"searchAfter":value.to_string()})).unwrap();
    let request = query.into_request(user).unwrap();
    assert_eq!(Language::De, request.language);
    assert_eq!(Currency::Usd, request.currency);
    assert_eq!(100, request.cursor.size);
    assert_eq!(Some(cursor), request.cursor.search_after);
    assert_eq!(1, watchlist_cursor(Some(0), None).unwrap().size);
    for invalid in [
        json!({}),
        json!([]),
        json!(["invalid", product]),
        json!(["1970-01-01T00:00:00Z", user]),
        json!(["1970-01-01T00:00:00Z", product.as_uuid()]),
        json!(["1970-01-01T00:00:00Z", product, "extra"]),
    ] {
        assert!(parse_watchlist_cursor(invalid).is_err());
    }
    assert!(watchlist_cursor(None, Some("not json")).is_err());
    for invalid in [
        json!({"language":null}),
        json!({"currency":"XXX"}),
        json!({"searchAfter":[]}),
        json!({"size":-1}),
    ] {
        assert!(serde_json::from_value::<ListWatchlistQueryData>(invalid.clone()).is_err());
        assert!(!jsonschema::is_valid(
            &schema::<ListWatchlistQueryData>(true),
            &invalid
        ));
    }
}

#[test]
fn should_serialize_entry_states_and_optional_timestamps_in_the_existing_shape() {
    let user = UserId::new();
    let product = ProductListingId::new();
    let schema = schema::<WatchlistEntryData>(false);
    let entry = WatchlistEntryData::from(WatchlistProductListing::create(
        NewWatchlistProductListing {
            user_id: user,
            product_listing_id: product,
            notifications: false,
            state: WatchlistState::Active,
        },
    ));
    let value = serde_json::to_value(entry).unwrap();
    assert_eq!(
        json!({"userId":user,"productListingId":product,"notifications":false,"state":"ACTIVE"}),
        value
    );
    assert!(jsonschema::is_valid(&schema, &value));
    let view = WatchlistEntryData::from(WatchlistProductListingView {
        user_id: user,
        product_listing_id: product,
        notifications: true,
        state: WatchlistState::InactiveByRestrictedPlan,
        created: OffsetDateTime::UNIX_EPOCH,
        updated: OffsetDateTime::UNIX_EPOCH,
    });
    let value = serde_json::to_value(view).unwrap();
    assert_eq!("INACTIVE_BY_RESTRICTED_PLAN", value["state"]);
    assert_eq!("1970-01-01T00:00:00Z", value["created"]);
    assert_eq!("1970-01-01T00:00:00Z", value["updated"]);
    assert!(jsonschema::is_valid(&schema, &value));
}

#[test]
fn should_report_returned_size_and_omit_absent_page_metadata() {
    let schema = schema::<CursoredWatchlistData>(false);
    let data = CursoredWatchlistData::try_from(CursoredResult {
        items: vec![],
        cursor: Cursor {
            size: 21,
            search_after: None,
        },
        total: None,
    })
    .unwrap();
    let value = serde_json::to_value(data).unwrap();
    assert_eq!(json!({"items":[],"size":0}), value);
    assert!(jsonschema::is_valid(&schema, &value));
    let cursor = ProductListingWatchlistDetailsCursor {
        watchlist_created: OffsetDateTime::UNIX_EPOCH,
        product_listing_id: ProductListingId::new(),
    };
    let data = CursoredWatchlistData::try_from(CursoredResult {
        items: vec![],
        cursor: Cursor {
            size: 21,
            search_after: Some(cursor),
        },
        total: Some(4),
    })
    .unwrap();
    let value = serde_json::to_value(data).unwrap();
    assert_eq!(
        watchlist_cursor_value(cursor).unwrap(),
        value["searchAfter"]
    );
    assert_eq!(4, value["total"]);
    assert!(jsonschema::is_valid(&schema, &value));
}
