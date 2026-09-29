use crate::{AURA_API, BUSINESS_SCHEMA, OPENSEARCH, api_support};
use api_support::{
    captured_ingestion_messages, seed_access_token_for, seed_listing_source,
    seed_operator_partnership_listing_source_grant, seed_partnership_membership, seed_user,
};

use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use listing_source_core::ListingSourceId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_ingestion_sqs::codec;
use product_listing_postgres::{
    SqlxPartnerProductListingAuthorizerFactory, SqlxProductListingCommandReceiptStoreFactory,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawCaptureWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::use_cases::{
    ProcessProductListingIngestionHandler, ProcessProductListingIngestionUseCase,
    ProductListingIngestionCompletion, ProductListingIngestionEffect, ProductListingIngestionError,
    ProductListingIngestionIntent, ProductListingIngestionOperation, UpdateProductListingError,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::access_token::Scope;

type TestResult = Result<(), Box<dyn std::error::Error>>;

async fn auth() -> (ListingSourceId, String) {
    let source = seed_listing_source().await;
    let user = seed_user("USER").await;
    seed_partnership_membership(user, source).await;
    seed_operator_partnership_listing_source_grant(source).await;
    let token = seed_access_token_for(user, HashSet::from([Scope::ProductListingsWrite])).await;
    (
        ListingSourceId::try_from(source).unwrap(),
        String::from(token),
    )
}

fn product(id: &str) -> Value {
    json!({"sourceListingId": id, "title": {"text": "Cabinet", "language": "en"},
        "description": {"text": "Original", "language": "en"},
        "url": format!("https://example.test/{id}"), "images": []})
}

async fn send(
    source: ListingSourceId,
    token: Option<&str>,
    method: reqwest::Method,
    body: &str,
    key: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let request = reqwest::Client::new()
        .request(
            method,
            format!(
                "{}/api/v1/listing-sources/{source}/product-listings/async",
                AURA_API.base_url()
            ),
        )
        .header("Idempotency-Key", key);
    let request = if let Some(token) = token {
        request.bearer_auth(token)
    } else {
        request
    };
    request.body(body.to_owned()).send().await
}

async fn counts(source: ListingSourceId) -> (i64, i64, i64, i64) {
    let pool = get_postgres_client().await;
    sqlx::query_as(
        "SELECT (SELECT count(*) FROM product_listings WHERE listing_source_id = $1), \
         (SELECT count(*) FROM product_listing_events e JOIN product_listings p USING (product_listing_id) WHERE p.listing_source_id = $1), \
         (SELECT count(*) FROM product_listing_command_receipts WHERE listing_source_id = $1), \
         (SELECT count(*) FROM product_listing_raw_streams WHERE listing_source_id = $1)",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool).await.unwrap()
}

pub(super) fn processor(pool: &sqlx::PgPool) -> impl ProcessProductListingIngestionUseCase {
    ProcessProductListingIngestionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPartnerProductListingAuthorizerFactory::new(),
        SqlxAuctionReferenceValidatorFactory::new(),
        SqlxProductListingRawCaptureWriterFactory::new(),
        SqlxProductListingCommandReceiptStoreFactory::new(),
    )
}

// Captured publication is deliberately not SQS delivery: execute only after asserting no
// authoritative listing, event, receipt, or raw write occurred at HTTP admission.
#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn rest_partial_admission_codec_and_postgres_execution_are_separate() {
    let result: TestResult = async {
    let (source, token) = auth().await;
    let body = format!("[{},null,{},{{}}]", product("first"), product("second"));
    let response = send(
        source,
        Some(&token),
        reqwest::Method::POST,
        &body,
        "sparse-create",
    )
    .await?;
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(response.headers()["idempotency-key"], "sparse-create");
    let report: Value = response.json().await?;
    assert_eq!(report["acceptedCount"], 2);
    assert_eq!(
        report["failures"],
        json!([
            {"index": 1, "error": "BAD_BODY_VALUE", "retryable": false},
            {"index": 3, "error": "BAD_BODY_VALUE", "retryable": false}
        ])
    );
    let messages = captured_ingestion_messages(source);
    assert_eq!(
        messages
            .iter()
            .map(|m| m.metadata.index)
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert!(messages.iter().all(|m| m.metadata.input_count == 4
        && m.metadata.operation == ProductListingIngestionOperation::Create));
    assert_eq!(counts(source).await, (0, 0, 0, 0));

    // Same whole array/key retains original indices and command identities, even with holes.
    let retry = send(
        source,
        Some(&token),
        reqwest::Method::POST,
        &body,
        "sparse-create",
    )
    .await?;
    assert_eq!(retry.status(), reqwest::StatusCode::ACCEPTED);
    assert_eq!(retry.json::<Value>().await?, report);
    let retried = captured_ingestion_messages(source);
    assert_eq!(retried.len(), 4);
    for (original, again) in messages.iter().zip(retried.iter().skip(2)) {
        assert_eq!(original.metadata.command_id, again.metadata.command_id);
        let original_wire = codec::decode(&codec::encode(original)?)?;
        let retry_wire = codec::decode(&codec::encode(again)?)?;
        assert_eq!(codec::semantic_fingerprint(&original_wire)?, codec::semantic_fingerprint(&retry_wire)?);
        assert_eq!(codec::fifo_deduplication_id(&original_wire)?, codec::fifo_deduplication_id(&retry_wire)?);
    }
    let first_wire = codec::decode(&codec::encode(&messages[0])?)?;
    let first_group = codec::fifo_group_id(&first_wire)?;
    assert!(!codec::encode(&messages[0])?.contains(&token));
    let pool = get_postgres_client().await;
    let processor = processor(&pool);
    for message in &messages {
        let decoded = codec::decode(&codec::encode(message)?)?.into_service_envelope()?;
        assert_eq!(decoded.message, *message);
        assert!(matches!(
            processor.execute(decoded.clone()).await?,
            ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Created(_))
        ));
        assert_eq!(
            processor.execute(decoded).await?,
            ProductListingIngestionCompletion::AlreadyCompleted
        );
    }
    assert_eq!(counts(source).await, (2, 2, 2, 0));

    let patch = r#"[{"sourceListingId":"first","url":"https://example.test/changed","availability":"SOLD_OUT"},{"sourceListingId":"missing","url":"https://example.test/missing"},{"sourceListingId":"invalid","url":null}]"#;
    let response = send(
        source,
        Some(&token),
        reqwest::Method::PATCH,
        patch,
        "patch-key",
    )
    .await?;
    assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
    let report: Value = response.json().await?;
    assert_eq!(report["acceptedCount"], 2);
    assert_eq!(
        report["failures"],
        json!([{"index":2,"sourceListingId":"invalid","error":"BAD_BODY_VALUE","retryable":false}])
    );
    assert_eq!(counts(source).await, (2, 2, 2, 0));
    let patch_messages = captured_ingestion_messages(source);
    assert_eq!(patch_messages.len(), 6);
    let changed_wire = codec::decode(&codec::encode(&patch_messages[4])?)?;
    assert_eq!(codec::fifo_group_id(&changed_wire)?, first_group);
    let changed = changed_wire.into_service_envelope()?;
    assert!(matches!(
        changed.message.intent,
        ProductListingIngestionIntent::Update { .. }
    ));
    assert!(matches!(
        processor.execute(changed).await?,
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::Updated(_))
    ));
    let missing = codec::decode(&codec::encode(&patch_messages[5])?)?.into_service_envelope()?;
    assert!(matches!(
        processor.execute(missing).await,
        Err(ProductListingIngestionError::Update(
            UpdateProductListingError::NotFound
        ))
    ));
    assert_eq!(counts(source).await, (2, 3, 3, 0));
    let state: (String, Option<String>) = sqlx::query_as(
        "SELECT url, availability FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = 'first'",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert_eq!(state, ("https://example.test/changed".to_owned(), Some("SOLD_OUT".to_owned())));

    for (method, body, key, operation, expected) in [
        (
            reqwest::Method::DELETE,
            json!([{"sourceListingId":"first"}]).to_string(),
            "withdraw-key",
            ProductListingIngestionOperation::Withdraw,
            (2, 4, 4, 0),
        ),
        (
            reqwest::Method::PUT,
            json!([product("first")]).to_string(),
            "restore-key",
            ProductListingIngestionOperation::Upsert,
            (2, 5, 5, 0),
        ),
    ] {
        let response = send(source, Some(&token), method, &body, key).await?;
        assert_eq!(response.status(), reqwest::StatusCode::ACCEPTED);
        assert_eq!(response.json::<Value>().await?["acceptedCount"], 1);
        assert_eq!(
            counts(source).await,
            (expected.0, expected.1 - 1, expected.2 - 1, expected.3)
        );
        let message = captured_ingestion_messages(source).pop().unwrap();
        assert_eq!(message.metadata.operation, operation);
        let wire = codec::decode(&codec::encode(&message)?)?;
        assert_eq!(codec::fifo_group_id(&wire)?, first_group);
        let command = wire.into_service_envelope()?;
        assert!(matches!(
            processor.execute(command).await?,
            ProductListingIngestionCompletion::Applied(_)
        ));
        assert_eq!(counts(source).await, expected);
    }
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = 'first'")
        .bind(source.as_uuid()).fetch_one(&pool).await?;
    assert_eq!(lifecycle, "ACTIVE");
    let url: String = sqlx::query_scalar(
        "SELECT url FROM product_listings WHERE listing_source_id = $1 AND source_listing_id = 'first'",
    )
    .bind(source.as_uuid())
    .fetch_one(&pool)
    .await?;
    assert_eq!(url, "https://example.test/first");
    Ok(())
    }.await;
    assert!(result.is_ok(), "{result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA, OPENSEARCH, &AURA_API])]
async fn rest_rejects_whole_request_and_checks_auth_even_for_empty_or_invalid_arrays() {
    let result: TestResult = async {
        let (source, token) = auth().await;
        let invalid_tail = format!("[{},{}", product("never-sent"), product("truncated"));
        for body in [
            "{ }",
            &invalid_tail,
            "[null,",
            "[null, {",
            &format!("[{}]", vec!["null"; 101].join(",")),
        ] {
            let response = send(
                source,
                Some(&token),
                reqwest::Method::POST,
                body,
                "bad-whole",
            )
            .await?;
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        }
        for body in ["[]", "[null]"] {
            assert_eq!(
                send(source, None, reqwest::Method::POST, body, "auth-required")
                    .await?
                    .status(),
                reqwest::StatusCode::UNAUTHORIZED
            );
        }
        let restricted =
            String::from(seed_access_token_for(seed_user("USER").await, HashSet::new()).await);
        for body in ["[]", "[null]"] {
            assert_eq!(
                send(
                    source,
                    Some(&restricted),
                    reqwest::Method::POST,
                    body,
                    "no-scope"
                )
                .await?
                .status(),
                reqwest::StatusCode::FORBIDDEN
            );
        }
        let response = send(
            source,
            Some(&token),
            reqwest::Method::POST,
            "[null]",
            "invalid-item",
        )
        .await?;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        assert_eq!(
            response.json::<Value>().await?["failures"],
            json!([{"index":0,"error":"BAD_BODY_VALUE","retryable":false}])
        );
        assert!(captured_ingestion_messages(source).is_empty());
        assert_eq!(counts(source).await, (0, 0, 0, 0));
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "{result:?}");
}
