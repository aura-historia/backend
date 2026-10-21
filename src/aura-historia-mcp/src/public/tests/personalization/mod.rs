mod fixtures;
mod ports;

use super::{
    authentication::{authenticated_app, authenticated_call},
    *,
};
use fixtures::{FILTER_ID, ListingFixture, NOTIFICATION_ID};
use std::sync::atomic::Ordering;
use user_core::access_token::Scope;

fn services(fixture: &ListingFixture) -> PublicServices {
    let mut services = Arc::new(Fixture::default()).services();
    services.listings = fixture.listings();
    services.auctions.catalogue = fixture.catalogue();
    services
}

fn listing_inputs() -> Vec<(&'static str, Value)> {
    minimal_inputs()
        .into_iter()
        .filter(|(name, _)| {
            matches!(
                *name,
                "simpleSearchProductListings"
                    | "getProductListing"
                    | "getProductListingByTitleSlug"
                    | "getSimilarProductListings"
                    | "getAuctionCatalogue"
            )
        })
        .collect()
}

fn listing<'a>(name: &str, output: &'a Value) -> &'a Value {
    match name {
        "simpleSearchProductListings" | "getSimilarProductListings" | "getAuctionCatalogue" => {
            &output["items"][0]
        }
        _ => output,
    }
}

fn scoped_fields(mask: u8) -> (Vec<Scope>, BTreeSet<&'static str>) {
    let mut scopes = vec![];
    let mut fields = BTreeSet::new();
    for (bit, scope, field) in [
        (1, Scope::WatchlistRead, "watchlist"),
        (2, Scope::SearchFiltersRead, "searchFilter"),
        (4, Scope::NotificationsRead, "notification"),
        (8, Scope::UsersRead, "contentVisibility"),
    ] {
        if mask & bit != 0 {
            scopes.push(scope);
            fields.insert(field);
        }
    }
    if mask & 8 != 0 {
        fields.insert("shipping");
    }
    (scopes, fields)
}

fn assert_private_fields(value: &Value, fields: &BTreeSet<&str>) {
    let state = value.get("userState");
    if fields.is_empty() {
        assert!(
            state.is_none(),
            "unauthorized userState must be omitted: {value}"
        );
        return;
    }
    let state = state.unwrap().as_object().unwrap();
    assert_eq!(*fields, state.keys().map(String::as_str).collect());
    if fields.contains("watchlist") {
        assert_eq!(
            json!({"watching": true, "notifications": true}),
            state["watchlist"]
        );
    }
    if fields.contains("searchFilter") {
        assert_eq!(true, state["searchFilter"]["matched"]);
        assert_eq!(FILTER_ID, state["searchFilter"]["userSearchFilterId"]);
        assert_eq!("Cabinets", state["searchFilter"]["userSearchFilterName"]);
        assert_eq!(true, state["searchFilter"]["matchFeedback"]);
    }
    if fields.contains("notification") {
        assert_eq!(
            json!([NOTIFICATION_ID]),
            state["notification"]["unseenNotificationIds"]
        );
    }
    if fields.contains("shipping") {
        assert_eq!("DE", state["shipping"]["destination"]["country"]);
    }
}

