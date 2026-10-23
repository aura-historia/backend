use super::{
    CreateProductListingData, UpdateProductListingData, UpsertProductListingData,
    WithdrawProductListingData, source_listing_id,
};
use application::patch_field::PatchField;
use listing_source_core::ListingSourceId;

#[test]
fn should_reject_null_auction_in_partner_writes() {
    let listing_source_id = ListingSourceId::new();
    let create: CreateProductListingData = serde_json::from_str(
        r#"{
                "sourceListingId":"SKU-1",
                "title":{"text":"Listing","language":"en"},
                "description":{"text":"Description","language":"en"},
                "url":"https://example.com/listing",
                "images":[],
                "auction":null
            }"#,
    )
    .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
    let update: UpdateProductListingData =
        serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
            .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
    let upsert: UpsertProductListingData =
        serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":null}"#)
            .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

    assert_eq!(
        "auction",
        create
            .into_command(listing_source_id)
            .err()
            .unwrap_or_else(|| panic!("null auction must fail"))
            .field
    );
    assert_eq!(
        "auction",
        update
            .into_key_and_command(listing_source_id)
            .err()
            .unwrap_or_else(|| panic!("null auction must fail"))
            .field
    );
    assert_eq!(
        "auction",
        upsert
            .into_command(listing_source_id)
            .err()
            .unwrap_or_else(|| panic!("null auction must fail"))
            .field
    );
}

#[test]
fn should_allow_optional_title_and_description_on_create() {
    let listing_source_id = ListingSourceId::new();
    let cases = [
        (
            r#"{"sourceListingId":"SKU-1","url":"https://example.com/listing","images":[]}"#,
            false,
            false,
        ),
        (
            r#"{"sourceListingId":"SKU-2","title":{"text":"Title","language":"en"},"url":"https://example.com/listing","images":[]}"#,
            true,
            false,
        ),
        (
            r#"{"sourceListingId":"SKU-3","description":{"text":"Description","language":"en"},"url":"https://example.com/listing","images":[]}"#,
            false,
            true,
        ),
        (
            r#"{"sourceListingId":"SKU-4","title":null,"description":null,"url":"https://example.com/listing","images":[]}"#,
            false,
            false,
        ),
    ];

    for (body, expects_title, expects_description) in cases {
        let data: CreateProductListingData =
            serde_json::from_str(body).unwrap_or_else(|error| panic!("valid create JSON: {error}"));
        let command = data
            .into_command(listing_source_id)
            .unwrap_or_else(|error| panic!("valid create command: {error}"));
        assert_eq!(expects_title, command.title.is_some());
        assert_eq!(expects_description, command.description.is_some());
    }
}

#[test]
fn should_normalize_decorative_titles_on_create_and_upsert() {
    let listing_source_id = ListingSourceId::new();
    let create: CreateProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","title":{"text":"𝐀𝐧𝐫𝐢𝐜𝐡𝐭𝐞 🌿✨Kirschbaum | 3360","language":"de"},"url":"https://example.com/listing","images":[]}"#,
        )
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
    let upsert: UpsertProductListingData = serde_json::from_str(
            r#"{"sourceListingId":"SKU-1","title":{"text":"𝐀𝐧𝐫𝐢𝐜𝐡𝐭𝐞 🌿✨Kirschbaum | 3360","language":"de"}}"#,
        )
        .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

    for title in [
        create
            .into_command(listing_source_id)
            .unwrap_or_else(|error| panic!("valid create command: {error}"))
            .title,
        upsert
            .into_command(listing_source_id)
            .unwrap_or_else(|error| panic!("valid upsert command: {error}"))
            .title,
    ] {
        assert_eq!(
            Some("Anrichte Kirschbaum | 3360"),
            title.as_ref().map(|title| title.payload.as_ref())
        );
    }
}

#[test]
fn should_reject_titles_without_text_on_create_and_upsert() {
    let listing_source_id = ListingSourceId::new();
    for text in ["", "   ", "🌿✨", " ★ ✿ "] {
        let title = serde_json::json!({"text": text, "language": "en"});
        let create: CreateProductListingData = serde_json::from_value(serde_json::json!({
            "sourceListingId": "SKU-1",
            "title": title,
            "url": "https://example.com/listing",
            "images": []
        }))
        .unwrap_or_else(|error| panic!("valid create JSON: {error}"));
        let upsert: UpsertProductListingData = serde_json::from_value(serde_json::json!({
            "sourceListingId": "SKU-1",
            "title": title
        }))
        .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

        for error in [
            create.into_command(listing_source_id).err(),
            upsert.into_command(listing_source_id).err(),
        ] {
            let error = error.unwrap_or_else(|| panic!("title {text:?} must fail"));
            assert_eq!("title.text", error.field);
        }
    }
}

