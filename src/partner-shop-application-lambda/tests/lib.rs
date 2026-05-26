use common::shop_name::ShopName;
use common::user_id::UserId;
use fake::{Fake, Faker};
use lambda_runtime::{Context, LambdaEvent};
use notification::core::notification::Notification;
use notification::core::notification_id::NotificationId;
use notification::service::notification_service::MockNotificationService;
use partner_shop_application::core::partner_shop_application_id::PartnerShopApplicationId;
use partner_shop_application::dynamodb::partner_shop_application_payload_type_record::PartnerShopApplicationPayloadTypeRecord;
use partner_shop_application::dynamodb::partner_shop_application_record::PartnerShopApplicationRecord;
use partner_shop_application::dynamodb::partner_shop_application_state_record::PartnerShopApplicationStateRecord;
use partner_shop_application::dynamodb::repository::{
    PartnerShopApplicationDynamoDbRepository, PartnerShopApplicationDynamoDbRepositoryImpl,
};
use partner_shop_application_lambda::handler;
use shop::dynamodb::partner_status_record::ShopPartnerStatusRecord;
use shop::dynamodb::repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl};
use shop::dynamodb::shop_record::ShopRecord;
use shop::dynamodb::shop_type_record::ShopTypeRecord;
use shop::service::command_service::CommandShopServiceImpl;
use test_api::*;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fake_notification(user_id: UserId) -> Notification {
    Notification {
        user_id,
        origin_event_id: common::event_id::EventId::new(),
        notification_id: NotificationId::new(),
        notification_type: None,
        notification_payload: Faker.fake(),
        seen: false,
        external: true,
        created: time::OffsetDateTime::now_utc(),
        updated: time::OffsetDateTime::now_utc(),
    }
}

fn mock_notification_service() -> MockNotificationService {
    let mut mock = MockNotificationService::new();
    mock.expect_create_notification().returning(|_, cmd| {
        let notification = fake_notification(cmd.user_id);
        Box::pin(async move { Ok(notification) })
    });
    mock
}

async fn seed_application_record(
    repository: &PartnerShopApplicationDynamoDbRepositoryImpl<'_>,
    record: &PartnerShopApplicationRecord,
) {
    repository
        .put_partner_shop_application_record(record.clone())
        .await
        .unwrap();
}

async fn seed_shop_record(repository: &ShopDynamoDbRepositoryImpl<'_>, record: &ShopRecord) {
    repository.put_shop_record(record.clone()).await.unwrap();
}

// ---------------------------------------------------------------------------
// WAIT_FOR_REVIEW integration tests
// ---------------------------------------------------------------------------

#[localstack_test(services = [DynamoDB()])]
async fn should_set_state_to_in_review_and_store_task_token_for_wait_for_review() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = MockNotificationService::new();

    let mut record: PartnerShopApplicationRecord = Faker.fake();
    record.business_state = PartnerShopApplicationStateRecord::Submitted;
    record.task_token = None;
    let app_id = record.id;
    let user_id = record.applicant_user_id;

    seed_application_record(&partner_app_repo, &record).await;

    let payload = serde_json::json!({
        "step": "WAIT_FOR_REVIEW",
        "task_token": "integration-test-token-123",
        "partner_application_id": app_id.to_string(),
        "applicant_user_id": user_id.to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_ok());

    let updated = partner_app_repo
        .get_partner_shop_application_record(&user_id, &app_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated.business_state,
        PartnerShopApplicationStateRecord::InReview
    );
    assert_eq!(
        updated.task_token.as_deref(),
        Some("integration-test-token-123")
    );
}

// ---------------------------------------------------------------------------
// APPROVE integration tests
// ---------------------------------------------------------------------------

