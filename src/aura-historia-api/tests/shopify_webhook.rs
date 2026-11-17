//! Shopify `product_listings/*` deliveries over `POST /v1/webhooks/shopify`, composed with the real
//! Shopify use case, the FIFO wire codec, the ingestion consumer and normalization against
//! PostgreSQL. Only the FIFO publisher is replaced, so admission custody can be observed.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use aura_historia_api::{
    app,
    state::{AppState, ShopifyWebhooksState},
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use base64::Engine;
use listing_source_core::{Domain, ListingSourceId};
use listing_source_postgres::SqlxListingSourceReaders;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use platform_postgres::SqlxUnitOfWork;
use product_listing_ingestion_sqs::codec;
use product_listing_normalization::SourcePayload;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxPendingProductListingRawStreamReader,
    SqlxProductListingCommandReceiptStoreFactory, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRawCaptureWriterFactory, SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::{
    ports::{ProductListingIngestionPublishError, ProductListingIngestionPublisher},
    use_cases::{
        ProcessProductListingIngestionHandler, ProcessProductListingIngestionUseCase,
        ProductListingIngestionCompletion, ProductListingIngestionEffect,
        ProductListingIngestionError, ProductListingIngestionIntent,
        ProductListingIngestionItemOutcome, ProductListingIngestionMessage,
        ProductListingIngestionOperation, ProductListingIngestionOutcome,
        SubmitInternalProductListingIngestionHandler,
        commands::process_shopify_product_listing::ProcessShopifyProductListingHandler,
    },
};
use product_listing_shopify::{
    SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD, SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE,
    SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE, ShopifyHmacWebhookVerifier,
    ShopifyProductPayloadDecoder, ShopifyWebhookSecrets,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionHandler,
    NormalizeProductListingRawRevisionMode, NormalizeProductListingRawRevisionUseCase,
};
use sqlx::types::Json;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tower::ServiceExt;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const CLIENT_SECRET: &str = "shopify-integration-client-secret";
const DEFAULT_TRIGGERED_AT: &str = "2026-01-01T00:00:00Z";

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_capture_shopify_raw_revision_without_direct_canonical_write() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;

    let status = harness
        .send(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            source.domain.as_str(),
            100,
            5,
            "shopify-event-1",
            "webhook-1",
            serde_json::json!({"futureShopifyKey": {"retained": true}}),
        ))
        .await;

    assert_eq!(StatusCode::OK, status);
    assert_eq!(0, raw_revision_count(source.id, 100).await);
    let wires = harness.published();
    assert_eq!(1, wires.len());
    let (command_id, command) = capture_command(&wires[0]);
    assert!(command_id.starts_with("plic1_"));
    assert_eq!(source.id, command.listing_source_id);
    assert_eq!("100", command.source_record_key.as_str());
    assert_eq!(Some("shopify-event-1"), command.source_event_id.as_deref());
    assert!(matches!(
        harness.process_pending().await.as_slice(),
        [Ok(ProductListingIngestionCompletion::Applied(
            ProductListingIngestionEffect::RawCaptured(_)
        ))]
    ));
    assert_eq!(1, raw_revision_count(source.id, 100).await);
    assert_eq!(0, listing_count(source.id).await);
    assert_eq!(0, product_listing_event_count().await);

    let revision = raw_revision(source.id, 100, 1).await;
    assert_eq!("UPSERT", revision.operation);
    assert_eq!("SHOPIFY_PRODUCT", revision.payload_format);
    assert_eq!(1, revision.raw_values_schema_version);
    assert_eq!(
        serde_json::json!("MACHINE_DECIMAL"),
        revision.raw_values.0["priceFormat"]
    );
    assert_eq!(
        serde_json::json!(true),
        revision.source_payload.0["product_listing"]["futureShopifyKey"]["retained"]
    );
    assert_eq!(
        None,
        revision.source_payload.0["product_listing"].get("body_html")
    );
    assert_eq!(
        serde_json::json!({"action": "UNCHANGED"}),
        revision.raw_values.0["description"]
    );
    assert_eq!(
        serde_json::json!({"action": "SET", "value": "in stock"}),
        revision.raw_values.0["availability"]
    );
    assert_eq!(
        serde_json::json!("USD"),
        revision.normalization_context.0["fallbackCurrency"]
    );
    assert_eq!(
        serde_json::json!("de"),
        revision.normalization_context.0["fallbackLanguage"]
    );
    assert_eq!(
        serde_json::json!("shopify-event-1"),
        revision.provenance.0["shopifyEventId"]
    );
    assert_eq!(
        serde_json::json!("webhook-1"),
        revision.provenance.0["shopifyWebhookId"]
    );
    assert_eq!(
        serde_json::Value::Null,
        revision.provenance.0["eventBridgeEventId"]
    );
    assert_eq!(Some("shopify-event-1".to_owned()), revision.source_event_id);
    assert_eq!(
        Some(occurred_at(DEFAULT_TRIGGERED_AT)),
        revision.source_occurred_at
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_collapse_equal_shopify_state_with_new_delivery_ids() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;

    let first = harness
        .send_and_process(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            source.domain.as_str(),
            101,
            5,
            "shopify-event-1",
            "webhook-1",
            serde_json::json!({}),
        ))
        .await;
    let second = harness
        .send_and_process(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            source.domain.as_str(),
            101,
            5,
            "shopify-event-2",
            "webhook-2",
            serde_json::json!({}),
        ))
        .await;

    assert_eq!((StatusCode::OK, StatusCode::OK), (first, second));
    assert_eq!(1, raw_revision_count(source.id, 101).await);
    assert_eq!(0, listing_count(source.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_preserve_e2_b_after_retrying_e1_a_and_capture_e3_a() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let domain = source.domain.as_str();
    let e1_a = shopify_payload_with_updated_at(
        107,
        5,
        "2026-01-01T00:00:01Z",
        serde_json::json!({"state": "A"}),
    );
    let e2_b = shopify_payload_with_updated_at(
        107,
        0,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "B"}),
    );

    let first = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            domain,
            e1_a.clone(),
            "shopify-event-e1",
            "webhook-shared",
            Some("2026-01-01T00:00:01Z"),
        ))
        .await;
    let second = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            domain,
            e2_b.clone(),
            "shopify-event-e2",
            "webhook-shared",
            Some("2026-01-01T00:00:02Z"),
        ))
        .await;

    assert_eq!((StatusCode::OK, StatusCode::OK), (first, second));
    assert_eq!(2, raw_revision_count(source.id, 107).await);
    assert_eq!(
        vec![
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD.to_owned(),
                delivery_id: "shopify-webhook:webhook-shared".to_owned(),
                source_payload_sha256: source_payload_digest(&e1_a),
            },
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-shared".to_owned(),
                source_payload_sha256: source_payload_digest(&e2_b),
            },
        ],
        provider_receipts(source.id, 107).await
    );

    let first_revision = raw_revision(source.id, 107, 1).await;
    assert_eq!(
        Some("shopify-event-e1".to_owned()),
        first_revision.source_event_id
    );
    assert_eq!(
        Some(occurred_at("2026-01-01T00:00:01Z")),
        first_revision.source_occurred_at
    );
    assert_eq!(
        serde_json::json!("webhook-shared"),
        first_revision.provenance.0["shopifyWebhookId"]
    );
    let second_revision = raw_revision(source.id, 107, 2).await;
    assert_eq!(
        Some("shopify-event-e2".to_owned()),
        second_revision.source_event_id
    );
    assert_eq!(
        Some(occurred_at("2026-01-01T00:00:02Z")),
        second_revision.source_occurred_at
    );

    assert_eq!(
        2,
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    let listing = listing_facts(source.id, 107).await;
    assert_eq!(Some("OUT_OF_STOCK".to_owned()), listing.0);

    let retry = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            domain,
            e1_a.clone(),
            "shopify-event-e1",
            "webhook-shared",
            Some("2026-01-01T00:00:01Z"),
        ))
        .await;

    assert_eq!(StatusCode::OK, retry);
    assert_eq!(2, raw_revision_count(source.id, 107).await);
    assert_eq!(2, provider_receipts(source.id, 107).await.len());
    assert_eq!(
        0,
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    let listing = listing_facts(source.id, 107).await;
    assert_eq!(Some("OUT_OF_STOCK".to_owned()), listing.0);

    let e3_a = shopify_payload_with_updated_at(
        107,
        5,
        "2026-01-01T00:00:03Z",
        serde_json::json!({"state": "A"}),
    );
    let third = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            domain,
            e3_a.clone(),
            "shopify-event-e3",
            "webhook-e3",
            Some("2026-01-01T00:00:03Z"),
        ))
        .await;

    assert_eq!(StatusCode::OK, third);
    assert_eq!(3, raw_revision_count(source.id, 107).await);
    assert_eq!(
        Some("shopify-event-e3".to_owned()),
        raw_revision(source.id, 107, 3).await.source_event_id
    );
    assert!(
        provider_receipts(source.id, 107)
            .await
            .iter()
            .any(|receipt| {
                receipt.scope == SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE
                    && receipt.delivery_id == "shopify-webhook:webhook-e3"
                    && receipt.source_payload_sha256 == source_payload_digest(&e3_a)
            })
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_acknowledge_fifo_admission_but_reject_conflicting_delivery_fingerprint() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let domain = source.domain.as_str();
    let e1_a = shopify_payload_with_updated_at(
        108,
        5,
        "2026-01-01T00:00:01Z",
        serde_json::json!({"state": "A"}),
    );
    let conflicting_b = shopify_payload_with_updated_at(
        108,
        0,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "B"}),
    );

    let first = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            domain,
            e1_a.clone(),
            "shopify-event-e1",
            "webhook-e1",
            Some("2026-01-01T00:00:01Z"),
        ))
        .await;
    let conflict = harness
        .send(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            domain,
            conflicting_b,
            "shopify-event-e1-conflict",
            "webhook-e1",
            Some("2026-01-01T00:00:02Z"),
        ))
        .await;

    assert_eq!(StatusCode::OK, first);
    assert_eq!(StatusCode::OK, conflict); // Upstream custody transferred to FIFO.
    assert_eq!(1, raw_revision_count(source.id, 108).await);
    assert!(matches!(
        harness.process_pending().await.as_slice(),
        [Err(ProductListingIngestionError::FingerprintConflict)]
    ));
    assert_eq!(1, raw_revision_count(source.id, 108).await);
    assert_eq!(
        vec![ProviderReceiptRow {
            scope: SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD.to_owned(),
            delivery_id: "shopify-webhook:webhook-e1".to_owned(),
            source_payload_sha256: source_payload_digest(&e1_a),
        }],
        provider_receipts(source.id, 108).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_apply_shopify_trigger_timestamp_ordering_with_equal_times_in_arrival_order() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let domain = source.domain.as_str();
    let newer_a = shopify_payload_with_updated_at(
        109,
        5,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "A"}),
    );
    let older_b = shopify_payload_with_updated_at(
        109,
        0,
        "2026-01-01T00:00:01Z",
        serde_json::json!({"state": "B"}),
    );
    let same_time_c = shopify_payload_with_updated_at(
        109,
        2,
        "2026-01-01T00:00:02Z",
        serde_json::json!({"state": "C"}),
    );

    let newer = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            domain,
            newer_a.clone(),
            "shopify-event-newer",
            "webhook-newer",
            Some("2026-01-01T00:00:02Z"),
        ))
        .await;
    let stale = harness
        .send_and_process(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            domain,
            older_b.clone(),
            "shopify-event-older",
            "webhook-older",
            Some("2026-01-01T00:00:01Z"),
        ))
        .await;
    let same_time = harness
        .send(delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            domain,
            same_time_c.clone(),
            "shopify-event-same-time",
            "webhook-same-time",
            Some("2026-01-01T00:00:02Z"),
        ))
        .await;

    assert_eq!(
        (StatusCode::OK, StatusCode::OK, StatusCode::OK),
        (newer, stale, same_time)
    );
    assert_eq!(1, raw_revision_count(source.id, 109).await);
    assert!(matches!(
        harness.process_pending().await.as_slice(),
        [Ok(ProductListingIngestionCompletion::Applied(
            ProductListingIngestionEffect::RawCaptured(_)
        ))]
    ));
    assert_eq!(2, raw_revision_count(source.id, 109).await);
    for revision in [1, 2] {
        assert_eq!(
            Some(occurred_at("2026-01-01T00:00:02Z")),
            raw_revision(source.id, 109, revision)
                .await
                .source_occurred_at
        );
    }
    assert_eq!(
        vec![
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD.to_owned(),
                delivery_id: "shopify-webhook:webhook-newer".to_owned(),
                source_payload_sha256: source_payload_digest(&newer_a),
            },
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-older".to_owned(),
                source_payload_sha256: source_payload_digest(&older_b),
            },
            ProviderReceiptRow {
                scope: SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE.to_owned(),
                delivery_id: "shopify-webhook:webhook-same-time".to_owned(),
                source_payload_sha256: source_payload_digest(&same_time_c),
            },
        ],
        provider_receipts(source.id, 109).await
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_normalize_shopify_machine_decimal_prices_and_preserve_provider_strings() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let domain = source.domain.as_str();
    let cases = [
        (111, "42.000", Some(4_200_i64)),
        (112, "42.5", Some(4_250_i64)),
        (113, "42.50", Some(4_250_i64)),
        (114, "", None),
    ];

    for &(product_id, price, _) in &cases {
        let mut payload = shopify_payload(product_id, 5, serde_json::json!({}));
        payload["variants"][0]["price"] = serde_json::json!(price);
        let event_id = format!("shopify-machine-{product_id}");
        let webhook_id = format!("webhook-machine-{product_id}");

        let status = harness
            .send_and_process(delivery_with_payload(
                SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
                domain,
                payload,
                &event_id,
                &webhook_id,
            ))
            .await;

        assert_eq!(StatusCode::OK, status);
        let revision = raw_revision(source.id, product_id, 1).await;
        assert_eq!(1, revision.raw_values_schema_version);
        assert_eq!(
            serde_json::json!("MACHINE_DECIMAL"),
            revision.raw_values.0["priceFormat"]
        );
        assert_eq!(
            serde_json::json!(price),
            revision.source_payload.0["product_listing"]["variants"][0]["price"]
        );
        let expected_price_patch = if price.is_empty() {
            serde_json::json!({"action": "CLEAR"})
        } else {
            serde_json::json!({"action": "SET", "value": price})
        };
        assert_eq!(expected_price_patch, revision.raw_values.0["price"]);
    }

    assert_eq!(
        cases.len(),
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    for &(product_id, _, expected_amount) in &cases {
        assert_eq!(
            expected_amount,
            listing_price_amount(source.id, product_id).await
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_preserve_current_shopify_listing_facts_after_asynchronous_normalization() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let status = harness
        .send_and_process(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            source.domain.as_str(),
            103,
            5,
            "shopify-event-1",
            "webhook-1",
            serde_json::json!({}),
        ))
        .await;
    assert_eq!(StatusCode::OK, status);
    assert_eq!(0, listing_count(source.id).await);

    let normalized = normalize_pending_shopify_revisions(get_postgres_client().await).await;

    assert_eq!(1, normalized);
    let listing = listing_facts(source.id, 103).await;
    assert_eq!(Some("IN_STOCK".to_owned()), listing.0);
    assert_eq!(4_200, listing.1);
    assert_eq!("USD", listing.2);
    assert_eq!(
        format!("https://{}/products/cabinet-103", source.domain.as_str()),
        listing.3
    );
    assert_eq!((None, None), listing_description(source.id, 103).await);

    let pool = get_postgres_client().await;
    sqlx::query(
        "UPDATE product_listings SET description_text = $1, description_language = $2 \
         WHERE listing_source_id = $3 AND source_listing_id = $4",
    )
    .bind("Existing canonical description")
    .bind("en")
    .bind(uuid::Uuid::from(source.id))
    .bind("103")
    .execute(&pool)
    .await
    .unwrap_or_else(|error| panic!("failed seeding existing canonical description: {error}"));

    let mut update_payload = shopify_payload(
        103,
        4,
        serde_json::json!({"body_html": "A replacement Shopify description"}),
    );
    update_payload["variants"][0]["price"] = serde_json::json!("43.00");
    let update_status = harness
        .send_and_process(delivery_with_payload(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            source.domain.as_str(),
            update_payload,
            "shopify-event-2",
            "webhook-2",
        ))
        .await;
    assert_eq!(StatusCode::OK, update_status);
    assert_eq!(
        1,
        normalize_pending_shopify_revisions(get_postgres_client().await).await
    );
    assert_eq!(
        (
            Some("Existing canonical description".to_owned()),
            Some("en".to_owned())
        ),
        listing_description(source.id, 103).await
    );
    assert_eq!(2, product_listing_event_count().await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_capture_delete_without_direct_withdrawal() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let domain = source.domain.as_str();

    let add = harness
        .send_and_process(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            domain,
            103,
            5,
            "shopify-event-1",
            "webhook-1",
            serde_json::json!({}),
        ))
        .await;
    let remove = harness
        .send_and_process(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE,
            domain,
            103,
            5,
            "shopify-event-2",
            "webhook-2",
            serde_json::json!({}),
        ))
        .await;

    assert_eq!((StatusCode::OK, StatusCode::OK), (add, remove));
    assert_eq!(2, raw_revision_count(source.id, 103).await);
    let delete_revision = raw_revision(source.id, 103, 2).await;
    assert_eq!("DELETE", delete_revision.operation);
    assert_eq!(1, delete_revision.raw_values_schema_version);
    assert_eq!(0, listing_count(source.id).await);
    assert_eq!(0, product_listing_event_count().await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_ignore_missing_source_and_capture_status_free_listing_updates() {
    let mut harness = ShopifyTestHarness::new().await;
    let missing_source = harness
        .send_and_process(product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            "missing-source.example",
            104,
            5,
            "shopify-event-1",
            "webhook-1",
            serde_json::json!({}),
        ))
        .await;
    assert_eq!(StatusCode::OK, missing_source);
    assert_eq!(0, raw_revision_count_for_source_listing_id(104).await);
    assert!(harness.published().is_empty());

    let source = seed_source().await;
    let mut missing_status_payload = shopify_payload(105, 5, serde_json::json!({}));
    missing_status_payload["status"] = serde_json::Value::Null;
    missing_status_payload["updated_at"] = serde_json::json!("not-a-timestamp");
    let missing_status = harness
        .send_and_process(delivery_with_payload(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            source.domain.as_str(),
            missing_status_payload,
            "shopify-event-2",
            "webhook-2",
        ))
        .await;
    assert_eq!(StatusCode::OK, missing_status);
    assert_eq!(1, raw_revision_count(source.id, 105).await);

    let mut unsupported_status_payload = shopify_payload(106, 5, serde_json::json!({}));
    unsupported_status_payload["status"] = serde_json::json!("published");
    unsupported_status_payload["updated_at"] = serde_json::json!("not-a-timestamp");
    let unsupported_status = harness
        .send_and_process(delivery_with_payload(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            source.domain.as_str(),
            unsupported_status_payload,
            "shopify-event-3",
            "webhook-3",
        ))
        .await;
    assert_eq!(StatusCode::OK, unsupported_status);
    assert_eq!(1, raw_revision_count(source.id, 106).await);
    assert_eq!(2, harness.published().len());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_permanently_malformed_shopify_product_without_capture() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let mut payload = shopify_payload(106, 5, serde_json::json!({}));
    payload["product_id"] = serde_json::json!("not-a-decimal-id");

    let status = harness
        .send_and_process(delivery_with_payload(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            source.domain.as_str(),
            payload,
            "shopify-event-1",
            "webhook-1",
        ))
        .await;

    assert_eq!(StatusCode::BAD_REQUEST, status);
    assert_eq!(0, raw_revision_count_for_source_listing_id(106).await);
    assert!(harness.published().is_empty());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_use_trigger_time_independently_of_product_updated_at() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    let mut payload = shopify_payload(110, 5, serde_json::json!({}));
    payload["updated_at"] = serde_json::json!("not-a-timestamp");

    let status = harness
        .send_and_process(delivery_with_payload(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_UPDATE,
            source.domain.as_str(),
            payload,
            "shopify-event-invalid-time",
            "webhook-invalid-time",
        ))
        .await;

    assert_eq!(StatusCode::OK, status);
    assert_eq!(1, raw_revision_count(source.id, 110).await);
    assert_eq!(
        Some(occurred_at(DEFAULT_TRIGGERED_AT)),
        raw_revision(source.id, 110, 1).await.source_occurred_at
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_retry_unconfirmed_publish_with_stable_command_and_one_durable_capture() {
    let source = seed_source().await;
    let mut harness = ShopifyTestHarness::new().await;
    harness
        .publisher
        .unconfirm_next
        .store(true, Ordering::SeqCst);

    let request = || {
        product_delivery(
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            source.domain.as_str(),
            115,
            5,
            "shopify-event-retry",
            "webhook-retry",
            serde_json::json!({}),
        )
    };
    assert_eq!(
        StatusCode::SERVICE_UNAVAILABLE,
        harness.send(request()).await
    );
    assert_eq!(0, raw_revision_count(source.id, 115).await);
    assert_eq!(StatusCode::OK, harness.send(request()).await);

    let wires = harness.published();
    assert_eq!(2, wires.len());
    let first = codec::decode(&wires[0]).unwrap();
    let retry = codec::decode(&wires[1]).unwrap();
    assert_eq!(first.command_id(), retry.command_id());
    assert_eq!(first.submission_id(), retry.submission_id());
    assert_eq!(
        first.verified_fingerprint().unwrap(),
        retry.verified_fingerprint().unwrap()
    );
    assert_eq!(
        codec::fifo_deduplication_id(&first).unwrap(),
        codec::fifo_deduplication_id(&retry).unwrap()
    );
    assert_eq!(
        codec::fifo_group_id(&first).unwrap(),
        codec::fifo_group_id(&retry).unwrap()
    );
    assert!(matches!(
        harness.process_pending().await.as_slice(),
        [
            Ok(ProductListingIngestionCompletion::Applied(
                ProductListingIngestionEffect::RawCaptured(_)
            )),
            Ok(ProductListingIngestionCompletion::AlreadyCompleted)
        ]
    ));
    assert_eq!(1, raw_revision_count(source.id, 115).await);
    assert_eq!(1, provider_receipts(source.id, 115).await.len());
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_command_receipts WHERE command_id = $1",
    )
    .bind(first.command_id())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap();
    assert_eq!(1, receipt_count);
    assert_eq!(0, listing_count(source.id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_withdraw_backfill_only_listing_and_preserve_other_source_and_publication_order() {
    let source = seed_source().await;
    let other = seed_source().await;
    seed_canonical_listing(source.id, "42").await;
    seed_canonical_listing(other.id, "42").await;
    let mut harness = ShopifyTestHarness::new().await;
    for (topic, at, delivery_id, payload) in [
        (
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE,
            "2026-01-01T00:00:02Z",
            "remove",
            serde_json::json!({"product_id":42}),
        ),
        (
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            "2026-01-01T00:00:01Z",
            "old-add",
            shopify_payload(42, 1, serde_json::json!({})),
        ),
    ] {
        assert_eq!(
            StatusCode::OK,
            harness
                .send_and_process(delivery(
                    topic,
                    source.domain.as_str(),
                    payload,
                    delivery_id,
                    delivery_id,
                    Some(at),
                ))
                .await
        );
    }
    assert_eq!(
        1,
        normalize_pending_shopify_revisions(harness.pool.clone()).await
    );
    assert_eq!("WITHDRAWN", lifecycle(source.id, "42").await);
    assert_eq!("ACTIVE", lifecycle(other.id, "42").await);
    assert_eq!(1, raw_revision_count(source.id, 42).await);
    for (topic, at, delivery_id) in [
        (
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_ADD,
            "2026-01-01T00:00:03Z",
            "republish",
        ),
        (
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE,
            "2026-01-01T00:00:02Z",
            "delayed-remove",
        ),
        (
            SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE,
            "2026-01-01T00:00:02Z",
            "delayed-remove",
        ),
    ] {
        let payload = if topic == SHOPIFY_TOPIC_PRODUCT_LISTINGS_REMOVE {
            serde_json::json!({"product_id":42})
        } else {
            shopify_payload(
                42,
                0,
                serde_json::json!({"updated_at":"2025-01-01T00:00:00Z"}),
            )
        };
        assert_eq!(
            StatusCode::OK,
            harness
                .send_and_process(delivery(
                    topic,
                    source.domain.as_str(),
                    payload,
                    delivery_id,
                    delivery_id,
                    Some(at),
                ))
                .await
        );
    }
    normalize_pending_shopify_revisions(harness.pool.clone()).await;
    assert_eq!("ACTIVE", lifecycle(source.id, "42").await);
    assert_eq!(
        Some("OUT_OF_STOCK".to_owned()),
        listing_facts(source.id, 42).await.0
    );
    assert_eq!(1, listing_count(source.id).await);
}

#[derive(Clone, Default)]
struct CapturingPublisher {
    wires: Arc<Mutex<Vec<String>>>,
    unconfirm_next: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl ProductListingIngestionPublisher for CapturingPublisher {
    async fn publish(
        &self,
        commands: Vec<ProductListingIngestionMessage>,
    ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError> {
        let mut wires = Vec::with_capacity(commands.len());
        let unconfirmed = self.unconfirm_next.swap(false, Ordering::SeqCst);
        let outcomes = commands
            .iter()
            .map(|command| {
                wires.push(
                    codec::encode(command)
                        .expect("Shopify command must fit the FIFO wire contract"),
                );
                ProductListingIngestionItemOutcome {
                    index: command.metadata.index,
                    command_id: command.metadata.command_id.clone(),
                    outcome: if unconfirmed {
                        ProductListingIngestionOutcome::Unconfirmed
                    } else {
                        ProductListingIngestionOutcome::Accepted
                    },
                }
            })
            .collect();
        self.wires.lock().unwrap().extend(wires);
        Ok(outcomes)
    }
}

struct ShopifyTestHarness {
    pool: sqlx::PgPool,
    publisher: CapturingPublisher,
    processed: usize,
}

impl ShopifyTestHarness {
    async fn new() -> Self {
        Self {
            pool: get_postgres_client().await,
            publisher: CapturingPublisher::default(),
            processed: 0,
        }
    }

    async fn send(&self, request: Request<Body>) -> StatusCode {
        let processor = ProcessShopifyProductListingHandler::new(
            SqlxListingSourceReaders::new(self.pool.clone()),
            SubmitInternalProductListingIngestionHandler::new(self.publisher.clone()),
            ShopifyProductPayloadDecoder,
        );
        let secrets = ShopifyWebhookSecrets::new(CLIENT_SECRET.to_owned(), None)
            .unwrap_or_else(|_| panic!("Shopify test client secret must be nonblank"));
        let router = app(
            AppState::new().with_shopify_webhooks(ShopifyWebhooksState::new(
                Arc::new(ShopifyHmacWebhookVerifier::new(secrets)),
                Arc::new(processor),
            )),
        );
        router
            .oneshot(request)
            .await
            .unwrap_or_else(|error| panic!("Shopify webhook route failed: {error}"))
            .status()
    }

    fn published(&self) -> Vec<String> {
        self.publisher.wires.lock().unwrap().clone()
    }

    async fn process_pending(
        &mut self,
    ) -> Vec<Result<ProductListingIngestionCompletion, ProductListingIngestionError>> {
        let wires = self.published();
        let pending = &wires[self.processed..];
        self.processed = wires.len();
        let consumer = ProcessProductListingIngestionHandler::new(
            SqlxUnitOfWork::new(self.pool.clone()),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            SqlxPartnerProductListingAuthorizerFactory::new(),
            SqlxAuctionReferenceValidatorFactory::new(),
            SqlxProductListingRawCaptureWriterFactory::new(),
            SqlxProductListingCommandReceiptStoreFactory::new(),
        );
        let mut results = Vec::with_capacity(pending.len());
        for wire in pending {
            let envelope = codec::decode(wire)
                .expect("published command must decode")
                .into_service_envelope()
                .expect("published command must be a verified service envelope");
            results.push(consumer.execute(envelope).await);
        }
        results
    }

    async fn send_and_process(&mut self, request: Request<Body>) -> StatusCode {
        let status = self.send(request).await;
        for result in self.process_pending().await {
            assert!(result.is_ok(), "ingestion consumer failed: {result:?}");
        }
        status
    }
}

fn capture_command(
    wire: &str,
) -> (
    String,
    product_listing_service::use_cases::CaptureProductListingRawObservationCommand,
) {
    let envelope = codec::decode(wire).expect("published command must decode");
    let command_id = envelope.command_id().to_owned();
    let service = envelope
        .into_service_envelope()
        .expect("verified ingestion envelope");
    assert_eq!(
        ProductListingIngestionOperation::CaptureRaw,
        service.message.metadata.operation
    );
    let ProductListingIngestionIntent::CaptureRaw(command) = service.message.intent else {
        panic!("Shopify must publish CAPTURE_RAW, not a canonical write");
    };
    (command_id, command)
}

struct ShopifySourceFixture {
    id: ListingSourceId,
    domain: Domain,
}

async fn seed_source() -> ShopifySourceFixture {
    let listing_source_id = ListingSourceId::new();
    let operator_party_id = uuid::Uuid::now_v7();
    let domain =
        Domain::try_from(format!("shopify-{}.example", listing_source_id.as_uuid()).as_str())
            .unwrap_or_else(|error| panic!("invalid Shopify domain: {error}"));
    let pool = get_postgres_client().await;

    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(operator_party_id)
        .bind(format!("operator-{operator_party_id}"))
        .bind("Shopify operator")
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting source operator: {error}"));
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, $2, $3, $4, 'DEALER')")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(format!("shopify-source-{}", listing_source_id.as_uuid()))
        .bind("Shopify source")
        .bind(operator_party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting listing source: {error}"));
    sqlx::query("INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'SHOPIFY')")
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting Shopify ingestion method: {error}"));
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations (listing_source_id, domain, currency, language) VALUES ($1, $2, 'USD', 'de')")
        .bind(uuid::Uuid::from(listing_source_id))
        .bind(domain.as_str())
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting Shopify source configuration: {error}"));
    let partnership_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership_id)
        .bind(operator_party_id)
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed inserting source operator partnership: {error}"));
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)")
        .bind(partnership_id)
        .bind(uuid::Uuid::from(listing_source_id))
        .execute(&pool)
        .await
        .unwrap_or_else(|error| panic!("failed granting Shopify source access: {error}"));

    ShopifySourceFixture {
        id: listing_source_id,
        domain,
    }
}

fn product_delivery(
    topic: &str,
    shop_domain: &str,
    product_id: u64,
    inventory_quantity: i64,
    shopify_event_id: &str,
    shopify_webhook_id: &str,
    extra: serde_json::Value,
) -> Request<Body> {
    delivery_with_payload(
        topic,
        shop_domain,
        shopify_payload(product_id, inventory_quantity, extra),
        shopify_event_id,
        shopify_webhook_id,
    )
}

fn delivery_with_payload(
    topic: &str,
    shop_domain: &str,
    payload: serde_json::Value,
    shopify_event_id: &str,
    shopify_webhook_id: &str,
) -> Request<Body> {
    delivery(
        topic,
        shop_domain,
        payload,
        shopify_event_id,
        shopify_webhook_id,
        Some(DEFAULT_TRIGGERED_AT),
    )
}

/// One signed Shopify delivery whose body is the `{"product_listing": ...}` webhook payload.
fn delivery(
    topic: &str,
    shop_domain: &str,
    payload: serde_json::Value,
    shopify_event_id: &str,
    shopify_webhook_id: &str,
    shopify_triggered_at: Option<&str>,
) -> Request<Body> {
    let body = serde_json::to_vec(&serde_json::json!({"product_listing": payload}))
        .unwrap_or_else(|error| panic!("failed serializing Shopify webhook body: {error}"));
    let mut builder = Request::post("/v1/webhooks/shopify")
        .header("content-type", "application/json")
        .header("X-Shopify-Hmac-Sha256", sign(&body))
        .header("X-Shopify-Topic", topic)
        .header("X-Shopify-Shop-Domain", shop_domain)
        .header("X-Shopify-Webhook-Id", shopify_webhook_id)
        .header("X-Shopify-Event-Id", shopify_event_id)
        .header("X-Shopify-API-Version", "2026-10");
    if let Some(shopify_triggered_at) = shopify_triggered_at {
        builder = builder.header("X-Shopify-Triggered-At", shopify_triggered_at);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|error| panic!("failed building Shopify webhook request: {error}"))
}

fn sign(body: &[u8]) -> String {
    let key = PKey::hmac(CLIENT_SECRET.as_bytes())
        .unwrap_or_else(|error| panic!("invalid Shopify HMAC key: {error}"));
    let mut signer = Signer::new(MessageDigest::sha256(), &key)
        .unwrap_or_else(|error| panic!("failed creating Shopify HMAC signer: {error}"));
    signer
        .update(body)
        .unwrap_or_else(|error| panic!("failed signing Shopify webhook body: {error}"));
    let signature = signer
        .sign_to_vec()
        .unwrap_or_else(|error| panic!("failed signing Shopify webhook body: {error}"));
    base64::engine::general_purpose::STANDARD.encode(signature)
}

fn shopify_payload(
    product_id: u64,
    inventory_quantity: i64,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "product_id": product_id,
        "title": "Shopify Cabinet",
        "body_html": "<p>Imported cabinet</p>",
        "handle": format!("cabinet-{product_id}"),
        "status": "active",
        "variants": [{"price": "42.00", "inventory_quantity": inventory_quantity, "inventory_management": "shopify"}],
        "images": [{"src": "https://images.example/cabinet.jpg"}]
    });
    if let (Some(target), Some(extra)) = (payload.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    payload
}

fn shopify_payload_with_updated_at(
    product_id: u64,
    inventory_quantity: i64,
    updated_at: &str,
    extra: serde_json::Value,
) -> serde_json::Value {
    let mut payload = shopify_payload(product_id, inventory_quantity, extra);
    payload["updated_at"] = serde_json::json!(updated_at);
    payload
}

#[derive(sqlx::FromRow)]
struct RawRevisionRow {
    operation: String,
    payload_format: String,
    raw_values_schema_version: i16,
    source_payload: Json<serde_json::Value>,
    raw_values: Json<serde_json::Value>,
    normalization_context: Json<serde_json::Value>,
    provenance: Json<serde_json::Value>,
    source_event_id: Option<String>,
    source_occurred_at: Option<OffsetDateTime>,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct ProviderReceiptRow {
    scope: String,
    delivery_id: String,
    source_payload_sha256: Vec<u8>,
}

async fn raw_revision(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
    revision: i64,
) -> RawRevisionRow {
    sqlx::query_as::<_, RawRevisionRow>(
        "SELECT r.operation, r.payload_format, r.raw_values_schema_version, r.source_payload, r.raw_values, r.normalization_context, r.provenance, r.source_event_id, r.source_occurred_at \
         FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.listing_source_id = $1 AND s.ingestion_method = 'SHOPIFY' AND s.source_record_key = $2 AND r.revision = $3",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .bind(revision)
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading raw Shopify revision: {error}"))
}

async fn provider_receipts(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> Vec<ProviderReceiptRow> {
    sqlx::query_as::<_, ProviderReceiptRow>(
        "SELECT r.provider_scope AS scope, r.provider_delivery_id AS delivery_id, r.observation_sha256 AS source_payload_sha256 \
         FROM product_listing_raw_provider_observation_receipts r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.ingestion_method = 'SHOPIFY' AND s.listing_source_id = $1 AND s.source_record_key = $2 \
         ORDER BY r.provider_scope, r.provider_delivery_id",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_all(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading Shopify provider receipts: {error}"))
}

fn source_payload_digest(payload: &serde_json::Value) -> Vec<u8> {
    let mut sanitized_payload = payload.clone();
    if let Some(object) = sanitized_payload.as_object_mut() {
        object.remove("body_html");
    }
    SourcePayload::new(serde_json::json!({"product_listing": sanitized_payload}))
        .and_then(|source_payload| source_payload.canonical_sha256())
        .map(|sha256| sha256.as_bytes().to_vec())
        .unwrap_or_else(|error| panic!("failed hashing Shopify source payload: {error}"))
}

fn occurred_at(value: &str) -> OffsetDateTime {
    OffsetDateTime::parse(value, &Rfc3339)
        .unwrap_or_else(|error| panic!("invalid Shopify fixture timestamp: {error}"))
}

async fn raw_revision_count(listing_source_id: ListingSourceId, source_listing_id: u64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.ingestion_method = 'SHOPIFY' AND s.listing_source_id = $1 AND s.source_record_key = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed counting raw Shopify revisions: {error}"))
}

async fn raw_revision_count_for_source_listing_id(source_listing_id: u64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_raw_revisions r \
         JOIN product_listing_raw_streams s ON s.product_listing_raw_stream_id = r.product_listing_raw_stream_id \
         WHERE s.ingestion_method = 'SHOPIFY' AND s.source_record_key = $1",
    )
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed counting raw Shopify revisions: {error}"))
}

async fn normalize_pending_shopify_revisions(pool: sqlx::PgPool) -> usize {
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool),
    );
    normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::Reconcile,
            max_revisions_per_stream: 32,
            pending_stream_limit: 100,
        })
        .await
        .map(|result| result.revisions.len())
        .unwrap_or_else(|error| panic!("failed normalizing Shopify raw revision: {error}"))
}

async fn listing_price_amount(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT price_amount FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading normalized Shopify price: {error}"))
}

async fn listing_facts(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> (Option<String>, i64, String, String) {
    sqlx::query_as(
        "SELECT availability, price_amount, price_currency, url FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading normalized Shopify listing: {error}"))
}

async fn listing_description(
    listing_source_id: ListingSourceId,
    source_listing_id: u64,
) -> (Option<String>, Option<String>) {
    sqlx::query_as(
        "SELECT description_text, description_language FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(uuid::Uuid::from(listing_source_id))
    .bind(source_listing_id.to_string())
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading normalized Shopify description: {error}"))
}

async fn listing_count(listing_source_id: ListingSourceId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1")
        .bind(uuid::Uuid::from(listing_source_id))
        .fetch_one(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed counting ProductListings: {error}"))
}

async fn product_listing_event_count() -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM product_listing_events")
        .fetch_one(&get_postgres_client().await)
        .await
        .unwrap_or_else(|error| panic!("failed counting ProductListing events: {error}"))
}

async fn seed_canonical_listing(source: ListingSourceId, id: &str) {
    use product_listing_service::use_cases::{
        CreateProductListingCommand, CreateProductListingHandler, CreateProductListingUseCase,
    };
    CreateProductListingHandler::new(
        SqlxUnitOfWork::new(get_postgres_client().await),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
        SqlxAuctionReferenceValidatorFactory::new(),
    )
    .execute(
        &application::operation_context::OperationContext {
            principal: application::operation_context::Principal::System,
            request_id: application::operation_context::RequestId::new("backfill"),
            correlation_id: application::operation_context::CorrelationId::new("backfill"),
        },
        CreateProductListingCommand {
            listing_source_id: source,
            source_listing_id: id.try_into().unwrap(),
            title: None,
            description: None,
            pricing: Default::default(),
            availability: None,
            url: format!("https://example.test/products/{id}")
                .parse()
                .unwrap(),
            images: Default::default(),
            auction: None,
        },
    )
    .await
    .unwrap();
}

async fn lifecycle(source: ListingSourceId, id: &str) -> String {
    sqlx::query_scalar(
        "SELECT lifecycle FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = $2",
    )
    .bind(source.into_uuid())
    .bind(id)
    .fetch_one(&get_postgres_client().await)
    .await
    .unwrap_or_else(|error| panic!("failed loading ProductListing lifecycle: {error}"))
}
