use application::patch_field::PatchField;
use aura_historia_contracts::{
    listing_sources::administered::{
        AdministeredListingSourcesPageData, AdministeredListingSourcesQuery,
    },
    product_listing_management::{SearchQuery, UpdateData, VersionData, version},
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use partnership_service::{
    ports::AdministeredListingSource,
    use_cases::queries::list_administered_listing_sources::ListAdministeredListingSourcesResult,
};
use product_listing_core::product_listing_id::ProductListingId;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde_json::json;

fn input_schema<T: JsonSchema>() -> jsonschema::Validator {
    let schema = SchemaSettings::default()
        .for_deserialize()
        .into_generator()
        .into_root_schema_for::<T>();
    jsonschema::validator_for(schema.as_value()).unwrap()
}
#[test]
fn should_describe_complete_management_patch_with_identical_semantic_acceptance() {
    let validator = input_schema::<UpdateData>();
    for (body, accepted) in [
        (json!({"expectedVersion":1}), true),
        (
            json!({"expectedVersion":i64::MAX,"price":{"type":"MONETARY","currency":"EUR","amount":i64::MAX},"priceEstimateMin":null,"priceEstimateMax":{"currency":"USD","amount":0},"availability":"SOLD_OUT","url":"https://example.test/new","images":[],"auction":{"auctionId":null,"lotNumber":null,"cataloguePosition":u32::MAX,"timing":{"biddingOpens":null,"scheduledCloses":"2026-10-23T12:00:00Z","reportedClosedAt":null}}}),
            true,
        ),
        (
            json!({"expectedVersion":1,"price":{"type":"ON_REQUEST"},"auction":{"timing":null}}),
            true,
        ),
        (
            json!({"expectedVersion":1,"price":null,"availability":null,"images":[]}),
            true,
        ),
        (json!({}), false),
        (json!({"expectedVersion":0}), false),
        (json!({"expectedVersion":u64::MAX}), false),
        (json!({"expectedVersion":1,"url":null}), false),
        (json!({"expectedVersion":1,"images":null}), false),
        (json!({"expectedVersion":1,"images":[null]}), false),
        (json!({"expectedVersion":1,"auction":null}), false),
        (json!({"expectedVersion":1,"title":"read only"}), false),
        (
            json!({"expectedVersion":1,"price":{"type":"ON_REQUEST","amount":2}}),
            false,
        ),
        (
            json!({"expectedVersion":1,"price":{"type":"MONETARY","currency":"EUR","amount":u64::MAX}}),
            false,
        ),
        (
            json!({"expectedVersion":1,"auction":{"cataloguePosition":0}}),
            false,
        ),
        (json!({"expectedVersion":1,"auction":{"other":true}}), false),
        (
            json!({"expectedVersion":1,"auction":{"timing":{"other":true}}}),
            false,
        ),
    ] {
        let decoded = serde_json::from_value::<UpdateData>(body.clone())
            .ok()
            .and_then(|input| input.into_command(ProductListingId::new()).ok())
            .is_some();
        assert_eq!(accepted, decoded, "validation: {body}");
        assert_eq!(accepted, validator.is_valid(&body), "schema: {body}");
    }
}
#[test]
fn should_keep_missing_null_value_and_empty_image_clear_distinct() {
    let input: UpdateData = serde_json::from_value(json!({"expectedVersion":i64::MAX,"price":null,"priceEstimateMin":{"currency":"EUR","amount":42},"images":[],"auction":{"auctionId":null,"lotNumber":"42","cataloguePosition":null,"timing":{"biddingOpens":null}}})).unwrap();
    let command = input.into_command(ProductListingId::new()).unwrap();
    assert_eq!(i64::MAX as u64, u64::from(command.expected_version));
    assert!(matches!(command.patch.price, PatchField::Clear));
    assert!(matches!(
        command.patch.price_estimate_min,
        PatchField::Set(_)
    ));
    assert!(matches!(
        command.patch.price_estimate_max,
        PatchField::Unchanged
    ));
    assert!(matches!(command.patch.availability, PatchField::Unchanged));
    assert!(matches!(command.patch.url, PatchField::Unchanged));
    assert!(matches!(command.patch.images, PatchField::Set(ref values) if values.is_empty()));
    let PatchField::Set(auction) = command.patch.auction else {
        panic!("auction patch")
    };
    assert!(matches!(auction.auction_id, PatchField::Clear));
    assert!(matches!(auction.lot_number, PatchField::Set(_)));
    assert!(matches!(auction.catalogue_position, PatchField::Clear));
    assert!(matches!(auction.bidding_opens, PatchField::Clear));
    assert!(matches!(auction.scheduled_closes, PatchField::Unchanged));
}
#[test]
fn should_retain_full_listing_query_and_bind_continuation_to_the_source() {
    let source = ListingSourceId::new();
    let query: SearchQuery = serde_json::from_value(json!({"query":" VASE ","sourceListingId":" SKU-42 ","lifecycle":"WITHDRAWN","availability":"RESERVED","auctionId":auction_core::AuctionId::new(),"sort":"created","order":"asc","size":100})).unwrap();
    let request = query.into_partner_request(source).unwrap();
    assert_eq!(Some(source), request.scope.listing_source_id);
    assert_eq!(Some("vase"), request.scope.query.as_deref());
    assert_eq!(
        Some("SKU-42"),
        request.scope.source_listing_id.as_ref().map(|v| v.as_ref())
    );
    assert_eq!(100, request.cursor.size);
    for value in [0, u64::MAX] {
        assert!(version(value).is_err());
    }
    let validator = input_schema::<VersionData>();
    assert!(validator.is_valid(&json!({"expectedVersion":i64::MAX})));
    assert!(!validator.is_valid(&json!({"expectedVersion":u64::MAX})));
}
#[test]
fn should_preserve_complete_granted_source_page_and_omit_terminal_next() {
    let id = ListingSourceId::new();
    let page = AdministeredListingSourcesPageData::from(ListAdministeredListingSourcesResult {
        items: vec![AdministeredListingSource {
            listing_source_id: id,
            slug_id: ListingSourceSlugId::raw("complete-source").unwrap(),
            name: ListingSourceName::try_from("Complete Source").unwrap(),
        }],
        next: None,
    });
    let actual = serde_json::to_value(page).unwrap();
    assert_eq!(
        json!({"items":[{"listingSourceId":id,"listingSourceSlugId":"complete-source","name":"Complete Source"}]}),
        actual
    );
    let schema = SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<AdministeredListingSourcesPageData>();
    assert!(
        jsonschema::validator_for(schema.as_value())
            .unwrap()
            .is_valid(&actual)
    );
    for (value, allowed) in [
        (json!({}), true),
        (json!({"size":1}), true),
        (json!({"size":100}), true),
        (json!({"size":0}), false),
        (json!({"size":101}), false),
        (json!({"userId":"spoof"}), false),
    ] {
        let result = serde_json::from_value::<AdministeredListingSourcesQuery>(value.clone())
            .ok()
            .and_then(|input| input.into_request().ok());
        assert_eq!(allowed, result.is_some(), "{value}");
        assert_eq!(
            allowed,
            input_schema::<AdministeredListingSourcesQuery>().is_valid(&value),
            "{value}"
        );
    }
}
