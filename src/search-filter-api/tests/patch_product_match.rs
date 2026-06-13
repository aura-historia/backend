use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::user_id::UserId;
use common::user_search_filter_id::UserSearchFilterId;
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::service::get_service::MockGetProductService;
use product::service::query_service::MockQueryProductService;
use product_personalization::service::MockProductPersonalizationService;
use search_filter::data::search_filter_product_match_data::SearchFilterProductMatchData;
use search_filter::dynamodb::repository::{
    UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
};
use search_filter::dynamodb::user_search_filter_match_record::{
    UserSearchFilterMatchRecord, mk_lsi1_sk, mk_pk, mk_sk,
};
use search_filter::service::user_search_filter_service::UserSearchFilterServiceImpl;
use search_filter_api::handler;
use search_filter_api::patch_product_match::PatchUserSearchFilterMatchData;
use test_api::*;
use time::OffsetDateTime;
use user::service::user_service::UserService;

fn user_ctx(user_id: common::user_id::UserId) -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::User(user_id),
    }
}

async fn seed_match_record(
    repository: &impl UserSearchFilterDynamoDbRepository,
    user_id: UserId,
    filter_id: UserSearchFilterId,
    shop_id: ShopId,
    shops_product_id: ShopsProductId,
    feedback: Option<bool>,
) {
    let created = OffsetDateTime::now_utc();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&created);
    record.user_id = user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id;
    record.feedback = feedback;
    record.created = created;
    record.updated = created;
    repository
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();
}

async fn patch_existing_match(
    patch: PatchUserSearchFilterMatchData,
    initial_feedback: Option<bool>,
) -> (i64, Option<bool>, Option<bool>) {
    let client = get_dynamodb_client().await;
    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let query_product_service = MockQueryProductService::default();
    let personalization_service = MockProductPersonalizationService::default();
    let user = user_service
        .create_user(&user_ctx(common::user_id::UserId::new()), Faker.fake())
        .await
        .unwrap();
    let filter_id = UserSearchFilterId::new();
    let shop_id = ShopId::new();
    let shops_product_id = ShopsProductId::new();
    seed_match_record(
        &repository,
        user.user_id,
        filter_id,
        shop_id,
        shops_product_id.clone(),
        initial_feedback,
    )
    .await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{shopId}/{shopsProductId}")
            .jwt_claim("sub", user.user_id)
            .path_parameter("userSearchFilterId", filter_id)
            .path_parameter("shopId", shop_id)
            .path_parameter("shopsProductId", shops_product_id.clone())
            .body_serde(&patch)
            .build(),
        context: Default::default(),
    };

    let response = handler(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    let status_code = response.status_code;
    let body: SearchFilterProductMatchData =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    let updated = repository
        .get_user_search_filter_match_record(&user.user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap()
        .unwrap();

    (status_code, body.feedback, updated.feedback)
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_setting_search_filter_product_match_feedback_true() {
    let (status_code, response_feedback, persisted_feedback) = patch_existing_match(
        PatchUserSearchFilterMatchData {
            feedback: Some(true),
        },
        None,
    )
    .await;

    assert_eq!(200, status_code);
    assert_eq!(Some(true), response_feedback);
    assert_eq!(Some(true), persisted_feedback);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_setting_search_filter_product_match_feedback_false() {
    let (status_code, response_feedback, persisted_feedback) = patch_existing_match(
        PatchUserSearchFilterMatchData {
            feedback: Some(false),
        },
        None,
    )
    .await;

    assert_eq!(200, status_code);
    assert_eq!(Some(false), response_feedback);
    assert_eq!(Some(false), persisted_feedback);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_patching_search_filter_product_match_without_feedback() {
    let (status_code, response_feedback, persisted_feedback) =
        patch_existing_match(PatchUserSearchFilterMatchData::default(), Some(true)).await;

    assert_eq!(200, status_code);
    assert_eq!(Some(true), response_feedback);
    assert_eq!(Some(true), persisted_feedback);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_404_when_search_filter_product_match_not_found() {
    let repository =
        UserSearchFilterDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1");
    let user_repository = user::dynamodb::repository::UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        "table_1",
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);
    let get_product_service = MockGetProductService::default();
    let query_product_service = MockQueryProductService::default();
    let personalization_service = MockProductPersonalizationService::default();
    let user = user_service
        .create_user(&user_ctx(common::user_id::UserId::new()), Faker.fake())
        .await
        .unwrap();

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::PATCH)
            .route_key("PATCH /api/v1/me/search-filters/{userSearchFilterId}/matches/{shopId}/{shopsProductId}")
            .jwt_claim("sub", user.user_id)
            .path_parameter("userSearchFilterId", UserSearchFilterId::new())
            .path_parameter("shopId", ShopId::new())
            .path_parameter("shopsProductId", ShopsProductId::new())
            .body_serde(&PatchUserSearchFilterMatchData {
                feedback: Some(false),
            })
            .build(),
        context: Default::default(),
    };

    let response = handler(
        lambda_event,
        &service,
        &get_product_service,
        &query_product_service,
        None,
        &personalization_service,
    )
    .await
    .unwrap();
    let json = extract_apigw_response_json_body!(response);

    assert_eq!(404, response.status_code);
    assert_eq!(404, json["status"]);
    assert_eq!("SEARCH_FILTER_NOT_FOUND", json["error"]);
}
