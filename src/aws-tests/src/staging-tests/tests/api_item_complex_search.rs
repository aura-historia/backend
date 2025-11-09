use aws_tests_common::get_cfn_output;
use common::currency::record::CurrencyRecord;
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::query::range_query::RangeQuery;
use common::{
    currency::data::CurrencyData, event_id::EventId, item_id::ItemId, language::data::LanguageData,
    shop_id::ShopId, shops_item_id::ShopsItemId,
};
use fake::{Fake, Faker};
use item::data::item_search_data::ItemSearchData;
use item::data::item_state_data::ItemStateData;
use item::dynamodb::item_record::{self, ItemRecord};
use item::dynamodb::item_state_record::ItemStateRecord;
use item::dynamodb::repository::{ItemDynamoDbRepository, ItemDynamoDbRepositoryImpl};
use item::opensearch::{
    item_document::ItemDocument,
    item_state_document::ItemStateDocument,
    repository::{ItemOpenSearchRepository, ItemOpenSearchRepositoryImpl},
};
use item::service::get_service::GetItemServiceImpl;
use item::watchlist::dynamodb::repository::WatchlistItemDynamoDbRepositoryImpl;
use item::watchlist::service::item_watchlist_service::{
    ItemWatchListService, ItemWatchListServiceImpl,
};
use opensearch::{IndexParts, params::Refresh};
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
        WatchlistItemDynamoDbRepositoryImpl::new(dynamodb_client, &cfn.dynamodb_table_1_name);
    let item_repository =
        ItemDynamoDbRepositoryImpl::new(dynamodb_client, &cfn.dynamodb_table_1_name);
    let get_item_service = GetItemServiceImpl::new(&item_repository);
    let item_watchlist_service = ItemWatchListServiceImpl::new(
        &watchlist_repository,
        &user_repository,
        &item_repository,
        &get_item_service,
    );

    let now = SystemTime::now();
    let os_client = get_opensearch_client().await;
    let item_opensearch_repository = ItemOpenSearchRepositoryImpl::new(os_client);
    let expected = ItemDocument {
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shops_item_id: ShopsItemId::new(),
        shop_name: "Hans Volkers Shop".into(),
        title_de: Some("Chopin Etudes Op.10 1833".to_string()),
        title_en: None,
        description_de: None,
        description_en: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        state: ItemStateDocument::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        text_embedding: None,
        created: now.into(),
        updated: now.into(),
    };
    let mut all = fake::vec![ItemDocument; 10];
    all.push(expected.clone());

    let insert_res = item_opensearch_repository
        .create_item_documents(all)
        .await
        .unwrap();
    assert!(!insert_res.errors);
    os_client
        .index(IndexParts::Index("items"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let ddb_materialized = ItemRecord {
        pk: item_record::mk_pk(&expected.shop_id, &expected.shops_item_id),
        sk: item_record::mk_sk().to_owned(),
        item_id: expected.item_id,
        event_id: expected.event_id,
        shop_id: expected.shop_id,
        shops_item_id: expected.shops_item_id.clone(),
        shop_name: expected.shop_name.clone(),
        title_native: TextRecord {
            text: "Chopin Etudes Op.10 1833".to_owned(),
            language: LanguageRecord::De,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_owned()),
        title_en: None,
        description_native: None,
        description_de: None,
        description_en: None,
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
        state: ItemStateRecord::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        created: now.into(),
        updated: now.into(),
    };
    let ddb_batch_write_res = item_repository
        .put_item_records([ddb_materialized].into())
        .await
        .unwrap();
    assert!(
        ddb_batch_write_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let user = create_random_test_user().await;
    item_watchlist_service
        .create_watchlist_item(&user.sub.into(), &expected.shop_id, &expected.shops_item_id)
        .await
        .unwrap();

    let search_filter = ItemSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        item_query: "Chopin Etudes Op.10".try_into().unwrap(),
        shop_name_query: Some("Hans Volkers".try_into().unwrap()),
        price_query: Some(RangeQuery {
            min: None,
            max: Some(99999999),
        }),
        state_query: [ItemStateData::Available, ItemStateData::Listed].into(),
        created_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999 - 01 - 02 0:00 UTC)),
        }),
        updated_query: None,
    };

    let url = format!(
        "{}/api/v1/items/search?sort=created&order=asc&size=5",
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
    assert_eq!(expected.shops_item_id.to_string(), item["shopsItemId"]);
    assert_eq!(expected.item_id.to_string(), item["itemId"]);
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
    let repository = ItemOpenSearchRepositoryImpl::new(os_client);
    let expected = ItemDocument {
        item_id: ItemId::new(),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shops_item_id: ShopsItemId::new(),
        shop_name: "Hans Volkers Shop".into(),
        title_de: Some("Chopin Etudes Op.10 1833".to_string()),
        title_en: None,
        description_de: None,
        description_en: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        state: ItemStateDocument::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        text_embedding: None,
        created: SystemTime::now().into(),
        updated: SystemTime::now().into(),
    };
    let mut all = fake::vec![ItemDocument; 10];
    all.push(expected.clone());

    let insert_res = repository.create_item_documents(all).await.unwrap();
    assert!(!insert_res.errors);
    os_client
        .index(IndexParts::Index("items"))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search_filter = ItemSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        item_query: "Chopin Etudes Op.10".try_into().unwrap(),
        shop_name_query: Some("Hans Volkers".try_into().unwrap()),
        price_query: Some(RangeQuery {
            min: None,
            max: Some(99999999),
        }),
        state_query: [ItemStateData::Available, ItemStateData::Listed].into(),
        created_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999 - 01 - 02 0:00 UTC)),
        }),
        updated_query: None,
    };

    let url = format!(
        "{}/api/v1/items/search?sort=created&order=asc&size=5",
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
    assert_eq!(expected.shops_item_id.to_string(), item["shopsItemId"]);
    assert_eq!(expected.item_id.to_string(), item["itemId"]);
    assert_eq!(expected.event_id.to_string(), item["eventId"]);
    assert_eq!(expected.url.to_string(), item["url"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);
    assert!(body["items"].as_array().unwrap()[0]["userState"].is_null());
}

#[staging_test]
async fn should_respond_200_when_no_hits_anon() {
    let url = format!(
        "{}/api/v1/items/search",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&Faker.fake::<ItemSearchData>())
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
