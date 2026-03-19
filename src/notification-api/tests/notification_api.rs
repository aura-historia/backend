use common::{event_id::EventId, user_id::UserId};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::{
    data::{
        get_notification_data::GetNotificationData, patch_notification_data::PatchNotificationData,
    },
    dynamodb::{
        notification_record::{NotificationRecord, mk_pk},
        repository::{NotificationDynamoDbRepository, NotificationDynamoDbRepositoryImpl},
    },
    service::{
        noop_adapters::{NoopS3Adapter, NoopSesAdapter},
        notification_service::NotificationServiceImpl,
    },
};
use notification_api::{
    notification_delete_all::handle as delete_all_handle,
    notification_delete_one::handle as delete_one_handle,
    notification_get::{EventIdCursoredData, handle as get_handle},
    notification_patch_all::handle as patch_all_handle,
    notification_patch_one::handle as patch_one_handle,
};
use test_api::*;
use user::{
    dynamodb::{
        repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
        user_record::UserRecord,
    },
    service::user_service::UserServiceImpl,
};

static NOOP_SES: NoopSesAdapter = NoopSesAdapter;
static NOOP_S3: NoopS3Adapter = NoopS3Adapter;

fn sender_email() -> serde_email::Email {
    "noreply@example.com".try_into().unwrap()
}

async fn seed_record(
    user_id: UserId,
    seen: bool,
    repository: &dyn NotificationDynamoDbRepository,
) -> NotificationRecord {
    let mut record: NotificationRecord = Faker.fake();
    record.pk = mk_pk(&user_id);
    record.user_id = user_id;
    record.seen = seen;
    repository
        .put_notification_record(record.clone())
        .await
        .unwrap();
    record
}

// ── GET /api/v1/me/notifications ────────────────────────────────────────────

#[localstack_test(services = [DynamoDB()])]
async fn should_200_with_empty_list_when_no_notifications() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = get_handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let body: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(0, body.items.len());
    assert_eq!(Some(0), body.total);
    assert!(body.search_after.is_none());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_with_correct_total_when_user_record_also_exists() {
    // Regression test: count_notification_records previously had no lower-bound on the sort
    // key, so it also counted non-notification records that share the same partition key
    // (e.g. user records with sk = "user#details"). This caused total to be +1.
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();

    // Seed a user record (sk = "user#details") in the same partition – this simulates
    // a real user created via Cognito post-confirmation.
    let mut user_record: UserRecord = Faker.fake();
    user_record.pk = user::dynamodb::user_record::mk_pk(&user_id);
    user_record.user_id = user_id;
    user_repository.put_user_record(user_record).await.unwrap();

    // Seed 2 notifications.
    seed_record(user_id, false, &notification_repository).await;
    seed_record(user_id, false, &notification_repository).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = get_handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let body: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(2, body.items.len());
    assert_eq!(Some(2), body.total);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_with_seeded_notifications() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    let record1 = seed_record(user_id, false, &notification_repository).await;
    let record2 = seed_record(user_id, false, &notification_repository).await;
    let record3 = seed_record(user_id, false, &notification_repository).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .build(),
        context: Default::default(),
    };

    let response = get_handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let body: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(3, body.items.len());
    assert_eq!(Some(3), body.total);

    let returned_ids: Vec<EventId> = body.items.iter().map(|n| n.origin_event_id).collect();
    assert!(returned_ids.contains(&record1.origin_event_id));
    assert!(returned_ids.contains(&record2.origin_event_id));
    assert!(returned_ids.contains(&record3.origin_event_id));
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_with_cursor_pagination() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    for _ in 0..5 {
        seed_record(user_id, false, &notification_repository).await;
    }

    // First page: size=2
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("size", "2")
            .build(),
        context: Default::default(),
    };

    let response = get_handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let first_page: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(2, first_page.items.len());
    assert_eq!(2, first_page.size);
    assert!(first_page.search_after.is_some());
    assert_eq!(Some(5), first_page.total);

    // Second page using search_after cursor
    let search_after = first_page.search_after.unwrap();
    let lambda_event2 = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("size", "2")
            .query_string_parameter("searchAfter", search_after)
            .build(),
        context: Default::default(),
    };

    let response2 = get_handle(lambda_event2, &service).await.unwrap();

    assert_eq!(200, response2.status_code);
    let second_page: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response2)).unwrap();
    assert_eq!(2, second_page.items.len());
    // Pages must not overlap
    let first_ids: Vec<EventId> = first_page.items.iter().map(|n| n.origin_event_id).collect();
    let second_ids: Vec<EventId> = second_page
        .items
        .iter()
        .map(|n| n.origin_event_id)
        .collect();
    assert!(first_ids.iter().all(|id| !second_ids.contains(id)));
}

