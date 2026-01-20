use aws_tests_common::get_cfn_output;
use common::currency::record::CurrencyRecord;
use common::language::document::{LanguageDocument, TextDocument};
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::query::range_query::RangeQuery;
use common::slug_id::SlugId;
use common::{
    currency::data::CurrencyData, event_id::EventId, language::data::LanguageData,
    product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId,
};
use fake::{Fake, Faker};
use opensearch::{IndexParts, params::Refresh};
use product::data::product_search_data::ProductSearchData;
use product::data::product_state_data::ProductStateData;
use product::dynamodb::product_record::{self, ProductRecord};
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use product::opensearch::{
    product_document::ProductDocument,
    product_state_document::ProductStateDocument,
    repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
};
use product::service::get_service::GetProductServiceImpl;
use product::watchlist::dynamodb::repository::WatchlistProductDynamoDbRepositoryImpl;
use product::watchlist::service::product_watchlist_service::{
    ProductWatchListService, ProductWatchListServiceImpl,
};
use staging_tests::{
    create_random_test_user, get_dynamodb_client, get_opensearch_client, staging_test,
};
use std::{
    time::{Duration, SystemTime},
    vec,
};
use time::macros::datetime;
use url::Url;
use user::dynamodb::repository::UserDynamoDbRepositoryImpl;

#[staging_test]
async fn should_respond_200_when_hits_authenticated() {
    let cfn = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let user_repository =
        UserDynamoDbRepositoryImpl::new(dynamodb_client, &cfn.dynamodb_table_1_name);
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(dynamodb_client, &cfn.dynamodb_table_1_name);
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &cfn.dynamodb_table_1_name);
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &product_repository,
        &get_product_service,
    );

    let now = SystemTime::now();
    let os_client = get_opensearch_client().await;
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(os_client);
    let expected = ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("Foo"),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shop_type: Faker.fake(),
        shops_product_id: ShopsProductId::new(),
        shop_name: "Hans Volkers Shop".into(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_string()),
        title_en: None,
        title_fr: None,
        title_es: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        price_estimate_min_eur: Faker.fake(),
        price_estimate_min_usd: Faker.fake(),
        price_estimate_min_gbp: Faker.fake(),
        price_estimate_min_aud: Faker.fake(),
        price_estimate_min_cad: Faker.fake(),
        price_estimate_min_nzd: Faker.fake(),
        price_estimate_max_eur: Faker.fake(),
        price_estimate_max_usd: Faker.fake(),
        price_estimate_max_gbp: Faker.fake(),
        price_estimate_max_aud: Faker.fake(),
        price_estimate_max_cad: Faker.fake(),
        price_estimate_max_nzd: Faker.fake(),
        state: ProductStateDocument::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        auction_start: None,
        auction_end: None,
        created: now.into(),
        updated: now.into(),
    };
    let mut all = fake::vec![ProductDocument; 10];
    all.push(expected.clone());

    let insert_res = product_opensearch_repository
        .create_product_documents(all)
        .await
        .unwrap();
    assert!(!insert_res.errors);
    os_client
        .index(IndexParts::Index("products"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let ddb_materialized = ProductRecord {
        pk: product_record::mk_pk(&expected.shop_id, &expected.shops_product_id),
        sk: product_record::mk_sk().to_owned(),
        product_id: expected.product_id,
        product_slug_id: SlugId::from("Chopin Etudes Op.10 1833"),
        event_id: expected.event_id,
        shop_id: expected.shop_id,
        shops_product_id: expected.shops_product_id.clone(),
        shop_name: expected.shop_name.clone(),
        shop_type: Faker.fake(),
        title_native: TextRecord {
            text: "Chopin Etudes Op.10 1833".to_owned(),
            language: LanguageRecord::De,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_owned()),
        title_en: None,
        title_fr: None,
        title_es: None,
        description_native: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        price_native: Some(PriceRecord {
            currency: CurrencyRecord::Eur,
            amount: 1400000,
        }),
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        price_estimate_min_native: Faker.fake(),
        price_estimate_min_eur: Faker.fake(),
        price_estimate_min_usd: Faker.fake(),
        price_estimate_min_gbp: Faker.fake(),
        price_estimate_min_aud: Faker.fake(),
        price_estimate_min_cad: Faker.fake(),
        price_estimate_min_nzd: Faker.fake(),
        price_estimate_max_native: Faker.fake(),
        price_estimate_max_eur: Faker.fake(),
        price_estimate_max_usd: Faker.fake(),
        price_estimate_max_gbp: Faker.fake(),
        price_estimate_max_aud: Faker.fake(),
        price_estimate_max_cad: Faker.fake(),
        price_estimate_max_nzd: Faker.fake(),
        state: ProductStateRecord::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        auction_start: None,
        auction_end: None,
        created: now.into(),
        updated: now.into(),
    };
    let ddb_batch_write_res = product_repository
        .put_product_records([ddb_materialized].into())
        .await
        .unwrap();
    assert!(
        ddb_batch_write_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let user = create_random_test_user().await;
    product_watchlist_service
        .create_watchlist_product(
            &user.sub.into(),
            &expected.shop_id,
            &expected.shops_product_id,
        )
        .await
        .unwrap();

    let search_filter = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: "Chopin Etudes Op.10".try_into().unwrap(),
        shop_name_query: ["Hans Volkers Shop".into()].into(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: Some(RangeQuery {
            min: None,
            max: Some(99999999),
        }),
        state_query: [ProductStateData::Available, ProductStateData::Listed].into(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        auction_start_query: None,
        auction_end_query: None,
        created_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999-01-02 0:00 UTC)),
        }),
        updated_query: None,
    };

    let url = format!(
        "{}/api/v1/products/search?sort=created&order=asc&size=5",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&search_filter)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(1, body["size"]);
    assert_eq!(1, body["total"]);

    let item = body["items"].as_array().unwrap()[0]["item"].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(
        expected.shops_product_id.to_string(),
        item["shopsProductId"]
    );
    assert_eq!(expected.product_id.to_string(), item["productId"]);
    assert_eq!(expected.event_id.to_string(), item["eventId"]);
    assert_eq!(expected.url.to_string(), item["url"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);

    let user_state = body["items"].as_array().unwrap()[0]["userState"].clone();
    assert!(user_state["watchlist"]["watching"].as_bool().unwrap());
    assert!(!user_state["watchlist"]["notifications"].as_bool().unwrap());
}

