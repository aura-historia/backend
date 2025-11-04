use aws_tests_common::get_cfn_output;
use common::query::range_query::RangeQuery;
use common::{
    currency::data::CurrencyData, event_id::EventId, item_id::ItemId, language::data::LanguageData,
    shop_id::ShopId, shops_item_id::ShopsItemId,
};
use fake::{Fake, Faker};
use item::data::item_search_data::ItemSearchData;
use item::data::item_state_data::ItemStateData;
use item::opensearch::{
    item_document::ItemDocument,
    item_state_document::ItemStateDocument,
    repository::{ItemOpenSearchRepository, ItemOpenSearchRepositoryImpl},
};
use opensearch::{IndexParts, params::Refresh};
use staging_tests::{get_opensearch_client, staging_test};
use std::{
    time::{Duration, SystemTime},
    vec,
};
use time::macros::datetime;
use url::Url;

#[staging_test]
async fn should_respond_200_when_hits() {
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
        embedding: None,
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

    let item = body["items"].as_array().unwrap()[0].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(expected.shops_item_id.to_string(), item["shopsItemId"]);
    assert_eq!(expected.item_id.to_string(), item["itemId"]);
    assert_eq!(expected.event_id.to_string(), item["eventId"]);
    assert_eq!(expected.url.to_string(), item["url"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);
}

#[staging_test]
async fn should_respond_200_when_no_hits() {
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
