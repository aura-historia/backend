use common::language::domain::Language;
use common::language::record::{LanguageRecord, TextRecord};
use common::personalized::api::PersonalizedData;
use common::{pagination::cursor::api::TimeCursoredData, user_id::UserId};
use lambda_runtime::LambdaEvent;
use notification::dynamodb::repository::NotificationDynamoDbRepositoryImpl;
use notification::service::noop_adapters::{NoopS3Adapter, NoopSesAdapter};
use notification::service::notification_service::NotificationServiceImpl;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::{
    product_record::ProductRecord,
    repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product_personalization::service::ProductPersonalizationServiceImpl;
use product_watchlist::{
    dynamodb::record::{WatchlistProductRecord, mk_gsi1_pk, mk_gsi1_sk, mk_lsi1_sk, mk_pk, mk_sk},
    dynamodb::repository::{
        WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
    },
    service::product_watchlist_service::ProductWatchListServiceImpl,
};
use product_watchlist_api::watchlist_get::handle;
use test_api::*;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
use user::service::user_service::UserServiceImpl;

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc() {
    let client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
        "noreply@example.com".parse().unwrap(),
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service = ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository);

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
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
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

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(10, actual.size);
    assert_eq!(10, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_asc_search_after() {
    let client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
        "noreply@example.com".parse().unwrap(),
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service = ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository);

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
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
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

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(12, actual.size);
    assert_eq!(12, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(expected_next_after.unwrap(), actual.search_after.unwrap());
    assert_eq!(15, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc() {
    let client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
        "noreply@example.com".parse().unwrap(),
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service = ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository);

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
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
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

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
            .collect::<Vec<_>>()
    );
    assert_eq!(23, actual.total.unwrap());
}

#[localstack_test(services = [DynamoDB()])]
async fn should_200_when_sort_created_desc_search_after() {
    let client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
        "noreply@example.com".parse().unwrap(),
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service = ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository);

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
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: true,
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

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert_eq!(7, actual.size);
    assert_eq!(7, actual.items.len());
    assert_eq!(
        expected,
        actual
            .items
            .into_iter()
            .map(|item| item.item.product_id)
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
    "en",
    "English title",
    Language::En,
    "English description",
    Language::En
)]
#[case("fr", "French title", Language::Fr, "French description", Language::Fr)]
#[case(
    "es",
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
#[localstack_test(services = [DynamoDB()])]
async fn should_respond_200_and_respect_language_query_param(
    #[case] _language_query: &str,
    #[case] expected_title: &str,
    #[case] expected_title_lang: Language,
    #[case] expected_description: &str,
    #[case] expected_description_lang: Language,
) {
    let client = get_dynamodb_client().await;
    let product_repository = ProductDynamoDbRepositoryImpl::new(client, "table_1");
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(client, "table_1");
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(client, "table_1");
    let user_repository = UserDynamoDbRepositoryImpl::new(client, "table_1");
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let noop_ses = NoopSesAdapter;
    let noop_s3 = NoopS3Adapter;
    let user_service = UserServiceImpl::new(&user_repository);
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses,
        &noop_s3,
        "",
        "",
        "",
        "noreply@example.com".parse().unwrap(),
    );
    let personalization_service = ProductPersonalizationServiceImpl::new(
        &watchlist_repository,
        &notification_service,
        &user_service,
    );
    let service = ProductWatchListServiceImpl::new(&watchlist_repository, &product_repository);

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
        product_record.title_it = Some("Italian title".to_string());
        product_record.description_native = Some(TextRecord {
            text: "German description".to_string(),
            language: LanguageRecord::De,
        });
        product_record.description_de = Some("German description".to_string());
        product_record.description_en = Some("English description".to_string());
        product_record.description_fr = Some("French description".to_string());
        product_record.description_es = Some("Spanish description".to_string());
        product_record.description_it = Some("Italian description".to_string());
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
            gsi1_pk: mk_gsi1_pk(&product_record.product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            user_id,
            product_id: product_record.product_id,
            shop_id: product_record.shop_id,
            shops_product_id: product_record.shops_product_id,
            notifications: false,
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

    let response = handle(
        lambda_event,
        &service,
        &get_product_service,
        &personalization_service,
    )
    .await
    .unwrap();
    assert_eq!(200, response.status_code);

    let actual: TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>> =
        serde_json::from_value(extract_apigw_response_json_body!(response)).unwrap();
    assert!(actual.size > 0);
    assert!(!actual.items.is_empty());
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.item.title.text == expected_title)
    );
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.item.title.language == expected_title_lang.into())
    );
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.item.description.as_ref().unwrap().text == expected_description)
    );
    assert!(
        actual
            .items
            .iter()
            .all(|item| item.item.description.as_ref().unwrap().language
                == expected_description_lang.into())
    );
}
