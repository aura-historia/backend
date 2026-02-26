use common::language::domain::Language;
use common::language::record::{LanguageRecord, TextRecord};
use common::{pagination::cursor::api::TimeCursoredData, user_id::UserId};
use fake::{Fake, Faker};
use lambda_runtime::LambdaEvent;
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product::watchlist::{
    dynamodb::record::{WatchlistProductRecord, mk_gsi1_pk, mk_gsi1_sk, mk_lsi1_sk, mk_pk, mk_sk},
    dynamodb::repository::{
        WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
    },
    service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_watchlist_api::watchlist_get::{WatchlistProductDataView, handle};
use test_api::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for product_record in product_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            gsi1_pk: None,
            gsi1_sk: None,
            user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .into_iter()
        .take(10)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", "2021-12-31T23:59:59Z")
            .query_string_parameter("size", "10")
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, actual.size);
    assert_eq!(10, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc_search_after() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    let mut from = None;
    let mut expected_next_after = None;
    for (i, product_record) in product_records.iter().cloned().enumerate() {
        let created = OffsetDateTime::now_utc();
        if i == 7 {
            from = Some(created);
        }
        if i == 19 {
            expected_next_after = Some(created);
        }
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            gsi1_pk: None,
            gsi1_sk: None,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .iter()
        .skip(8)
        .take(12)
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "asc")
            .query_string_parameter("searchAfter", from.unwrap().format(&Rfc3339).unwrap())
            .query_string_parameter("size", "12")
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(12, actual.size);
    assert_eq!(12, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
    assert_eq!(15, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for product_record in product_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            gsi1_pk: None,
            gsi1_sk: None,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .into_iter()
        .skip(16)
        .rev()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .query_string_parameter("searchAfter", "2999-12-31T23:59:59Z")
            .query_string_parameter("size", "7")
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc_search_after() {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let product_records = fake::vec![ProductRecord; 23];
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    let mut from = None;
    let mut expected_next_after = None;
    for (i, product_record) in product_records.iter().cloned().enumerate() {
        let created = OffsetDateTime::now_utc();
        if i == 7 {
            from = Some(created);
        }
        if i == 0 {
            expected_next_after = Some(created);
        }
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            user_id,
            gsi1_pk: Some(mk_gsi1_pk(&product_record.product_id)),
            gsi1_sk: Some(mk_gsi1_sk(&user_id)),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: true,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let expected = product_records
        .iter()
        .take(7)
        .rev()
        .map(|record| record.product_id)
        .collect::<Vec<_>>();
    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
            .query_string_parameter("language", "de")
            .query_string_parameter("currency", "EUR")
            .query_string_parameter("sort", "created")
            .query_string_parameter("order", "desc")
            .query_string_parameter("searchAfter", from.unwrap().format(&Rfc3339).unwrap())
            .query_string_parameter("size", "20")
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.product.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
    assert_eq!(7, actual.total.unwrap());
}

#[rstest::rstest]
#[trace]
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
async fn should_respond_200_and_respect_language_query_param(
    #[case] _language_query: &str,
    #[case] expected_title: &str,
    #[case] expected_title_lang: Language,
    #[case] expected_description: &str,
    #[case] expected_description_lang: Language,
) {
    let client = get_dynamodb_client().await;
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let mut product_records = fake::vec![ProductRecord; 23];
    for product_record in &mut product_records {
        product_record.title_native = TextRecord {
            text: "German title".to_string(),
            language: LanguageRecord::De,
        };
        product_record.title_de = Some("German title".to_string());
        product_record.title_en = Some("English title".to_string());
        product_record.title_fr = Some("French title".to_string());
        product_record.title_es = Some("Spanish title".to_string());
        product_record.description_native = Some(TextRecord {
            text: "German description".to_string(),
            language: LanguageRecord::De,
        });
        product_record.description_de = Some("German description".to_string());
        product_record.description_en = Some("English description".to_string());
        product_record.description_fr = Some("French description".to_string());
        product_record.description_es = Some("Spanish description".to_string());
    }
    let put_res = product_repository
        .put_product_records(product_records.clone().try_into().unwrap())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    let user_id = UserId::new();
    for product_record in product_records.clone() {
        let created = OffsetDateTime::now_utc();
        let watchlist_record = WatchlistProductRecord {
            pk: mk_pk(&user_id),
            sk: mk_sk(&product_record.shop_id, &product_record.shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created).unwrap(),
            gsi1_pk: None,
            gsi1_sk: None,
            user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
            user_record: Faker.fake(),
            created,
            updated: created,
        };
        watchlist_repository
            .put_watchlist_record(watchlist_record)
            .await
            .unwrap();
    }

    let lambda_event = LambdaEvent {
        payload: ApiGatewayV2httpRequestProxy::builder()
            .http_method(http::Method::GET)
            .jwt_claim("sub", user_id)
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
            .build(),
        context: Default::default(),
    };

    let response = handle(lambda_event, &service).await.unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<WatchlistProductDataView> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert!(actual.size > 0);
    assert!(!actual.items.is_empty());
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.product.title.text == expected_title)
    );
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.product.title.language == expected_title_lang.into())
    );
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.product.description.as_ref().unwrap().text == expected_description)
    );
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.product.description.as_ref().unwrap().language
                == expected_description_lang.into())
    );
}
