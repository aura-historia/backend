use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};

use api_support::{
    seed_access_token_for, seed_operator_partnership_listing_source_grant,
    seed_partnership_membership, seed_user, woocommerce_ingestion_messages,
};
use base64::Engine;
use listing_source_core::ListingSourceId;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use platform_postgres::SqlxUnitOfWork;
use product_listing_ingestion_sqs::codec;
use product_listing_normalization::SourcePayload;
use product_listing_postgres::{
    SqlxPendingProductListingRawStreamReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRawNormalizationWriterFactory, SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    CaptureProductListingRawObservationResult, ProcessProductListingIngestionUseCase,
    ProductListingIngestionCompletion, ProductListingIngestionEffect,
    ProductListingIngestionIntent, ProductListingIngestionNotAttemptedReason,
    ProductListingIngestionOperation, ProductListingIngestionOutcome,
    ProductListingIngestionRejectionReason,
};
use product_service::use_cases::{
    NormalizeProductListingRawRevisionCommand, NormalizeProductListingRawRevisionHandler,
    NormalizeProductListingRawRevisionMode, NormalizeProductListingRawRevisionUseCase,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::access_token::Scope;

const SECRET: &str = "woocommerce-webhook-test-secret";
type TestResult = Result<(), Box<dyn std::error::Error>>;

fn assert_test_result(result: TestResult) {
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_admit_signed_woocommerce_product_without_synchronous_capture() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let body = json!({
            "id": 17,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet",
            "description": "<p>Cabinet description</p>",
            "short_description": "<p>Short cabinet description</p>",
            "price": "42.69",
            "status": "publish",
            "stock_status": "instock",
            "images": [],
            "date_modified_gmt": "2026-09-06T12:34:56",
            "futureWooKey": { "nested": true, "description": "Keep nested unknown data" }
        })
        .to_string();
        let response = send(
            &source,
            &token,
            "product.created",
            &body,
            Some("delivery-1"),
        )
        .await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert!(response.bytes().await?.is_empty());

        let source_id = source.parse::<ListingSourceId>()?;
        let messages = woocommerce_ingestion_messages(source_id);
        assert_eq!(1, messages.len());
        let message = &messages[0];
        assert_eq!(source_id, message.metadata.listing_source_id);
        assert_eq!(
            ProductListingIngestionOperation::CaptureRaw,
            message.metadata.operation
        );
        assert_eq!(0, message.metadata.index);
        assert_eq!(1, message.metadata.input_count);
        assert!(
            message
                .metadata
                .submission_id
                .as_str()
                .starts_with("plis1_")
        );
        assert!(message.metadata.command_id.as_str().starts_with("plic1_"));
        let ProductListingIngestionIntent::CaptureRaw(command) = &message.intent else {
            panic!("expected WooCommerce raw capture intent");
        };
        assert_eq!("17", command.source_record_key);
        assert_eq!("WOOCOMMERCE", command.ingestion_method.as_str());
        assert_eq!(Some("delivery-1"), command.source_event_id.as_deref());
        assert_eq!(
            json!(true),
            command.input.source_payload().value()["futureWooKey"]["nested"]
        );
        assert_eq!(
            json!("Keep nested unknown data"),
            command.input.source_payload().value()["futureWooKey"]["description"]
        );
        let mut expected_source_payload: Value = serde_json::from_str(&body)?;
        if let Some(object) = expected_source_payload.as_object_mut() {
            object.remove("description");
            object.remove("short_description");
        }
        assert_eq!(
            &expected_source_payload,
            command.input.source_payload().value()
        );
        assert_eq!(None, command.input.source_payload().value().get("description"));
        assert_eq!(
            None,
            command.input.source_payload().value().get("short_description")
        );
        assert_eq!(
            json!("MACHINE_DECIMAL"),
            command.input.raw_values().value()["priceFormat"]
        );
        assert_eq!(
            json!({"action": "SET", "value": "in stock"}),
            command.input.raw_values().value()["availability"]
        );
        assert_eq!(
            json!({"action": "UNCHANGED"}),
            command.input.raw_values().value()["description"]
        );
        let receipt = command
            .provider_receipt
            .as_ref()
            .ok_or("missing provider receipt")?;
        assert_eq!("product.created", receipt.scope().as_str());
        assert_eq!("delivery-1", receipt.delivery_id());
        let digest = SourcePayload::new(expected_source_payload.clone())?.canonical_sha256()?;
        assert_eq!(
            digest.as_bytes(),
            receipt.source_evidence_sha256().as_bytes()
        );
        let wire = codec::encode(message)?;
        assert!(!wire.contains(&token));
        assert!(!wire.contains(SECRET));
        assert!(!wire.contains(&signature(&body)));
        let restored = codec::decode(&wire)?.into_service_envelope()?;
        assert_eq!(message, &restored.message);
        assert_eq!(
            command.source_occurred_at,
            match &restored.message.intent {
                ProductListingIngestionIntent::CaptureRaw(raw) => raw.source_occurred_at,
                _ => panic!("expected raw capture after queue codec round trip"),
            }
        );
        assert_no_raw_rows(source_id).await?;
        // A confirmed webhook response transfers custody to the queue; it is not a raw
        // capture. Drive the actual codec and service-owned PostgreSQL processor manually.
        let envelope = codec::decode(&wire)?.into_service_envelope()?;
        let pool = get_postgres_client().await;
        let processor = super::async_product_listing_ingestion::processor(&pool);
        let capture = processor.execute(envelope.clone()).await?;
        assert!(matches!(
            &capture,
            ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
                _
            ))
        ));
        let stored: (String, String, Option<String>, serde_json::Value) = sqlx::query_as(
            "SELECT s.ingestion_method, s.source_record_key, r.source_event_id, r.source_payload \
             FROM product_listing_raw_streams s JOIN product_listing_raw_revisions r \
             ON r.product_listing_raw_stream_id = s.product_listing_raw_stream_id WHERE s.listing_source_id = $1",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(stored.0, "WOOCOMMERCE");
        assert_eq!(stored.1, "17");
        assert_eq!(stored.2.as_deref(), Some("delivery-1"));
        assert_eq!(stored.3, expected_source_payload);
        assert_eq!(stored.3["futureWooKey"]["nested"], true);
        assert_eq!(
            stored.3["futureWooKey"]["description"],
            "Keep nested unknown data"
        );
        assert_eq!(None, stored.3.get("description"));
        assert_eq!(None, stored.3.get("short_description"));
        assert_eq!(
            processor.execute(envelope).await?,
            ProductListingIngestionCompletion::AlreadyCompleted
        );
        normalize_captured_revision(&pool, &capture).await?;
        let canonical_description: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT description_text, description_language FROM product_listings \
             WHERE listing_source_id = $1 AND source_listing_id = '17'",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!((None, None), canonical_description);
        let receipts: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_command_receipts WHERE listing_source_id = $1",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(receipts, 1);
        let raw_revisions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_raw_revisions r JOIN product_listing_raw_streams s USING (product_listing_raw_stream_id) WHERE s.listing_source_id = $1",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(raw_revisions, 1);
        let listings: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listings WHERE listing_source_id = $1",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(listings, 1);

        sqlx::query(
            "UPDATE product_listings SET description_text = $1, description_language = $2 \
             WHERE listing_source_id = $3 AND source_listing_id = '17'",
        )
        .bind("Existing canonical description")
        .bind("en")
        .bind(source_id.as_uuid())
        .execute(&pool)
        .await?;

        let update_body = json!({
            "id": 17,
            "name": "Woo Cabinet",
            "permalink": "https://partner.example/product-listings/woo-cabinet",
            "description": "A replacement WooCommerce description",
            "short_description": "A different short description",
            "price": "43.00",
            "status": "publish",
            "stock_status": "instock",
            "images": [],
            "date_modified_gmt": "2026-09-07T12:34:56",
            "futureWooKey": { "nested": true }
        })
        .to_string();
        let update_response = send(
            &source,
            &token,
            "product.updated",
            &update_body,
            Some("delivery-2"),
        )
        .await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, update_response.status());
        let updated_messages = woocommerce_ingestion_messages(source_id);
        assert_eq!(2, updated_messages.len());
        let ProductListingIngestionIntent::CaptureRaw(update_command) = &updated_messages[1].intent else {
            panic!("expected WooCommerce raw update capture intent");
        };
        assert_eq!(None, update_command.input.source_payload().value().get("description"));
        assert_eq!(
            None,
            update_command.input.source_payload().value().get("short_description")
        );
        assert_eq!(
            json!({"action": "UNCHANGED"}),
            update_command.input.raw_values().value()["description"]
        );
        let update_envelope = codec::decode(&codec::encode(&updated_messages[1])?)?
            .into_service_envelope()?;
        let update_capture = processor.execute(update_envelope).await?;
        assert!(matches!(
            &update_capture,
            ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
                _
            ))
        ));
        normalize_captured_revision(&pool, &update_capture).await?;
        let preserved_description: (Option<String>, Option<String>) = sqlx::query_as(
            "SELECT description_text, description_language FROM product_listings \
             WHERE listing_source_id = $1 AND source_listing_id = '17'",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(
            (Some("Existing canonical description".to_owned()), Some("en".to_owned())),
            preserved_description
        );
        let final_raw_revisions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM product_listing_raw_revisions r JOIN product_listing_raw_streams s USING (product_listing_raw_stream_id) WHERE s.listing_source_id = $1",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(2, final_raw_revisions);
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_forward_machine_decimal_strings_without_running_normalization() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let cases = [(28, "42.000"), (29, "42.5"), (30, "42.50"), (31, "")];
        for (id, price) in cases {
            let body = product_body(id, price, "publish");
            assert_eq!(
                reqwest::StatusCode::NO_CONTENT,
                send(
                    &source,
                    &token,
                    "product.created",
                    &body,
                    Some(&format!("price-{id}"))
                )
                .await?
                .status()
            );
        }
        let source_id = source.parse::<ListingSourceId>()?;
        let messages = woocommerce_ingestion_messages(source_id);
        assert_eq!(cases.len(), messages.len());
        for (message, (id, price)) in messages.iter().zip(cases) {
            let ProductListingIngestionIntent::CaptureRaw(command) = &message.intent else {
                panic!("expected raw capture intent");
            };
            assert_eq!(id.to_string(), command.source_record_key);
            assert_eq!(
                json!(price),
                command.input.source_payload().value()["price"]
            );
            assert_eq!(
                json!("MACHINE_DECIMAL"),
                command.input.raw_values().value()["priceFormat"]
            );
            let patch = if price.is_empty() {
                json!({"action": "CLEAR"})
            } else {
                json!({"action": "SET", "value": price})
            };
            assert_eq!(patch, command.input.raw_values().value()["price"]);
        }
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_admit_retries_and_source_order_conflicts_for_later_worker_resolution() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let original = product_body_with_timestamp(27, "42.00", "2026-09-06T10:01:00");
        let changed = product_body_with_timestamp(27, "43.00", "2026-09-06T10:02:00");
        let same_timestamp = product_body_with_timestamp(27, "44.00", "2026-09-06T10:01:00");
        let stale = product_body_with_timestamp(27, "45.00", "2026-09-06T10:00:00");
        let attempts = [
            (&original, "delivery-original"),
            (&original, "delivery-original"),
            (&changed, "delivery-original"),
            (&same_timestamp, "delivery-source-order-conflict"),
            (&stale, "delivery-stale"),
        ];
        for (body, delivery) in attempts {
            assert_eq!(
                reqwest::StatusCode::NO_CONTENT,
                send(&source, &token, "product.updated", body, Some(delivery))
                    .await?
                    .status()
            );
        }
        let source_id = source.parse::<ListingSourceId>()?;
        let messages = woocommerce_ingestion_messages(source_id);
        assert_eq!(attempts.len(), messages.len());
        assert_eq!(
            messages[0].metadata.command_id,
            messages[1].metadata.command_id
        );
        assert_eq!(
            messages[0].metadata.command_id,
            messages[2].metadata.command_id
        );
        assert_ne!(
            messages[0].metadata.command_id,
            messages[3].metadata.command_id
        );
        for (message, (body, delivery)) in messages.iter().zip(attempts) {
            let ProductListingIngestionIntent::CaptureRaw(command) = &message.intent else {
                panic!("expected raw capture intent");
            };
            assert_eq!(Some(delivery), command.source_event_id.as_deref());
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(body)?,
                *command.input.source_payload().value()
            );
        }
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_admit_delete_without_immediate_withdrawal() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let body = json!({"id": 22}).to_string();
        for _ in 0..2 {
            assert_eq!(
                reqwest::StatusCode::NO_CONTENT,
                send(&source, &token, "product.deleted", &body, None)
                    .await?
                    .status()
            );
        }
        let source_id = source.parse::<ListingSourceId>()?;
        let messages = woocommerce_ingestion_messages(source_id);
        assert_eq!(2, messages.len());
        for message in messages {
            let ProductListingIngestionIntent::CaptureRaw(command) = message.intent else {
                panic!("expected raw capture intent");
            };
            assert_eq!("22", command.source_record_key);
            assert_eq!(
                "DELETE",
                format!("{:?}", command.input.operation()).to_uppercase()
            );
            assert!(command.provider_receipt.is_none());
        }
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_verify_webhooks_against_rotated_secret_using_exact_raw_body() {
    let result: TestResult = async {
        let (source, webhook_token) = webhook_auth().await?;
        let source_id = source.parse::<ListingSourceId>()?;
        let before_rotation_body = r#" { "id": 97, "name": "Rotation \\u00e9", "permalink": "https://partner.example/products/97", "price": "42.00", "status": "publish", "stock_status": "instock", "images": [] } "#;
        let after_rotation_body = r#" { "id": 98, "name": "Rotation \\u00e9", "permalink": "https://partner.example/products/98", "price": "43.00", "status": "publish", "stock_status": "instock", "images": [] } "#;

        let before_rotation = send_signed(
            &source,
            &webhook_token,
            "product.created",
            before_rotation_body,
            Some("rotation-before"),
            SECRET,
        )
        .await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, before_rotation.status());
        assert_eq!(1, woocommerce_ingestion_messages(source_id).len());

        let partner_user = seed_user("USER").await;
        seed_partnership_membership(partner_user, source_id.into_uuid()).await;
        let partner_token = String::from(
            seed_access_token_for(
                partner_user,
                HashSet::from([Scope::ListingSourcesWrite]),
            )
            .await,
        );
        let rotation = reqwest::Client::new()
            .put(format!(
                "{}/api/v1/listing-sources/{source}/ingestion-configurations/woocommerce",
                AURA_API.base_url()
            ))
            .bearer_auth(partner_token)
            .json(&json!({
                            "webhookSecret": "rotated-webhook-secret",
                            "currency": "EUR",
                            "language": "en"
                        }))
            .send()
            .await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, rotation.status());

        let old_secret = send_signed(
            &source,
            &webhook_token,
            "product.created",
            after_rotation_body,
            Some("rotation-old-secret"),
            SECRET,
        )
        .await?;
        assert_eq!(reqwest::StatusCode::UNAUTHORIZED, old_secret.status());
        assert_eq!("BAD_HEADER_VALUE", old_secret.json::<Value>().await?["error"]);
        assert_eq!(1, woocommerce_ingestion_messages(source_id).len());

        let new_secret = send_signed(
            &source,
            &webhook_token,
            "product.created",
            after_rotation_body,
            Some("rotation-new-secret"),
            "rotated-webhook-secret",
        )
        .await?;
        let new_secret_status = new_secret.status();
        let new_secret_body = new_secret.text().await?;
        assert_eq!(
            reqwest::StatusCode::NO_CONTENT,
            new_secret_status,
            "unexpected response body: {new_secret_body}"
        );
        assert!(new_secret_body.is_empty());
        assert_eq!(2, woocommerce_ingestion_messages(source_id).len());
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_missing_invalid_or_tampered_signatures_before_publication() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let body = json!({"id": 23}).to_string();
        let url = format!(
            "{}/api/v1/webhooks/woocommerce/{source}",
            AURA_API.base_url()
        );
        let client = reqwest::Client::new();
        for signature_header in [
            None,
            Some(signature("different-body")),
            Some(signature(&body)),
        ] {
            let mut request = client
                .post(&url)
                .bearer_auth(&token)
                .header("x-wc-webhook-topic", "product.deleted");
            let sent_body = if signature_header.as_deref() == Some(signature(&body).as_str()) {
                json!({"id": 24}).to_string()
            } else {
                body.clone()
            };
            if let Some(signature_header) = signature_header {
                request = request.header("x-wc-webhook-signature", signature_header);
            }
            let response = request.body(sent_body).send().await?;
            assert_eq!(reqwest::StatusCode::UNAUTHORIZED, response.status());
            assert_eq!(
                "BAD_HEADER_VALUE",
                response.json::<serde_json::Value>().await?["error"]
            );
        }
        let source_id = source.parse::<ListingSourceId>()?;
        assert!(woocommerce_ingestion_messages(source_id).is_empty());
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_require_write_capability_and_authorize_ignored_webhooks_without_a_source_grant() {
    let result: TestResult = async {
        let source_uuid = seed_listing_source().await;
        configure_woocommerce_source(source_uuid).await?;
        seed_operator_partnership_listing_source_grant(source_uuid).await;
        let source = ListingSourceId::try_from(source_uuid)?.to_string();
        let no_capability_user = seed_user("USER").await;
        seed_partnership_membership(no_capability_user, source_uuid).await;
        let no_capability =
            String::from(seed_access_token_for(no_capability_user, HashSet::new()).await);
        let no_grant_user = seed_user("USER").await;
        let no_grant = String::from(
            seed_access_token_for(no_grant_user, HashSet::from([Scope::ProductListingsWrite]))
                .await,
        );
        let cases = [
            ("product.updated", product_body(24, "42.00", "publish")),
            ("product.deleted", json!({"id": 25}).to_string()),
            (
                "product.updated",
                json!({"id": 26, "status": "future-status"}).to_string(),
            ),
        ];
        for (topic, body) in &cases {
            let response = send(&source, &no_capability, topic, body, None).await?;
            assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status());
            assert_eq!(
                "FORBIDDEN",
                response.json::<serde_json::Value>().await?["error"]
            );
        }
        let source_id = source.parse::<ListingSourceId>()?;
        assert!(woocommerce_ingestion_messages(source_id).is_empty());

        for (index, (topic, body)) in cases.iter().enumerate() {
            let response = send(&source, &no_grant, topic, body, None).await?;
            if index == 2 {
                assert_eq!(reqwest::StatusCode::FORBIDDEN, response.status());
                assert_eq!(
                    "FORBIDDEN",
                    response.json::<serde_json::Value>().await?["error"]
                );
            } else {
                assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
            }
        }
        // Write-capable published/deleted events are admitted to the queue; the
        // ignored path still checks the source grant through the no-op authorizer.
        assert_eq!(2, woocommerce_ingestion_messages(source_id).len());
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_acknowledge_authorized_ignored_webhook_without_sending() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let body = json!({"id": 30, "status": "future-status"}).to_string();
        let response = send(
            &source,
            &token,
            "product.updated",
            &body,
            Some("ignored-30"),
        )
        .await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert!(response.bytes().await?.is_empty());
        let source_id = source.parse::<ListingSourceId>()?;
        assert!(woocommerce_ingestion_messages(source_id).is_empty());
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_reject_invalid_paths_and_malformed_inputs_without_sending() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let bare = source.parse::<ListingSourceId>()?.into_uuid().to_string();
        for invalid_id in [
            source.replacen("ls_", "usr_", 1),
            bare,
            "not-an-object-id".to_owned(),
        ] {
            let response = send(
                &invalid_id,
                &token,
                "product.created",
                &product_body(32, "42.00", "publish"),
                None,
            )
            .await?;
            assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
            assert_eq!(
                "INVALID_OBJECT_ID",
                response.json::<serde_json::Value>().await?["error"]
            );
        }
        for (topic, body, expected_error) in [
            (
                "orders.created",
                json!({"id": 25}).to_string(),
                "BAD_HEADER_VALUE",
            ),
            ("product.created", "not-json".to_owned(), "BAD_BODY_VALUE"),
            ("product.created", "".to_owned(), "BAD_BODY_VALUE"),
            (
                "product.created",
                json!({"id": 33, "status": "publish"}).to_string(),
                "BAD_BODY_VALUE",
            ),
            (
                "product.created",
                product_body_with_timestamp(34, "42.00", "not-a-gmt-timestamp"),
                "BAD_BODY_VALUE",
            ),
            (
                "product.created",
                product_body_with_timestamp(35, "42.00", "2026-09-06T10:03:00+01:00"),
                "BAD_BODY_VALUE",
            ),
        ] {
            let response = send(&source, &token, topic, &body, None).await?;
            assert_eq!(reqwest::StatusCode::BAD_REQUEST, response.status());
            assert_eq!(
                expected_error,
                response.json::<serde_json::Value>().await?["error"]
            );
        }
        let source_id = source.parse::<ListingSourceId>()?;
        assert!(woocommerce_ingestion_messages(source_id).is_empty());
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_fail_closed_when_woocommerce_secret_or_configuration_is_unavailable() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let source_id = source.parse::<ListingSourceId>()?;
        let pool = get_postgres_client().await;
        let invalid_secret_update = sqlx::query("UPDATE listing_source_woocommerce_ingestion_configurations SET webhook_secret = '  ' WHERE listing_source_id = $1")
            .bind(source_id.as_uuid()).execute(&pool).await;
        assert!(invalid_secret_update.is_err(), "database accepted a blank WooCommerce secret");
        let persisted_secret = sqlx::query_scalar::<_, String>(
            "SELECT webhook_secret FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id = $1",
        )
        .bind(source_id.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!(SECRET, persisted_secret);

        let body = product_body(36, "42.00", "publish");
        let response = send(&source, &token, "product.created", &body, None).await?;
        assert_eq!(reqwest::StatusCode::NO_CONTENT, response.status());
        assert_eq!(1, woocommerce_ingestion_messages(source_id).len());

        sqlx::query("DELETE FROM listing_source_woocommerce_ingestion_configurations WHERE listing_source_id = $1")
            .bind(source_id.as_uuid()).execute(&pool).await?;
        sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id = $1 AND ingestion_method = 'WOOCOMMERCE'")
            .bind(source_id.as_uuid()).execute(&pool).await?;
        let response = send(&source, &token, "product.created", &body, None).await?;
        assert_eq!(reqwest::StatusCode::NOT_FOUND, response.status());
        assert_eq!("LISTING_SOURCE_NOT_FOUND", response.json::<serde_json::Value>().await?["error"]);
        assert_eq!(1, woocommerce_ingestion_messages(source_id).len());
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }.await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_not_acknowledge_unconfirmed_queue_admission() {
    let result: TestResult = async {
        let (source, token) = webhook_auth().await?;
        let source_id = source.parse::<ListingSourceId>()?;
        api_support::set_woocommerce_publisher_outcome(
            source_id,
            ProductListingIngestionOutcome::Unconfirmed,
        );
        let response = send(
            &source,
            &token,
            "product.created",
            &product_body(37, "42.00", "publish"),
            Some("unconfirmed-37"),
        )
        .await?;
        assert_eq!(reqwest::StatusCode::SERVICE_UNAVAILABLE, response.status());
        assert_eq!(
            "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE",
            response.json::<serde_json::Value>().await?["error"]
        );
        assert_eq!(1, woocommerce_ingestion_messages(source_id).len());
        assert_no_raw_rows(source_id).await?;
        Ok(())
    }
    .await;
    assert_test_result(result);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn should_report_publisher_rejections_instead_of_acknowledging_them() {
    let result: TestResult = async {
        for (outcome, expected_status, expected_error) in [
            (
                ProductListingIngestionOutcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "INVALID_MESSAGE_SIZE".to_owned(),
                    },
                    retryable: false,
                },
                reqwest::StatusCode::PAYLOAD_TOO_LARGE,
                "PRODUCT_LISTING_INGESTION_PAYLOAD_TOO_LARGE",
            ),
            (
                ProductListingIngestionOutcome::Rejected {
                    reason: ProductListingIngestionRejectionReason::Publisher {
                        code: "ENCODING_FAILED".to_owned(),
                    },
                    retryable: false,
                },
                reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                "PRODUCT_LISTING_INTERNAL_ERROR",
            ),
            (
                ProductListingIngestionOutcome::NotAttempted {
                    reason: ProductListingIngestionNotAttemptedReason::DeadlineExceeded,
                    retryable: true,
                },
                reqwest::StatusCode::SERVICE_UNAVAILABLE,
                "PRODUCT_LISTING_TEMPORARILY_UNAVAILABLE",
            ),
        ] {
            let (source, token) = webhook_auth().await?;
            let source_id = source.parse::<ListingSourceId>()?;
            api_support::set_woocommerce_publisher_outcome(source_id, outcome);
            let response = send(
                &source,
                &token,
                "product.created",
                &product_body(38, "42.00", "publish"),
                Some("failed-forward-38"),
            )
            .await?;
            assert_eq!(expected_status, response.status());
            assert_eq!(
                expected_error,
                response.json::<serde_json::Value>().await?["error"]
            );
            assert_eq!(1, woocommerce_ingestion_messages(source_id).len());
            assert_no_raw_rows(source_id).await?;
        }
        Ok(())
    }
    .await;
    assert_test_result(result);
}

