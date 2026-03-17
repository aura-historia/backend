use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::{
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    language::{domain::Language, record::TextRecord},
    personalized::api::PersonalizedData,
    price::domain::{FixedFxRate, FxRate, Price},
    product_state::domain::ProductState,
};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use notification::service::notification_service::MockNotificationService;
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product::{
    core::product_event::{
        ProductEventPayload,
        domain::{
            ProductDomainEventPayload, ProductPriceChangeDomainEventPayload,
            ProductStateChangeDomainEventPayload,
        },
    },
    data::{get_data::GetProductData, user_state_data::ProductUserStateData},
};
use product_api::get_product::handle;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::{
    dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl,
    service::{
        command::UpdateWatchlistProductCommand,
        product_watchlist_service::{ProductWatchListService, ProductWatchListServiceImpl},
    },
};
use std::time::{Duration, SystemTime};
use test_api::*;
use user::dynamodb::{
    repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    user_record::UserRecord,
};
use user::service::user_service::UserServiceImpl;

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_without_history_when_anon() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceDropped(
            ProductPriceChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                new_native_price: event_1_price,
                new_other_price: FixedFxRate()
                    .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                    .unwrap(),
                old_native_price: Price {
                    monetary_amount: 100000u64.into(),
                    currency: Currency::Eur,
                },
                old_other_price: FixedFxRate()
                    .exchange_all(Currency::Eur, 100000u64.into())
                    .unwrap(),
            },
        )),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateRemoved(
            ProductStateChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                old_state: ProductState::Sold,
            },
        )),
    };
    let insert_res = product_repository
        .put_product_event_records([event_1.clone().into(), event_2.clone().into()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .build(),
        context: Default::default(),
    };
    let response = handle(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body.get("userState").is_none());
    assert!(body["item"]["history"].is_null())
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_personalized_when_authenticated_and_not_watched() {
    let ddb_client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(move |_| Box::pin(async move { Ok(Some(user_record.user_id)) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceDropped(
            ProductPriceChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                new_native_price: event_1_price,
                new_other_price: FixedFxRate()
                    .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                    .unwrap(),
                old_native_price: Price {
                    monetary_amount: 100000u64.into(),
                    currency: Currency::Eur,
                },
                old_other_price: FixedFxRate()
                    .exchange_all(Currency::Eur, 100000u64.into())
                    .unwrap(),
            },
        )),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateRemoved(
            ProductStateChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                old_state: ProductState::Sold,
            },
        )),
    };
    let insert_res = product_repository
        .put_product_event_records([event_1.clone().into(), event_2.clone().into()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .build(),
        context: Default::default(),
    };
    let response = handle(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(
        !body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_personalized_when_authenticated_and_watched() {
    let ddb_client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &product_repository,
        &get_product_service,
    );
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(move |_| Box::pin(async move { Ok(Some(user_record.user_id)) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let _ = watchlist_service
        .create_watchlist_product(
            &user_record.user_id,
            &record.shop_id,
            &record.shops_product_id,
        )
        .await
        .unwrap();
    let _ = watchlist_service
        .update_watchlist_product(
            &user_record.user_id,
            &record.shop_id,
            &record.shops_product_id,
            UpdateWatchlistProductCommand {
                notifications: Some(true),
            },
        )
        .await
        .unwrap();

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceDropped(
            ProductPriceChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                new_native_price: event_1_price,
                new_other_price: FixedFxRate()
                    .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                    .unwrap(),
                old_native_price: Price {
                    monetary_amount: 100000u64.into(),
                    currency: Currency::Eur,
                },
                old_other_price: FixedFxRate()
                    .exchange_all(Currency::Eur, 100000u64.into())
                    .unwrap(),
            },
        )),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateRemoved(
            ProductStateChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                old_state: ProductState::Sold,
            },
        )),
    };
    let insert_res = product_repository
        .put_product_event_records([event_1.clone().into(), event_2.clone().into()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .build(),
        context: Default::default(),
    };
    let response = handle(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(
        body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case("de", "German title", Language::De, "German description", Language::De)]
#[case(
    "de-DE",
    "German title",
    Language::De,
    "German description",
    Language::De
)]
#[case(
    "de-AT",
    "German title",
    Language::De,
    "German description",
    Language::De
)]
#[case(
    "de;q=1.0",
    "German title",
    Language::De,
    "German description",
    Language::De
)]
#[case(
    "de-DE,de;q=0.9,en;q=0.8",
    "German title",
    Language::De,
    "German description",
    Language::De
)]
#[case(
    "en;q=0.5,de;q=1.0",
    "German title",
    Language::De,
    "German description",
    Language::De
)]
#[case(
    "de,*;q=0.1",
    "German title",
    Language::De,
    "German description",
    Language::De
)]
#[case(
    "en",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case(
    "en-US",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case(
    "en-GB",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case(
    "en;q=0.7",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case(
    "fr;q=0.3,en;q=0.9",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case(
    "zh,ko;q=0.5,en;q=0.6",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case(
    "*,en;q=0.8",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case("fr", "French title", Language::Fr, "French description", Language::Fr)]
#[case(
    "fr-FR",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "fr-CA",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "fr;q=1.0",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "fr,en;q=0.4",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "fr-BE,fr;q=0.9",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "es;q=0.2,de;q=0.4,fr;q=0.8",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "*,fr;q=0.7",
    "French title",
    Language::Fr,
    "French description",
    Language::Fr
)]
#[case(
    "es",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "es-ES",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "es-MX",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "es;q=1.0",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "es,en;q=0.3",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "es-AR,es;q=0.9",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "fr;q=0.1,de;q=0.2,es;q=0.6",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "*,es;q=0.5",
    "Spanish title",
    Language::Es,
    "Spanish description",
    Language::Es
)]
#[case(
    "it",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "it-IT",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "it-CH",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "it;q=1.0",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "it,en;q=0.3",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "it-IT,it;q=0.9",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "de;q=0.1,fr;q=0.2,it;q=0.6",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[case(
    "*,it;q=0.5",
    "Italian title",
    Language::It,
    "Italian description",
    Language::It
)]
#[trace]
#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_and_respect_language_query_param(
    #[case] _language_query: &str,
    #[case] expected_title: &str,
    #[case] expected_title_lang: Language,
    #[case] expected_description: &str,
    #[case] expected_description_lang: Language,
) {
    use common::language::record::LanguageRecord;

    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let mut record = Faker.fake::<ProductRecord>();
    record.title_native = TextRecord {
        text: "German title".to_string(),
        language: LanguageRecord::De,
    };
    record.title_de = Some("German title".to_string());
    record.title_en = Some("English title".to_string());
    record.title_fr = Some("French title".to_string());
    record.title_es = Some("Spanish title".to_string());
    record.title_it = Some("Italian title".to_string());
    record.description_native = Some(TextRecord {
        text: "German description".to_string(),
        language: LanguageRecord::De,
    });
    record.description_de = Some("German description".to_string());
    record.description_en = Some("English description".to_string());
    record.description_fr = Some("French description".to_string());
    record.description_es = Some("Spanish description".to_string());
    record.description_it = Some("Italian description".to_string());
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/shops/{shopId}/products/{shopsProductId}".to_owned())
            .query_string_parameter(
                "language",
                match expected_title_lang {
                    Language::De => "de",
                    Language::En => "en",
                    Language::Fr => "fr",
                    Language::Es => "es",
                    Language::It => "it",
                },
            )
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .build(),
        context: Default::default(),
    };
    let response = handle(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    let actual: PersonalizedData<GetProductData, ProductUserStateData> =
        serde_json::from_value(body).unwrap();

    assert_eq!(expected_title, actual.item.title.text);
    assert_eq!(expected_title_lang, actual.item.title.language.into());
    assert_eq!(
        expected_description,
        actual.item.description.as_ref().unwrap().text
    );
    assert_eq!(
        expected_description_lang,
        actual.item.description.as_ref().unwrap().language.into()
    );
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_for_path_params_slugs() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let user_repository = UserDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = MockNotificationService::default();
    let product_personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .route_key("GET /api/v1/by-slug/shops/{shopSlugId}/products/{productSlugId}".to_owned())
            .path_parameter("shopSlugId", record.shop_slug_id)
            .path_parameter("productSlugId", record.product_slug_id)
            .build(),
        context: Default::default(),
    };
    let response = handle(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    let actual: PersonalizedData<GetProductData, ProductUserStateData> =
        serde_json::from_value(body).unwrap();

    assert_eq!(record.product_id, actual.item.product_id);
    assert_eq!(record.shop_id, actual.item.shop_id);
    assert_eq!(record.shops_product_id, actual.item.shops_product_id);
}