#[tokio::test]
async fn should_omit_private_state_anonymously_with_real_listing_services() {
    let fixture = ListingFixture::new(false, true);
    let app = router(
        TransportConfig {
            allowed_hosts: vec!["mcp.example.test".into()],
            ..Default::default()
        },
        registry(services(&fixture)).unwrap(),
        Arc::new(AnonymousGate),
    )
    .unwrap();
    for (name, arguments) in listing_inputs() {
        let (status, response, _) = post(
            app.clone(),
            "tools/call",
            json!({"name":name,"arguments":arguments}),
        )
        .await;
        assert_eq!(StatusCode::OK, status);
        assert!(response.get("error").is_none(), "{name}: {response}");
        assert_ne!(
            Some(true),
            response["result"]["isError"].as_bool(),
            "{name}: {response}"
        );
        let output = &response["result"]["structuredContent"];
        let value = listing(name, output);
        assert_private_fields(value, &BTreeSet::new());
        assert!(value["item"]["images"][0]["url"].is_null());
        assert_eq!(
            "REQUIRES_CONSENT",
            value["item"]["contentPolicy"]["decision"]
        );
    }
    assert_eq!(0, fixture.destination_reads.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_disclose_exact_sections_for_all_read_scope_combinations_through_real_services() {
    for mask in 0..16 {
        let fixture = ListingFixture::new(false, true);
        let (scopes, fields) = scoped_fields(mask);
        let registry = registry(services(&fixture)).unwrap();
        let app = authenticated_app(services(&fixture), fixture.user_id, scopes);
        for (name, arguments) in listing_inputs() {
            let output = authenticated_call(&app, name, arguments).await;
            let schema = serde_json::to_value(
                registry
                    .get(name)
                    .unwrap()
                    .tool()
                    .output_schema
                    .as_ref()
                    .unwrap(),
            )
            .unwrap();
            assert!(
                jsonschema::is_valid(&schema, &output),
                "{name} mask {mask}: {output}"
            );
            let value = listing(name, &output);
            assert_private_fields(value, &fields);
            // Stored consent still controls images when users:read itself is absent.
            assert_eq!(
                "https://images.example.test/sensitive.jpg",
                value["item"]["images"][0]["url"]
            );
            assert_eq!(LISTING_ID, value["item"]["productListingId"]);
            assert_eq!("merchant:42", value["item"]["sourceListingId"]);
            if matches!(name, "simpleSearchProductListings" | "getAuctionCatalogue") {
                if name == "simpleSearchProductListings" {
                    assert_eq!(1, output["total"]);
                    assert_eq!(1, output["size"]);
                } else {
                    assert_eq!(1, output["pageSize"]);
                }
                assert!(output.get("searchAfter").is_some());
            }
        }
        assert_eq!(
            if mask & 8 == 0 { 0 } else { 5 },
            fixture.destination_reads.load(Ordering::SeqCst)
        );
    }
}

#[tokio::test]
async fn should_keep_write_and_unrelated_grants_from_disclosing_private_state() {
    let fixture = ListingFixture::new(false, true);
    let app = authenticated_app(
        services(&fixture),
        fixture.user_id,
        vec![
            Scope::UsersWrite,
            Scope::WatchlistWrite,
            Scope::NotificationsWrite,
            Scope::SearchFiltersWrite,
            Scope::ProductListingsRead,
        ],
    );
    for (name, arguments) in listing_inputs() {
        let output = authenticated_call(&app, name, arguments).await;
        assert_private_fields(listing(name, &output), &BTreeSet::new());
    }
    assert_eq!(0, fixture.destination_reads.load(Ordering::SeqCst));
}

#[tokio::test]
async fn should_enforce_hidden_and_sensitive_image_policy_independently_of_disclosure_scopes() {
    for hidden in [false, true] {
        for consent in [false, true] {
            for mask in [0, 1, 2, 4, 8, 15] {
                let fixture = ListingFixture::new(hidden, consent);
                let (scopes, mut fields) = scoped_fields(mask);
                if hidden {
                    fields.remove("shipping");
                }
                let app = authenticated_app(services(&fixture), fixture.user_id, scopes);
                for (name, arguments) in listing_inputs() {
                    let output = authenticated_call(&app, name, arguments).await;
                    let value = listing(name, &output);
                    assert_private_fields(value, &fields);
                    let item = &value["item"];
                    if hidden {
                        assert_eq!(json!([]), item["images"]);
                        assert!(item["productListingTitleSlugId"].is_null());
                        assert!(item["shipping"].is_null());
                        assert_eq!("Hidden", item["source"]["name"]);
                        assert_eq!("https://aura-historia.com/pricing", item["url"]);
                        assert_eq!(
                            "00000000-0000-0000-0000-000000000000",
                            item["sourceListingId"]
                        );
                    } else {
                        assert_eq!(consent, item["images"][0]["url"].is_string());
                        assert_eq!("cabinet-abcdef", item["productListingTitleSlugId"]);
                    }
                    if fields.contains("searchFilter") {
                        assert_eq!(hidden, value["userState"]["searchFilter"]["hidden"]);
                    }
                    if fields.contains("contentVisibility") {
                        assert_eq!(
                            consent,
                            value["userState"]["contentVisibility"]["showUnassessedOrSensitiveContent"]
                        );
                    }
                }
                assert_eq!(
                    if hidden || mask & 8 == 0 { 0 } else { 5 },
                    fixture.destination_reads.load(Ordering::SeqCst)
                );
            }
        }
    }
}