async fn normalize_captured_revision(
    pool: &sqlx::PgPool,
    completion: &ProductListingIngestionCompletion,
) -> TestResult {
    let ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::RawCaptured(
        CaptureProductListingRawObservationResult::Changed {
            product_listing_raw_stream_id,
            product_listing_raw_revision_id,
            revision,
        },
    )) = completion
    else {
        panic!("expected a changed raw revision");
    };
    let normalizer = NormalizeProductListingRawRevisionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool.clone()),
    );
    let result = normalizer
        .execute(NormalizeProductListingRawRevisionCommand {
            mode: NormalizeProductListingRawRevisionMode::RawRevision {
                product_listing_raw_stream_id: *product_listing_raw_stream_id,
                product_listing_raw_revision_id: *product_listing_raw_revision_id,
                revision: *revision,
            },
            max_revisions_per_stream: 1,
            pending_stream_limit: 1,
        })
        .await?;
    assert_eq!(1, result.revisions.len());
    Ok(())
}

async fn assert_no_raw_rows(source: ListingSourceId) -> TestResult {
    let pool = get_postgres_client().await;
    let raw_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM product_listing_raw_streams WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await?;
    let listing_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM product_listings WHERE listing_source_id = $1")
            .bind(source.as_uuid())
            .fetch_one(&pool)
            .await?;
    assert_eq!(0, raw_count);
    assert_eq!(0, listing_count);
    Ok(())
}