#[localstack_test(services = [DynamoDB()])]
async fn should_create_shop_and_approve_for_new_application() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = mock_notification_service();

    let mut record: PartnerShopApplicationRecord = Faker.fake();
    record.business_state = PartnerShopApplicationStateRecord::InReview;
    record.payload_type = PartnerShopApplicationPayloadTypeRecord::New;
    record.shop_name = Some(ShopName::from("Integration Test Shop"));
    record.shop_type = Some(ShopTypeRecord::CommercialDealer);
    record.shop_domains = Some(Default::default());
    record.shop_image = None;
    record.existing_shop_id = None;
    let app_id = record.id;
    let user_id = record.applicant_user_id;

    seed_application_record(&partner_app_repo, &record).await;

    let payload = serde_json::json!({
        "step": "APPROVE",
        "partner_application_id": app_id.to_string(),
        "applicant_user_id": user_id.to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_ok());

    let updated = partner_app_repo
        .get_partner_shop_application_record(&user_id, &app_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated.business_state,
        PartnerShopApplicationStateRecord::Approved
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_link_existing_shop_and_approve_for_existing_application() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = mock_notification_service();

    let existing_shop: ShopRecord = Faker.fake();
    let existing_shop_id = existing_shop.shop_id;
    seed_shop_record(&shop_repo, &existing_shop).await;

    let mut record: PartnerShopApplicationRecord = Faker.fake();
    record.business_state = PartnerShopApplicationStateRecord::InReview;
    record.payload_type = PartnerShopApplicationPayloadTypeRecord::Existing;
    record.existing_shop_id = Some(existing_shop_id);
    record.shop_name = None;
    record.shop_type = None;
    record.shop_domains = None;
    record.shop_image = None;
    let app_id = record.id;
    let user_id = record.applicant_user_id;

    seed_application_record(&partner_app_repo, &record).await;

    let payload = serde_json::json!({
        "step": "APPROVE",
        "partner_application_id": app_id.to_string(),
        "applicant_user_id": user_id.to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_ok());

    let updated = partner_app_repo
        .get_partner_shop_application_record(&user_id, &app_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated.business_state,
        PartnerShopApplicationStateRecord::Approved
    );

    let updated_shop = shop_repo
        .get_shop_record(&existing_shop_id)
        .await
        .unwrap()
        .unwrap();

    assert!(updated_shop.shop_partner_status == ShopPartnerStatusRecord::Partnered);
    // TODO: actually test linking
}

// ---------------------------------------------------------------------------
// REJECT integration tests
// ---------------------------------------------------------------------------

#[localstack_test(services = [DynamoDB()])]
async fn should_set_state_to_rejected_for_new_application_reject() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = mock_notification_service();

    let mut record: PartnerShopApplicationRecord = Faker.fake();
    record.business_state = PartnerShopApplicationStateRecord::InReview;
    record.payload_type = PartnerShopApplicationPayloadTypeRecord::New;
    record.shop_name = Some(ShopName::from("Rejected Shop"));
    record.shop_image = None;
    record.existing_shop_id = None;
    let app_id = record.id;
    let user_id = record.applicant_user_id;

    seed_application_record(&partner_app_repo, &record).await;

    let payload = serde_json::json!({
        "step": "REJECT",
        "partner_application_id": app_id.to_string(),
        "applicant_user_id": user_id.to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_ok());

    let updated = partner_app_repo
        .get_partner_shop_application_record(&user_id, &app_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated.business_state,
        PartnerShopApplicationStateRecord::Rejected
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_set_state_to_rejected_for_existing_application_reject() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = mock_notification_service();

    let existing_shop: ShopRecord = Faker.fake();
    let existing_shop_id = existing_shop.shop_id;
    seed_shop_record(&shop_repo, &existing_shop).await;

    let mut record: PartnerShopApplicationRecord = Faker.fake();
    record.business_state = PartnerShopApplicationStateRecord::InReview;
    record.payload_type = PartnerShopApplicationPayloadTypeRecord::Existing;
    record.existing_shop_id = Some(existing_shop_id);
    record.shop_name = None;
    record.shop_image = None;
    let app_id = record.id;
    let user_id = record.applicant_user_id;

    seed_application_record(&partner_app_repo, &record).await;

    let payload = serde_json::json!({
        "step": "REJECT",
        "partner_application_id": app_id.to_string(),
        "applicant_user_id": user_id.to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_ok());

    let updated = partner_app_repo
        .get_partner_shop_application_record(&user_id, &app_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        updated.business_state,
        PartnerShopApplicationStateRecord::Rejected
    );
}

// ---------------------------------------------------------------------------
// Error case integration tests
// ---------------------------------------------------------------------------

#[localstack_test(services = [DynamoDB()])]
async fn should_return_error_when_application_not_found_for_approve() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = MockNotificationService::new();

    let payload = serde_json::json!({
        "step": "APPROVE",
        "partner_application_id": PartnerShopApplicationId::new().to_string(),
        "applicant_user_id": UserId::new().to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_err());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_error_when_application_not_found_for_reject() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = MockNotificationService::new();

    let payload = serde_json::json!({
        "step": "REJECT",
        "partner_application_id": PartnerShopApplicationId::new().to_string(),
        "applicant_user_id": UserId::new().to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_err());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_return_error_when_task_token_missing_for_wait_for_review() {
    let partner_app_repo =
        PartnerShopApplicationDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_repo = ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let shop_service = CommandShopServiceImpl::new(
        &shop_repo,
        &shop::service::geocoding_service::NoopGeocodingService,
    );
    let mock_notification = MockNotificationService::new();

    let payload = serde_json::json!({
        "step": "WAIT_FOR_REVIEW",
        "partner_application_id": PartnerShopApplicationId::new().to_string(),
        "applicant_user_id": UserId::new().to_string(),
    });

    let event = LambdaEvent::new(payload, Context::default());
    let result = handler(
        &partner_app_repo,
        &shop_service,
        &shop_repo,
        &mock_notification,
        event,
    )
    .await;

    assert!(result.is_err());
}