// ── PATCH /api/v1/me/notifications/{eventId} ────────────────────────────────

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_patch_one_updates_seen() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    let record = seed_record(user_id, false, &notification_repository).await;
    assert!(!record.seen);

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", user_id)
            .path_parameter("eventId", record.origin_event_id)
            .body_serde(&PatchNotificationData { seen: Some(true) })
            .build(),
        context: Default::default(),
    };

    let response = patch_one_handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let data: GetNotificationData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(record.origin_event_id, data.origin_event_id);
    assert!(data.seen);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_404_when_patch_one_notification_not_found() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", UserId::new())
            .path_parameter("eventId", EventId::new())
            .body_serde(&PatchNotificationData { seen: Some(true) })
            .build(),
        context: Default::default(),
    };

    let err = patch_one_handle(lambda_event, &service).await.unwrap_err();
    assert_eq!(404, err.status);
}

// ── PATCH /api/v1/me/notifications ──────────────────────────────────────────

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_patch_all_marks_all_seen() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    let record1 = seed_record(user_id, false, &notification_repository).await;
    let record2 = seed_record(user_id, false, &notification_repository).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", user_id)
            .body_serde(&PatchNotificationData { seen: Some(true) })
            .build(),
        context: Default::default(),
    };

    let response = patch_all_handle(lambda_event, &service).await.unwrap();

    assert_eq!(200, response.status_code);
    let page: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(2, page.items.len());
    assert!(page.items.iter().all(|n| n.seen));

    let returned_ids: Vec<EventId> = page.items.iter().map(|n| n.origin_event_id).collect();
    assert!(returned_ids.contains(&record1.origin_event_id));
    assert!(returned_ids.contains(&record2.origin_event_id));
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_patch_all_without_body() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    seed_record(user_id, false, &notification_repository).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = patch_all_handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);
}

// ── DELETE /api/v1/me/notifications/{eventId} ───────────────────────────────

#[localstack_test(services = [DynamoDB()])]
async fn should_204_when_delete_one_removes_notification() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    let record = seed_record(user_id, false, &notification_repository).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .jwt_claim("sub", user_id)
            .path_parameter("eventId", record.origin_event_id)
            .build(),
        context: Default::default(),
    };

    let response = delete_one_handle(lambda_event, &service).await.unwrap();
    assert_eq!(204, response.status_code);

    // Verify the record is gone
    let get_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };
    let get_response = get_handle(get_event, &service).await.unwrap();
    let body: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(get_response)).unwrap();
    assert_eq!(0, body.items.len());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_404_when_delete_one_notification_not_found() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .jwt_claim("sub", UserId::new())
            .path_parameter("eventId", EventId::new())
            .build(),
        context: Default::default(),
    };

    let err = delete_one_handle(lambda_event, &service).await.unwrap_err();
    assert_eq!(404, err.status);
}

// ── DELETE /api/v1/me/notifications ─────────────────────────────────────────

#[localstack_test(services = [DynamoDB()])]
async fn should_204_when_delete_all_removes_all_notifications() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let user_id = UserId::new();
    for _ in 0..3 {
        seed_record(user_id, false, &notification_repository).await;
    }

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };

    let response = delete_all_handle(lambda_event, &service).await.unwrap();
    assert_eq!(204, response.status_code);

    // Verify all records are gone
    let get_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .build(),
        context: Default::default(),
    };
    let get_response = get_handle(get_event, &service).await.unwrap();
    let body: EventIdCursoredData<GetNotificationData> =
        serde_json::from_value(extract_apigw_response_json_body!(get_response)).unwrap();
    assert_eq!(0, body.items.len());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_204_when_delete_all_with_no_notifications() {
    let client = get_dynamodb_client().await;
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &NOOP_SES,
        &NOOP_S3,
        "",
        "",
        "",
        sender_email(),
    );

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::DELETE)
            .jwt_claim("sub", UserId::new())
            .build(),
        context: Default::default(),
    };

    let response = delete_all_handle(lambda_event, &service).await.unwrap();
    assert_eq!(204, response.status_code);
}