async fn seed_listing_source() -> uuid::Uuid {
    let source = uuid::Uuid::now_v7();
    let party = uuid::Uuid::now_v7();
    let pool = get_postgres_client().await;
    let mut tx = pool.begin().await.expect("begin source seed");
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, $3)")
        .bind(party)
        .bind(format!("woocommerce-webhook-party-{party}"))
        .bind(format!("WooCommerce Webhook Party {party}"))
        .execute(&mut *tx)
        .await
        .expect("seed party");
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, url) VALUES ($1, $2, $3, $4, $5)")
        .bind(source).bind(format!("woocommerce-webhook-source-{source}"))
        .bind(format!("WooCommerce Webhook Listing Source {source}"))
        .bind(party).bind("https://woocommerce-webhook.example/")
        .execute(&mut *tx).await.expect("seed source");
    sqlx::query("INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'PARTNER_API')")
        .bind(source).execute(&mut *tx).await.expect("seed ingestion method");
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(uuid::Uuid::now_v7())
        .bind(party)
        .execute(&mut *tx)
        .await
        .expect("seed partnership");
    tx.commit().await.expect("commit source seed");
    source
}

async fn webhook_auth() -> Result<(String, String), Box<dyn std::error::Error>> {
    let source = seed_listing_source().await;
    configure_woocommerce_source(source).await?;
    let user = seed_user("USER").await;
    seed_partnership_membership(user, source).await;
    seed_operator_partnership_listing_source_grant(source).await;
    let token = seed_access_token_for(user, HashSet::from([Scope::ProductListingsWrite])).await;
    Ok((
        ListingSourceId::try_from(source)?.to_string(),
        String::from(token),
    ))
}

