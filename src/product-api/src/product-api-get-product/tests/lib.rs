use cognito::access_token_verifier_service::MockAccessTokenVerifierService;
use common::{
    currency::domain::Currency,
    event::Event,
    event_id::EventId,
    language::domain::Language,
    personalized::api::PersonalizedData,
    price::domain::{FixedFxRate, FxRate, Price},
    product_state::domain::ProductState,
};
use fake::{Fake, Faker};
use http::header::ACCEPT_LANGUAGE;
use lambda_runtime::LambdaEvent;
use product::{
    core::product_event::{
        ProductEventPayload, ProductPriceChangeEventPayload, ProductStateChangeEventPayload,
    },
    data::{get_data::GetProductData, user_state_data::ProductUserStateData},
    service::personalization_service::ProductPersonalizationServiceImpl,
    watchlist::service::{
        command::UpdateWatchlistProductCommand, product_watchlist_service::ProductWatchListService,
    },
};
use product::{
    dynamodb::{
        product_record::ProductRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
    },
    watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl,
};
use product::{
    service::get_service::GetProductServiceImpl,
    watchlist::service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_api_get_product::handler;
use std::time::{Duration, SystemTime};
use test_api::*;
use user::dynamodb::{
    repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
    user_record::UserRecord,
};

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_with_history_when_anon_and_history_flag_true() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);
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
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
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
        }),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
            [
                event_1.clone().try_into().unwrap(),
                event_2.clone().try_into().unwrap(),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "true")
            .query_string_parameter("currency", "USD")
            .build(),
        context: Default::default(),
    };
    let response = handler(
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
    let history = body["item"]["history"].as_array().unwrap();
    assert_eq!(2, history.len());
    assert_eq!(event_1_id.to_string(), history[0]["eventId"]);
    assert_eq!("PRICE_DROPPED", history[0]["eventType"]);
    assert_eq!("USD", history[0]["payload"]["newPrice"]["currency"]);
    assert_eq!(
        u64::from(
            event_1_price
                .into_exchanged(&FixedFxRate(), Currency::Usd)
                .unwrap()
                .monetary_amount
        ),
        history[0]["payload"]["newPrice"]["amount"]
    );
    assert_eq!(event_2_id.to_string(), history[1]["eventId"]);
    assert_eq!("STATE_REMOVED", history[1]["eventType"]);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_without_history_when_anon_and_history_flag_false() {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);
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
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
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
        }),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
            [
                event_1.clone().try_into().unwrap(),
                event_2.clone().try_into().unwrap(),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
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
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(move |_| Box::pin(async move { Ok(Some(user_record.id)) }));

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
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
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
        }),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
            [
                event_1.clone().try_into().unwrap(),
                event_2.clone().try_into().unwrap(),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body["item"]["history"].is_null());
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
        &user_repository,
        &product_repository,
        &get_product_service,
    );
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);

    let user_record = Faker.fake::<UserRecord>();
    let _ = user_repository
        .put_user_record(user_record.clone())
        .await
        .unwrap();
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(move |_| Box::pin(async move { Ok(Some(user_record.id)) }));

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let _ = watchlist_service
        .create_watchlist_product(&user_record.id, &record.shop_id, &record.shops_product_id)
        .await
        .unwrap();
    let _ = watchlist_service
        .update_watchlist_product(
            &user_record.id,
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
        payload: ProductEventPayload::PriceDropped(ProductPriceChangeEventPayload {
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
        }),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::StateRemoved(ProductStateChangeEventPayload {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id.clone(),
            old_state: ProductState::Sold,
        }),
    };
    let insert_res = product_repository
        .put_product_event_records(
            [
                event_1.clone().try_into().unwrap(),
                event_2.clone().try_into().unwrap(),
            ]
            .into(),
        )
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .query_string_parameter("history", "false")
            .build(),
        context: Default::default(),
    };
    let response = handler(
        lambda_event,
        &get_product_service,
        &access_token_verifier_service,
        &product_personalization_service,
    )
    .await
    .unwrap();

    assert_eq!(200, response.status_code);

    let body = extract_apigw_response_json_body!(response);
    assert!(body["item"]["history"].is_null());
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

#[trace]
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
#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_and_respect_accept_language_header(
    #[case] accept_language_header: &str,
    #[case] expected_title: &str,
    #[case] expected_title_lang: Language,
    #[case] expected_description: &str,
    #[case] expected_description_lang: Language,
) {
    let ddb_client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_personalization_service =
        ProductPersonalizationServiceImpl::new(&watchlist_repository);
    let mut access_token_verifier_service = MockAccessTokenVerifierService::default();
    access_token_verifier_service
        .expect_verify_extract_user_id()
        .return_once(|_| Box::pin(async { Ok(None) }));

    let mut record = Faker.fake::<ProductRecord>();
    record.title_de = Some("German title".to_string());
    record.title_en = Some("English title".to_string());
    record.title_fr = Some("French title".to_string());
    record.title_es = Some("Spanish title".to_string());
    record.description_de = Some("German description".to_string());
    record.description_en = Some("English description".to_string());
    record.description_fr = Some("French description".to_string());
    record.description_es = Some("Spanish description".to_string());
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .header(ACCEPT_LANGUAGE.as_str(), accept_language_header)
            .path_parameter("shopId", record.shop_id)
            .path_parameter("shopsProductId", record.shops_product_id)
            .build(),
        context: Default::default(),
    };
    let response = handler(
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
