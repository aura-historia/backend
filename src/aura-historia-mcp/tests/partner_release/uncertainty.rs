use super::support::*;
use super::{SCHEMA, aura_integration_test};
use serde_json::json;
use test_api::IntegrationTestService;

#[aura_integration_test(services=[SCHEMA],isolated=true)]
async fn should_preserve_partial_admission_uncertainty_and_exact_retry_identity_through_real_http()
{
    let pool = test_api::get_postgres_client().await;
    let owner = actor(&pool, SCOPES).await;
    let source = source(&pool, &owner, "Publisher fixture").await;
    let server = Server::start(pool).await;
    let client = server.client(Some(&owner.token), discover()).await;
    let tools = catalogue(&client).await;
    let mut input = batch(
        source.source,
        vec![
            product("accepted"),
            product(""),
            product("unknown"),
            product("rejected"),
            product("not-attempted"),
        ],
    );
    input["idempotencyKey"] = json!("retain-complete-ordered-batch");
    let report = invoke(
        &client,
        &tools,
        "postAsyncPartnerProductListings",
        input.clone(),
    )
    .await;
    assert_eq!(5, report["originalInputCount"]);
    assert_eq!(1, report["acceptedCount"]);
    assert_eq!("retain-complete-ordered-batch", report["idempotencyKey"]);
    assert_eq!(
        vec![
            "PUBLISHED",
            "REJECTED",
            "UNKNOWN",
            "REJECTED",
            "NOT_ATTEMPTED"
        ],
        report["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["outcome"].as_str().unwrap())
            .collect::<Vec<_>>()
    );
    for (index, item) in report["items"].as_array().unwrap().iter().enumerate() {
        assert_eq!(index, item["index"]);
    }
    assert_eq!(true, report["items"][2]["retryable"]);
    assert_eq!(false, report["items"][3]["retryable"]);
    assert!(report.get("jobStatus").is_none());
    let first = server.publisher.last();
    assert_eq!(
        vec![0, 2, 3, 4],
        first
            .iter()
            .map(|message| message.metadata.index)
            .collect::<Vec<_>>()
    );
    assert!(
        first
            .iter()
            .all(|message| message.metadata.input_count == 5)
    );
    invoke(
        &client,
        &tools,
        "postAsyncPartnerProductListings",
        input.clone(),
    )
    .await;
    let second = server.publisher.last();
    assert_eq!(
        first
            .iter()
            .map(|m| &m.metadata.command_id)
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|m| &m.metadata.command_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(2, server.publisher.count());
    *server.publisher.fail_before_sending.lock().unwrap() = true;
    let failed = invoke(&client, &tools, "postAsyncPartnerProductListings", input).await;
    assert_eq!(0, failed["acceptedCount"]);
    assert_eq!(
        "PRODUCT_LISTING_INGESTION_INTERNAL_ERROR",
        failed["items"][0]["error"]
    );
    assert!(
        failed["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["outcome"] == "REJECTED")
    );
    assert!(!failed.to_string().contains("PRIVATE_PUBLISHER_CAUSE"));
    assert_eq!(3, server.publisher.attempts());
    assert_eq!(2, server.publisher.count());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM product_listings WHERE listing_source_id=$1")
            .bind(source.source.as_uuid())
            .fetch_one(&server.pool)
            .await
            .unwrap();
    assert_eq!(0, count);
    client.cancel().await.unwrap();
}
