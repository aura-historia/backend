use super::support::*;
use super::{SCHEMA, aura_integration_test};
use listing_source_core::ListingSourceId;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use test_api::IntegrationTestService;

struct Workflow<'a> {
    client: &'a Client,
    tools: BTreeMap<String, Value>,
    called: BTreeSet<String>,
}
impl Workflow<'_> {
    async fn call(&mut self, name: &str, input: Value) -> Value {
        self.called.insert(name.to_owned());
        invoke(self.client, &self.tools, name, input).await
    }
}
#[aura_integration_test(services=[SCHEMA], isolated=true)]
async fn should_complete_every_partner_operation_through_production_http_and_official_sdk() {
    let pool = test_api::get_postgres_client().await;
    let owner = actor(&pool, SCOPES).await;
    let source = source(&pool, &owner, "A gallery").await;
    let other = super::support::source(&pool, &owner, "B gallery").await;
    let server = Server::start(pool).await;
    let client = server.client(Some(&owner.token), discover()).await;
    let mut flow = Workflow {
        tools: catalogue(&client).await,
        client: &client,
        called: BTreeSet::new(),
    };
    sources(&mut flow, &source, &other).await;
    let id = listings(&mut flow, source.source).await;
    ingestion(&mut flow, &server, source.source).await;
    let site = locations(&mut flow, &source).await;
    let profile = shipping(&mut flow, &source, &site).await;
    delivery_workflow(&mut flow, &server, source.source, &id, &profile).await;
    cleanup_defaults(&mut flow, &source, site).await;
    assert_eq!(partner_names(), flow.called);
    // Earlier slices remain usable in the same production composition.
    invoke(
        &client,
        &flow.tools,
        "searchPublicListingSources",
        json!({"size":1}),
    )
    .await;
    invoke(&client, &flow.tools, "getUserAccount", json!({})).await;
    client.cancel().await.unwrap();
}
async fn sources(flow: &mut Workflow<'_>, source: &Source, other: &Source) {
    let first = flow.call("getMyListingSources", json!({"size":1})).await;
    assert_eq!(json!(source.source), first["items"][0]["listingSourceId"]);
    let second = flow
        .call(
            "getMyListingSources",
            json!({"size":1,"after":first["next"]}),
        )
        .await;
    assert_eq!(json!(other.source), second["items"][0]["listingSourceId"]);
    assert!(second.get("next").is_none());
}
async fn listings(flow: &mut Workflow<'_>, source: ListingSourceId) -> Value {
    let report = flow
        .call(
            "postPartnerProductListings",
            batch(
                source,
                vec![product("painting"), product("painting"), product("spare")],
            ),
        )
        .await;
    assert_eq!("PARTIAL_SUCCESS", report["status"]);
    assert_eq!(2, report["succeededCount"]);
    assert_eq!(1, report["failures"][0]["index"]);
    let id = report["successes"][0]["productListingId"].clone();
    let page = flow
        .call(
            "listPartnerProductListings",
            json!({"listingSourceId":source,"size":1}),
        )
        .await;
    assert_eq!(1, page["items"].as_array().unwrap().len());
    let rest = flow
        .call(
            "listPartnerProductListings",
            json!({"listingSourceId":source,"size":1,"searchAfter":page["searchAfter"]}),
        )
        .await;
    assert_eq!(1, rest["items"].as_array().unwrap().len());
    let details = flow
        .call("getPartnerProductListing", listing(source, &id))
        .await;
    assert_eq!("painting", details["sourceListingId"]);
    let mut patch = listing(source, &id);
    patch["expectedVersion"] = details["expectedVersion"].clone();
    patch["availability"] = json!("AVAILABLE");
    let updated = flow.call("updatePartnerProductListing", patch).await;
    assert_eq!("CHANGED", updated["outcome"]);
    let mut lifecycle = listing(source, &id);
    lifecycle["expectedVersion"] = updated["expectedVersion"].clone();
    let withdrawn = flow
        .call("withdrawPartnerProductListing", lifecycle.clone())
        .await;
    lifecycle["expectedVersion"] = withdrawn["expectedVersion"].clone();
    flow.call("restorePartnerProductListing", lifecycle).await;
    for (name, products) in [
        (
            "patchPartnerProductListings",
            vec![json!({"sourceListingId":"painting","availability":"RESERVED"})],
        ),
        (
            "deletePartnerProductListings",
            vec![json!({"sourceListingId":"spare"})],
        ),
        ("putPartnerProductListings", vec![product("spare")]),
    ] {
        assert_eq!(
            "COMPLETED",
            flow.call(name, batch(source, products)).await["status"]
        );
    }
    id
}
async fn ingestion(flow: &mut Workflow<'_>, server: &Server, source: ListingSourceId) {
    let configured=flow.call("putShopifyListingSourceIngestionConfiguration",json!({"listingSourceId":source,"configuration":{"domain":"gallery.example.test","language":"de","currency":"EUR"}})).await;
    assert_eq!(true, configured["created"]);
    for (name, item) in [
        ("postAsyncPartnerProductListings", product("queued")),
        (
            "patchAsyncPartnerProductListings",
            json!({"sourceListingId":"queued","availability":"AVAILABLE"}),
        ),
        (
            "deleteAsyncPartnerProductListings",
            json!({"sourceListingId":"queued"}),
        ),
        ("putAsyncPartnerProductListings", product("queued")),
    ] {
        let mut input = batch(source, vec![item]);
        input["idempotencyKey"] = json!(name);
        let report = flow.call(name, input).await;
        assert_eq!(1, report["acceptedCount"]);
        assert_eq!("PUBLISHED", report["items"][0]["outcome"]);
        assert!(report.get("jobStatus").is_none());
        let message = server.publisher.last().remove(0);
        assert!(matches!(
            server.apply(message.clone()).await.unwrap(),
            product_listing_service::use_cases::ProductListingIngestionCompletion::Applied(_)
        ));
        assert_eq!(
            product_listing_service::use_cases::ProductListingIngestionCompletion::AlreadyCompleted,
            server.apply(message).await.unwrap()
        );
    }
}
async fn locations(flow: &mut Workflow<'_>, source: &Source) -> Value {
    let site=flow.call("createPartyLocation",json!({"partyId":source.party,"label":"Gallery","roles":["BUSINESS_PREMISES","WAREHOUSE"],"geography":{"referenceRelease":"iso-codes-4.20.1","country":"DE","postalCode":"01234"},"position":{"latitudeDegrees":48.137,"longitudeDegrees":11.575,"accuracyMetres":1.5,"precision":"PREMISES"},"evidence":{"reference":"Private observation","scope":"SITE_DESCRIPTION"}})).await;
    assert_eq!(48.137, site["position"]["latitudeDegrees"]);
    assert_eq!("01234", site["geography"]["postalCode"]);
    let list = flow
        .call("listPartyLocations", json!({"partyId":source.party}))
        .await;
    assert_eq!(site, list["items"][0]);
    flow.call(
        "getPartyLocation",
        json!({"partyId":source.party,"locationId":site["id"]}),
    )
    .await;
    let corrected=flow.call("correctPartyLocation",json!({"partyId":source.party,"locationId":site["id"],"expectedRevision":1,"correction":"SAME_SITE","label":"Main gallery"})).await;
    assert_eq!(2, corrected["revision"]);
    let default = flow
        .call(
            "getSourceLocationDefault",
            json!({"listingSourceId":source.source}),
        )
        .await;
    let assigned=flow.call("assignSourceLocationDefault",json!({"listingSourceId":source.source,"expectedRevision":default["revision"],"partyLocationId":site["id"],"businessAssociation":{"reference":"Business evidence","scope":"ASSOCIATED_BUSINESS"}})).await;
    let changed=flow.call("changeSourceLocationDefaultApplicability",json!({"listingSourceId":source.source,"expectedRevision":assigned["revision"],"businessAssociation":null,"advertisedInventory":{"reference":"Stock evidence","scope":"ADVERTISED_INVENTORY"}})).await;
    assert!(changed["assignment"].get("businessAssociation").is_none());
    site["id"].clone()
}
async fn shipping(flow: &mut Workflow<'_>, source: &Source, site: &Value) -> Value {
    let party = source.party;
    let source = source.source;
    let mut terms = aura_historia_contracts::shipping::terms::encode(
        &shipping_core::fixtures::gallery_terms(8000),
    )
    .unwrap();
    terms["collection"]["site"] = json!({"kind":"PARTY_LOCATION","party_location_id":site});
    terms["shipping"]["coverage"]["dispatch_origin"] =
        json!({"kind":"PARTY_LOCATION","party_location_id":site});
    let content = json!({"name":"Gallery delivery","attributedPartyId":party,"terms":terms});
    let creation = json!({"listingSourceId":source,"creationKey":"gallery-profile","content":content,"evidence":[{"reference":"https://gallery.example.test/terms","assertion":"ADVERTISED"}]});
    let profile = flow.call("createShippingProfile", creation.clone()).await;
    assert_eq!(profile, flow.call("createShippingProfile", creation).await);
    assert_eq!(
        site,
        &profile["content"]["terms"]["collection"]["site"]["party_location_id"]
    );
    let id = profile["shippingProfileId"].clone();
    flow.call(
        "getShippingProfile",
        json!({"listingSourceId":source,"shippingProfileId":id}),
    )
    .await;
    let list = flow
        .call(
            "listShippingProfiles",
            json!({"listingSourceId":source,"size":100}),
        )
        .await;
    assert_eq!(1, list["result"].as_array().unwrap().len());
    let replacement=flow.call("replaceShippingProfile",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":1,"content":content,"evidence":[]})).await;
    let retired=flow.call("retireShippingProfile",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":replacement["managementRevision"]})).await;
    flow.call("restoreShippingProfile",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":retired["managementRevision"]})).await;
    let default = flow
        .call("getShippingDefault", json!({"listingSourceId":source}))
        .await;
    let assigned=flow.call("assignShippingDefault",json!({"listingSourceId":source,"shippingProfileId":id,"expectedRevision":default["revision"]})).await;
    assert_eq!("ACTIVE", assigned["eligibility"]);
    id
}
async fn delivery_workflow(
    flow: &mut Workflow<'_>,
    server: &Server,
    source: ListingSourceId,
    id: &Value,
    profile: &Value,
) {
    let query = json!({"listingSourceId":source,"sourceListingId":"painting"});
    let current = flow.call("getProductListingDelivery", query.clone()).await;
    assert_eq!(id, &current["productListingId"]);
    let mut command = delivery(
        source,
        "painting",
        current["expectedVersion"].as_u64().unwrap(),
    );
    command["delivery"]["value"]["selection"] =
        json!({"state":"SET","value":{"kind":"EXPLICIT","profile_id":profile}});
    let changed = flow.call("changeProductListingDelivery", command).await;
    assert_eq!("CHANGED", changed["outcome"]);
    let selected = flow.call("getProductListingDelivery", query.clone()).await;
    assert_eq!(profile, &selected["delivery"]["selection"]["profile_id"]);
    let mut command = delivery(
        source,
        "painting",
        changed["expectedVersion"].as_u64().unwrap(),
    );
    command["idempotencyKey"] = json!("delivery-submission");
    command["delivery"]["value"]["shipping"] = json!({"state":"CLEAR"});
    let admitted = flow.call("submitProductListingDelivery", command).await;
    assert_eq!(1, admitted["originalInputCount"]);
    assert_eq!(0, admitted["items"][0]["index"]);
    assert_eq!(
        selected,
        flow.call("getProductListingDelivery", query.clone()).await
    );
    server
        .apply(server.publisher.last().remove(0))
        .await
        .unwrap();
    let applied = flow.call("getProductListingDelivery", query).await;
    assert_eq!(
        changed["expectedVersion"].as_u64().unwrap() + 1,
        applied["expectedVersion"]
    );
}
async fn cleanup_defaults(flow: &mut Workflow<'_>, source: &Source, site: Value) {
    let default = flow
        .call(
            "getShippingDefault",
            json!({"listingSourceId":source.source}),
        )
        .await;
    let cleared = flow
        .call(
            "clearShippingDefault",
            json!({"listingSourceId":source.source,"expectedRevision":default["revision"]}),
        )
        .await;
    assert!(cleared["shippingProfileId"].is_null());
    let default = flow
        .call(
            "getSourceLocationDefault",
            json!({"listingSourceId":source.source}),
        )
        .await;
    flow.call(
        "clearSourceLocationDefault",
        json!({"listingSourceId":source.source,"expectedRevision":default["revision"]}),
    )
    .await;
    flow.call("setPartyLocationLifecycle",json!({"partyId":source.party,"locationId":site,"expectedRevision":2,"lifecycle":"RETIRED"})).await;
}