async fn configure_woocommerce_source(source: uuid::Uuid) -> Result<(), sqlx::Error> {
    let pool = get_postgres_client().await;
    sqlx::query("INSERT INTO listing_source_ingestion_methods (listing_source_id, ingestion_method) VALUES ($1, 'WOOCOMMERCE')")
        .bind(source).execute(&pool).await?;
    sqlx::query("INSERT INTO listing_source_woocommerce_ingestion_configurations (listing_source_id, webhook_secret, currency, language) VALUES ($1, $2, 'EUR', 'en')")
        .bind(source).bind(SECRET).execute(&pool).await?;
    Ok(())
}

fn product_body(id: u64, price: &str, status: &str) -> String {
    json!({"id": id, "name": "Woo Cabinet", "permalink": format!("https://partner.example/products/{id}"),
        "price": price, "status": status, "stock_status": "instock", "images": []}).to_string()
}

fn product_body_with_timestamp(id: u64, price: &str, timestamp: &str) -> String {
    let mut body: serde_json::Value =
        serde_json::from_str(&product_body(id, price, "publish")).expect("valid product body");
    body["date_modified_gmt"] = json!(timestamp);
    body.to_string()
}

async fn send(
    source: &str,
    token: &str,
    topic: &str,
    body: &str,
    delivery: Option<&str>,
) -> Result<reqwest::Response, reqwest::Error> {
    send_signed(source, token, topic, body, delivery, SECRET).await
}

async fn send_signed(
    source: &str,
    token: &str,
    topic: &str,
    body: &str,
    delivery: Option<&str>,
    secret: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let request = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/webhooks/woocommerce/{source}",
            AURA_API.base_url()
        ))
        .bearer_auth(token)
        .header("x-wc-webhook-topic", topic)
        .header(
            "x-wc-webhook-signature",
            signature_with_secret(body, secret),
        );
    let request = match delivery {
        Some(delivery) => request.header("x-wc-webhook-delivery-id", delivery),
        None => request,
    };
    request.body(body.to_owned()).send().await
}

fn signature(body: &str) -> String {
    signature_with_secret(body, SECRET)
}

fn signature_with_secret(body: &str, secret: &str) -> String {
    let key = PKey::hmac(secret.as_bytes()).expect("HMAC key");
    let mut signer = Signer::new(MessageDigest::sha256(), &key).expect("HMAC signer");
    signer.update(body.as_bytes()).expect("sign body");
    base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().expect("HMAC signature"))
}