#[staging_test]
async fn should_respond_200_when_hits_anon() {
    let os_client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(os_client);
    let expected = ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("Foo"),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shops_product_id: ShopsProductId::new(),
        shop_name: "Hans Volkers Shop".into(),
        shop_type: Faker.fake(),
        title_native: TextDocument {
            text: "Foo".to_string(),
            language: LanguageDocument::Fr,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_string()),
        title_en: None,
        title_fr: None,
        title_es: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        price_estimate_min_eur: Faker.fake(),
        price_estimate_min_usd: Faker.fake(),
        price_estimate_min_gbp: Faker.fake(),
        price_estimate_min_aud: Faker.fake(),
        price_estimate_min_cad: Faker.fake(),
        price_estimate_min_nzd: Faker.fake(),
        price_estimate_max_eur: Faker.fake(),
        price_estimate_max_usd: Faker.fake(),
        price_estimate_max_gbp: Faker.fake(),
        price_estimate_max_aud: Faker.fake(),
        price_estimate_max_cad: Faker.fake(),
        price_estimate_max_nzd: Faker.fake(),
        state: ProductStateDocument::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        text_embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
        auction_start: None,
        auction_end: None,
        created: SystemTime::now().into(),
        updated: SystemTime::now().into(),
    };
    let mut all = fake::vec![ProductDocument; 10];
    all.push(expected.clone());

    let insert_res = repository.create_product_documents(all).await.unwrap();
    assert!(!insert_res.errors);
    os_client
        .index(IndexParts::Index("products"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search_filter = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: "Chopin Etudes Op.10".try_into().unwrap(),
        shop_name_query: ["Hans Volkers Shop".into()].into(),
        exclude_shop_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: Some(RangeQuery {
            min: None,
            max: Some(99999999),
        }),
        state_query: [ProductStateData::Available, ProductStateData::Listed].into(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        auction_start_query: None,
        auction_end_query: None,
        created_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999 - 01 - 02 0:00 UTC)),
        }),
        updated_query: None,
    };

    let url = format!(
        "{}/api/v1/products/search?sort=created&order=asc&size=5",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&search_filter)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(1, body["size"]);
    assert_eq!(1, body["total"]);

    let item = body["items"].as_array().unwrap()[0]["item"].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(
        expected.shops_product_id.to_string(),
        item["shopsProductId"]
    );
    assert_eq!(expected.product_id.to_string(), item["productId"]);
    assert_eq!(expected.event_id.to_string(), item["eventId"]);
    assert_eq!(expected.url.to_string(), item["url"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);
    assert!(body["items"].as_array().unwrap()[0]["userState"].is_null());
}

#[staging_test]
async fn should_respond_200_when_no_hits_anon() {
    let url = format!(
        "{}/api/v1/products/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&Faker.fake::<ProductSearchData>())
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