#[test]
fn should_map_existing_auction_id_and_listing_owned_leaf_patches() {
    let auction_id = auction_core::AuctionId::new();
    let data: UpdateProductListingData = serde_json::from_str(&format!(
            r#"{{"sourceListingId":"SKU-1","auction":{{"auctionId":"{auction_id}","lotNumber":null,"timing":{{"biddingOpens":null,"reportedClosedAt":"2026-05-01T12:00:00Z"}}}}}}"#
        ))
        .unwrap_or_else(|error| panic!("valid update JSON: {error}"));

    let (_, command) = data
        .into_key_and_command(ListingSourceId::new())
        .unwrap_or_else(|error| panic!("valid update command: {error}"));
    let PatchField::Set(auction) = command.auction else {
        panic!("auction patch should be asserted");
    };
    assert_eq!(PatchField::Set(auction_id), auction.auction_id);
    assert_eq!(PatchField::Clear, auction.lot_number);
    assert_eq!(PatchField::Clear, auction.bidding_opens);
    assert!(matches!(auction.reported_closed_at, PatchField::Set(_)));
}

#[test]
fn should_reject_date_only_and_timezone_less_lot_timestamps() {
    for timestamp in ["2026-05-01", "2026-05-01T12:00:00"] {
        assert!(serde_json::from_str::<UpdateProductListingData>(&format!(
                r#"{{"sourceListingId":"SKU-1","auction":{{"timing":{{"biddingOpens":"{timestamp}"}}}}}}"#
            ))
            .is_err());
    }
}

#[test]
fn should_clear_membership_with_null_auction_id_and_reject_retired_fields() {
    let update: UpdateProductListingData =
        serde_json::from_str(r#"{"sourceListingId":"SKU-1","auction":{"auctionId":null}}"#)
            .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
    let (_, command) = update
        .into_key_and_command(ListingSourceId::new())
        .unwrap_or_else(|error| panic!("valid update command: {error}"));
    let PatchField::Set(auction) = command.auction else {
        panic!("auction patch should be asserted");
    };
    assert_eq!(PatchField::Clear, auction.auction_id);

    assert!(
        serde_json::from_str::<UpdateProductListingData>(
            r#"{"sourceListingId":"SKU-1","auction":{"sourceAuctionId":"sale-42"}}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<UpdateProductListingData>(
            r#"{"sourceListingId":"SKU-1","auction":{"metadata":{"name":"sale"}}}"#
        )
        .is_err()
    );
}

#[test]
fn should_leave_auction_unchanged_when_partner_update_or_upsert_omits_it() {
    let update: UpdateProductListingData = serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
        .unwrap_or_else(|error| panic!("valid update JSON: {error}"));
    let upsert: UpsertProductListingData = serde_json::from_str(r#"{"sourceListingId":"SKU-1"}"#)
        .unwrap_or_else(|error| panic!("valid upsert JSON: {error}"));

    let (_, update) = update
        .into_key_and_command(ListingSourceId::new())
        .unwrap_or_else(|error| panic!("valid update command: {error}"));
    let upsert = upsert
        .into_command(ListingSourceId::new())
        .unwrap_or_else(|error| panic!("valid upsert command: {error}"));

    assert_eq!(update.auction, PatchField::Unchanged);
    assert_eq!(upsert.auction, PatchField::Unchanged);
}

#[test]
fn should_parse_source_listing_id_without_slugifying_it() {
    let data: WithdrawProductListingData =
        serde_json::from_str(r#"{"sourceListingId":"\u2003SKU  #42/Blue\u2002"}"#)
            .unwrap_or_else(|error| panic!("valid request data: {error}"));

    let key = data
        .into_product_key(ListingSourceId::new())
        .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));

    assert_eq!(key.source_listing_id.as_ref(), "SKU  #42/Blue");
}

#[test]
fn should_reject_blank_source_listing_id_at_api_mapping() {
    let error = source_listing_id("\u{2003}\t".to_owned())
        .err()
        .unwrap_or_else(|| panic!("blank source listing ID must fail"));

    assert_eq!(error.field, "sourceListingId");
}

#[test]
fn should_describe_exact_unsigned_batch_report_values_in_serialization_schema() {
    use super::results::{
        ProductListingBatchFailureCode, ProductListingBatchFailureData,
        ProductListingBatchReportData, ProductListingBatchStatus,
    };
    let source = listing_source_core::ListingSourceId::new();
    let report = ProductListingBatchReportData {
        status: ProductListingBatchStatus::Failed,
        input_count: usize::MAX,
        succeeded_count: 0,
        failed_count: usize::MAX,
        successes: vec![],
        failures: vec![ProductListingBatchFailureData {
            index: usize::MAX,
            listing_source_id: source,
            source_listing_id: "retained".into(),
            error: ProductListingBatchFailureCode::Forbidden,
        }],
    };
    let value = serde_json::to_value(report).unwrap();
    let schema = serde_json::to_value(
        schemars::generate::SchemaSettings::default()
            .for_serialize()
            .into_generator()
            .into_root_schema_for::<ProductListingBatchReportData>(),
    )
    .unwrap();
    let validator = jsonschema::validator_for(&schema).unwrap();
    assert!(validator.is_valid(&value));
    assert_eq!(
        serde_json::json!(u64::MAX),
        schema["$defs"]["ProductListingBatchFailureData"]["properties"]["index"]["maximum"]
    );
    let mut invalid = value.clone();
    invalid["failures"][0]["index"] = serde_json::from_str("18446744073709551616").unwrap();
    assert!(!validator.is_valid(&invalid));
    invalid["failures"][0]["index"] = serde_json::json!(-1);
    assert!(!validator.is_valid(&invalid));
}
