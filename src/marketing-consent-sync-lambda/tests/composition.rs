use application::transaction::{Transaction, UnitOfWork};
use marketing_consent_sync_lambda::compose_marketing_consent_sync_use_case;
use platform_postgres::SqlxUnitOfWork;
use serde_email::Email;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use user_loops::LoopsNewsletterConfig;
use user_postgres::SqlxMarketingConsentIntentRepository;
use user_service::{
    use_cases::SyncMarketingConsentIntentResult,
    use_cases::commands::coordinate_marketing_consent::MarketingConsentCoordinator,
};
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path, query_param},
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const CONTACT_ID: &str = "contact-1939";
const LIST_ID: &str = "newsletter-test-list";

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn real_postgres_and_fake_loops_finalize_once_and_ignore_late_replay() {
    let pool = get_postgres_client().await;
    let loops = MockServer::start().await;
    let address: Email =
        Email::try_from(format!("consent-{}@example.test", uuid::Uuid::now_v7())).unwrap();
    let address_value: &str = address.as_ref();
    let find_reads = Arc::new(AtomicUsize::new(0));
    let before_email = address.to_string();
    let after_email = address.to_string();
    let suppression_email = address.to_string();
    let before_reads = Arc::clone(&find_reads);
    Mock::given(method("GET"))
        .and(path("/api/v1/contacts/find"))
        .and(query_param("email", address_value))
        .respond_with(move |_: &wiremock::Request| {
            let first = before_reads.fetch_add(1, Ordering::SeqCst) == 0;
            let (global, on_list) = if first { (false, false) } else { (true, true) };
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "id": CONTACT_ID,
                "email": if first { &before_email } else { &after_email },
                "subscribed": global,
                "mailingLists": { LIST_ID: on_list },
                "optInStatus": "accepted"
            }]))
        })
        .expect(2)
        .mount(&loops)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/contacts/suppression"))
        .and(query_param("email", address_value))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "contact": { "id": CONTACT_ID, "email": suppression_email },
                "isSuppressed": false,
                "removalQuota": { "limit": 5, "remaining": 5 }
            }))
        })
        .expect(2)
        .mount(&loops)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/v1/contacts/update"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "success": true,
            "id": CONTACT_ID
        })))
        .expect(1)
        .mount(&loops)
        .await;

    let config = LoopsNewsletterConfig::new(
        "synthetic-test-key".into(),
        LIST_ID.into(),
        format!("{}/api", loops.uri()),
    )
    .unwrap();
    let use_case = compose_marketing_consent_sync_use_case(pool.clone(), config).unwrap();
    let uow = SqlxUnitOfWork::new(pool.clone());
    let repository = SqlxMarketingConsentIntentRepository::new();
    let mut tx = uow.begin().await.unwrap();
    let intent_id = MarketingConsentCoordinator::new(&mut tx, &repository)
        .accepted_double_opt_in(
            format!("composition-proof-{}", uuid::Uuid::now_v7()),
            address,
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(matches!(
        use_case.execute(intent_id).await,
        Ok(SyncMarketingConsentIntentResult::Applied)
    ));
    let status: String = sqlx::query_scalar(
        "SELECT status FROM marketing_email_consent_sync_intents WHERE intent_id = $1",
    )
    .bind(intent_id.as_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!("APPLIED", status);

    // Model a queue/archive replay arriving outside FIFO's five-minute deduplication window.
    sqlx::query(
        "UPDATE marketing_email_consent_sync_intents SET completed_at = clock_timestamp() - interval '6 minutes' WHERE intent_id = $1",
    )
    .bind(intent_id.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        use_case.execute(intent_id).await,
        Ok(SyncMarketingConsentIntentResult::Applied)
    ));
    let updates = loops.received_requests().await.unwrap();
    assert_eq!(
        1,
        updates
            .iter()
            .filter(|request| request.method == "PUT")
            .count()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn definite_loops_rejection_releases_the_same_intent_for_retry() {
    let pool = get_postgres_client().await;
    let loops = MockServer::start().await;
    let address: Email =
        Email::try_from(format!("retry-{}@example.test", uuid::Uuid::now_v7())).unwrap();
    let address_value: &str = address.as_ref();
    let email_value = address.to_string();
    let find_reads = Arc::new(AtomicUsize::new(0));
    let before_reads = Arc::clone(&find_reads);
    let find_email = email_value.clone();
    Mock::given(method("GET"))
        .and(path("/api/v1/contacts/find"))
        .and(query_param("email", address_value))
        .respond_with(move |_: &wiremock::Request| {
            let read = before_reads.fetch_add(1, Ordering::SeqCst);
            let (global, on_list) = if read < 2 {
                (false, false)
            } else {
                (true, true)
            };
            ResponseTemplate::new(200).set_body_json(serde_json::json!([{
                "id": CONTACT_ID,
                "email": find_email,
                "subscribed": global,
                "mailingLists": { LIST_ID: on_list },
                "optInStatus": "accepted"
            }]))
        })
        .expect(3)
        .mount(&loops)
        .await;
    let suppression_email = email_value.clone();
    Mock::given(method("GET"))
        .and(path("/api/v1/contacts/suppression"))
        .and(query_param("email", address_value))
        .respond_with(move |_: &wiremock::Request| {
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "contact": { "id": CONTACT_ID, "email": suppression_email },
                "isSuppressed": false,
                "removalQuota": { "limit": 5, "remaining": 5 }
            }))
        })
        .expect(3)
        .mount(&loops)
        .await;
    let update_calls = Arc::new(AtomicUsize::new(0));
    let update_count = Arc::clone(&update_calls);
    Mock::given(method("PUT"))
        .and(path("/api/v1/contacts/update"))
        .respond_with(move |_: &wiremock::Request| {
            if update_count.fetch_add(1, Ordering::SeqCst) == 0 {
                ResponseTemplate::new(429)
            } else {
                ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "success": true,
                    "id": CONTACT_ID
                }))
            }
        })
        .expect(2)
        .mount(&loops)
        .await;

    let config = LoopsNewsletterConfig::new(
        "synthetic-test-key".into(),
        LIST_ID.into(),
        format!("{}/api", loops.uri()),
    )
    .unwrap();
    let use_case = compose_marketing_consent_sync_use_case(pool.clone(), config).unwrap();
    let uow = SqlxUnitOfWork::new(pool);
    let repository = SqlxMarketingConsentIntentRepository::new();
    let mut tx = uow.begin().await.unwrap();
    let intent_id = MarketingConsentCoordinator::new(&mut tx, &repository)
        .accepted_double_opt_in(
            format!("retry-composition-proof-{}", uuid::Uuid::now_v7()),
            address,
            OffsetDateTime::now_utc(),
        )
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(matches!(
        use_case.execute(intent_id).await,
        Ok(SyncMarketingConsentIntentResult::Retryable)
    ));
    assert!(matches!(
        use_case.execute(intent_id).await,
        Ok(SyncMarketingConsentIntentResult::Applied)
    ));
    assert_eq!(2, update_calls.load(Ordering::SeqCst));
}
