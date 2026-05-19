use common::batch::Batch;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use fake::{Fake, Faker};
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use product::dynamodb::product_event_record::{self, ProductEventRecord};
use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
use product::dynamodb::product_record::{self, ProductRecord};
use product::dynamodb::product_state_record::ProductStateRecord;
use product::dynamodb::product_update_record::ProductRecordUpdate;
use product::dynamodb::repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl};
use test_api::*;
use time::OffsetDateTime;
use url::Url;

async fn get_repository() -> ProductDynamoDbRepositoryImpl<'static> {
    ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_records_for_single_record() {
    let expected = Faker.fake::<ProductRecord>();

    get_repository()
        .await
        .put_product_records(Batch::from([expected.clone()]))
        .await
        .unwrap();

    let actual = get_repository()
        .await
        .get_product_record(&expected.shop_id, &expected.shops_product_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_records_for_multiple_records() {
    let now1 = OffsetDateTime::now_utc();
    let shop_id = ShopId::new();
    let shops_product_id_1: ShopsProductId = "123465".into();
    let expected1 = ProductRecord {
        pk: format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id_1}"),
        sk: "product#materialized".to_string(),
        gsi2_pk: Faker.fake(),
        gsi2_sk: Faker.fake(),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: EventId::new(),
        shop_id,
        seller_id: Faker.fake(),
        shops_product_id: shops_product_id_1.clone(),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: Faker.fake(),
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        title_it: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        price_native: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateRecord::Available,
        url: Url::parse("https://foo.bar/123456").unwrap(),
        view_url: Url::parse("https://foo.bar/123456").unwrap(),
        images: Faker.fake(),
        embedding: Some(fake::vec![f32; 768]),
        auction_start: None,
        auction_end: None,
        created: now1,
        updated: now1,
    };
    let shops_product_id_2: ShopsProductId = "abcdefg".into();
    let now2 = OffsetDateTime::now_utc();
    let expected2 = ProductRecord {
        pk: format!("product#shop_id#{shop_id}#shops_product_id#{shops_product_id_2}"),
        sk: "product#materialized".to_string(),
        gsi2_pk: Faker.fake(),
        gsi2_sk: Faker.fake(),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: EventId::new(),
        shop_id,
        seller_id: Faker.fake(),
        shops_product_id: shops_product_id_2.clone(),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: Faker.fake(),
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        title_it: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        price_native: Some(PriceRecord {
            amount: 110,
            currency: CurrencyRecord::Eur,
        }),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateRecord::Available,
        url: Url::parse("https://foo.bar/123456").unwrap(),
        view_url: Url::parse("https://foo.bar/123456").unwrap(),
        images: Faker.fake(),
        embedding: Some(fake::vec![f32; 768]),
        auction_start: None,
        auction_end: None,
        created: now2,
        updated: now2,
    };

    get_repository()
        .await
        .put_product_records([expected1.clone(), expected2.clone()].into())
        .await
        .unwrap();

    let actual1 = get_repository()
        .await
        .get_product_record(&shop_id, &shops_product_id_1)
        .await
        .unwrap()
        .unwrap();
    let actual2 = get_repository()
        .await
        .get_product_record(&shop_id, &shops_product_id_2)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected1, actual1);
    assert_eq!(expected2, actual2);
}

#[rstest::rstest]
#[test_attr(apply(test))]
#[case(ProductEventRecord::Domain(Faker.fake()))]
#[case(ProductEventRecord::Enrichment(Faker.fake()))]
#[case(ProductEventRecord::Policy(Faker.fake()))]
#[trace]
#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_event_records_for_single_record(#[case] expected: ProductEventRecord) {
    get_repository()
        .await
        .put_product_event_records(Batch::from([expected.clone()]))
        .await
        .unwrap();

    let actual = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .into_iter()
        .map(serde_dynamo::from_item)
        .collect::<Result<Vec<ProductEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_event_records_for_multiple_records() {
    let shop_id = ShopId::new();
    let now1 = OffsetDateTime::now_utc();
    let event_id1 = EventId::new();
    let shops_product_id1: ShopsProductId = "123465".into();
    let price = Some(PriceRecord {
        amount: 110,
        currency: CurrencyRecord::Eur,
    });
    let expected1 = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&shop_id, &shops_product_id1),
        sk: product_event_record::domain::mk_sk(&event_id1),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: event_id1,
        event_type: ProductDomainEventTypeRecord::DomainCreated,
        event_type_schema_version: 0,
        shop_id,
        seller_id: Faker.fake(),
        shops_product_id: shops_product_id1.clone(),
        shop_name: Some("Foo".to_string()),
        seller_name: Some("Foo".to_string()),
        shop_type: Faker.fake(),
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        title_it: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        new_price_native: price,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        old_price_native: None,
        old_price_eur: None,
        old_price_usd: None,
        old_price_gbp: None,
        old_price_aud: None,
        old_price_cad: None,
        old_price_nzd: None,
        old_price_cny: None,
        old_price_brl: None,
        old_price_pln: None,
        old_price_try: None,
        old_price_jpy: None,
        old_price_czk: None,
        old_price_rub: None,
        old_price_aed: None,
        old_price_sar: None,
        old_price_hkd: None,
        old_price_sgd: None,
        old_price_chf: None,
        new_state: Some(ProductStateRecord::Available),
        old_state: Some(ProductStateRecord::Listed),
        url: Some(Url::parse("https://foo.bar/123456").unwrap()),
        view_url: None,
        images: Faker.fake(),
        auction_start: None,
        auction_end: None,
        timestamp: now1,
        new_price_nzd: None,
        new_price_cny: None,
        new_price_brl: None,
        new_price_pln: None,
        new_price_try: None,
        new_price_jpy: None,
        new_price_czk: None,
        new_price_rub: None,
        new_price_aed: None,
        new_price_sar: None,
        new_price_hkd: None,
        new_price_sgd: None,
        new_price_chf: None,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_min_cny: None,
        new_price_estimate_min_brl: None,
        new_price_estimate_min_pln: None,
        new_price_estimate_min_try: None,
        new_price_estimate_min_jpy: None,
        new_price_estimate_min_czk: None,
        new_price_estimate_min_rub: None,
        new_price_estimate_min_aed: None,
        new_price_estimate_min_sar: None,
        new_price_estimate_min_hkd: None,
        new_price_estimate_min_sgd: None,
        new_price_estimate_min_chf: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        new_price_estimate_max_cny: None,
        new_price_estimate_max_brl: None,
        new_price_estimate_max_pln: None,
        new_price_estimate_max_try: None,
        new_price_estimate_max_jpy: None,
        new_price_estimate_max_czk: None,
        new_price_estimate_max_rub: None,
        new_price_estimate_max_aed: None,
        new_price_estimate_max_sar: None,
        new_price_estimate_max_hkd: None,
        new_price_estimate_max_sgd: None,
        new_price_estimate_max_chf: None,
    };

    let now2 = OffsetDateTime::now_utc();
    let event_id2 = EventId::new();
    let shops_product_id2: ShopsProductId = "123465".into();
    let expected2 = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&shop_id, &shops_product_id2),
        sk: product_event_record::domain::mk_sk(&event_id2),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: event_id2,
        event_type: ProductDomainEventTypeRecord::DomainCreated,
        event_type_schema_version: 0,
        shop_id,
        seller_id: Faker.fake(),
        shops_product_id: shops_product_id2.clone(),
        shop_name: Some("Foo".to_string()),
        seller_name: Some("Bar".to_string()),
        shop_type: Faker.fake(),
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        title_it: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        new_price_native: price,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        new_price_nzd: None,
        new_price_cny: None,
        new_price_brl: None,
        new_price_pln: None,
        new_price_try: None,
        new_price_jpy: None,
        new_price_czk: None,
        new_price_rub: None,
        new_price_aed: None,
        new_price_sar: None,
        new_price_hkd: None,
        new_price_sgd: None,
        new_price_chf: None,
        old_price_native: None,
        old_price_eur: None,
        old_price_usd: None,
        old_price_gbp: None,
        old_price_aud: None,
        old_price_cad: None,
        old_price_nzd: None,
        old_price_cny: None,
        old_price_brl: None,
        old_price_pln: None,
        old_price_try: None,
        old_price_jpy: None,
        old_price_czk: None,
        old_price_rub: None,
        old_price_aed: None,
        old_price_sar: None,
        old_price_hkd: None,
        old_price_sgd: None,
        old_price_chf: None,
        new_state: Some(ProductStateRecord::Available),
        old_state: Some(ProductStateRecord::Listed),
        url: Some(Url::parse("https://foo.bar/123456").unwrap()),
        view_url: None,
        images: Faker.fake(),
        auction_start: None,
        auction_end: None,
        timestamp: now2,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_min_cny: None,
        new_price_estimate_min_brl: None,
        new_price_estimate_min_pln: None,
        new_price_estimate_min_try: None,
        new_price_estimate_min_jpy: None,
        new_price_estimate_min_czk: None,
        new_price_estimate_min_rub: None,
        new_price_estimate_min_aed: None,
        new_price_estimate_min_sar: None,
        new_price_estimate_min_hkd: None,
        new_price_estimate_min_sgd: None,
        new_price_estimate_min_chf: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
        new_price_estimate_max_cny: None,
        new_price_estimate_max_brl: None,
        new_price_estimate_max_pln: None,
        new_price_estimate_max_try: None,
        new_price_estimate_max_jpy: None,
        new_price_estimate_max_czk: None,
        new_price_estimate_max_rub: None,
        new_price_estimate_max_aed: None,
        new_price_estimate_max_sar: None,
        new_price_estimate_max_hkd: None,
        new_price_estimate_max_sgd: None,
        new_price_estimate_max_chf: None,
    };

    get_repository()
        .await
        .put_product_event_records(Batch::from([
            expected1.clone().into(),
            expected2.clone().into(),
        ]))
        .await
        .unwrap();

    let actual = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap()
        .into_iter()
        .map(serde_dynamo::from_item)
        .collect::<Result<Vec<ProductDomainEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected1, expected2], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_update_product_record() {
    let now = OffsetDateTime::now_utc();
    let shop_id = ShopId::new();
    let shops_product_id: ShopsProductId = "123465".into();
    let price = PriceRecord {
        amount: 110,
        currency: CurrencyRecord::Eur,
    };
    let initial = ProductRecord {
        pk: product_record::mk_pk(&shop_id, &shops_product_id),
        sk: product_record::mk_sk().to_string(),
        gsi2_pk: Faker.fake(),
        gsi2_sk: Faker.fake(),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        seller_slug_id: Faker.fake(),
        event_id: EventId::new(),
        shop_id,
        seller_id: Faker.fake(),
        shops_product_id: shops_product_id.clone(),
        shop_name: "Foo".to_string(),
        seller_name: "Bar".to_string(),
        shop_type: Faker.fake(),
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        title_it: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        price_native: Some(price),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        state: ProductStateRecord::Available,
        url: Url::parse("https://foo.bar/123456").unwrap(),
        view_url: Url::parse("https://foo.bar/123456").unwrap(),
        images: Faker.fake(),
        embedding: Some(fake::vec![f32; 768]),
        auction_start: None,
        auction_end: None,
        created: now,
        updated: now,
    };
    let now2 = OffsetDateTime::now_utc();
    let event_id2 = EventId::new();
    let update = ProductRecordUpdate {
        event_id: Some(event_id2),
        price_native: None,
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_cny: None,
        price_brl: None,
        price_pln: None,
        price_try: None,
        price_jpy: None,
        price_czk: None,
        price_rub: None,
        price_aed: None,
        price_sar: None,
        price_hkd: None,
        price_sgd: None,
        price_chf: None,
        state: Some(ProductStateRecord::Sold),
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        images: None,
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_min_cny: None,
        price_estimate_min_brl: None,
        price_estimate_min_pln: None,
        price_estimate_min_try: None,
        price_estimate_min_jpy: None,
        price_estimate_min_czk: None,
        price_estimate_min_rub: None,
        price_estimate_min_aed: None,
        price_estimate_min_sar: None,
        price_estimate_min_hkd: None,
        price_estimate_min_sgd: None,
        price_estimate_min_chf: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        price_estimate_max_cny: None,
        price_estimate_max_brl: None,
        price_estimate_max_pln: None,
        price_estimate_max_try: None,
        price_estimate_max_jpy: None,
        price_estimate_max_czk: None,
        price_estimate_max_rub: None,
        price_estimate_max_aed: None,
        price_estimate_max_sar: None,
        price_estimate_max_hkd: None,
        price_estimate_max_sgd: None,
        price_estimate_max_chf: None,
        url: None,
        view_url: None,
        auction_start: None,
        auction_end: None,
        embedding: None,
        updated: now2,
    };
    let mut expected = initial.clone();
    expected.event_id = event_id2;
    expected.state = ProductStateRecord::Sold;
    expected.updated = now2;

    get_repository()
        .await
        .put_product_records(Batch::from([initial.clone()]))
        .await
        .unwrap();
    get_repository()
        .await
        .update_product_record(&shop_id, &shops_product_id, update)
        .await
        .unwrap();

    let actual = get_repository()
        .await
        .get_product_record(&shop_id, &shops_product_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(expected, actual);
}

// ---------------------------------------------------------------------------
// transact_write_product_create
// ---------------------------------------------------------------------------

#[localstack_test(services = [DynamoDB()])]
async fn should_write_event_and_product_record_atomically_when_product_does_not_exist() {
    let repo = get_repository().await;

    let product_record: ProductRecord = Faker.fake();
    // Build a matching domain event record whose pk/sk/product_id align with the product record.
    let event_id = product_record.event_id;
    let domain_event: ProductDomainEventRecord = Faker.fake();
    let domain_event = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(
            &product_record.shop_id,
            &product_record.shops_product_id,
        ),
        sk: product_event_record::domain::mk_sk(&event_id),
        event_id,
        shop_id: product_record.shop_id,
        shops_product_id: product_record.shops_product_id.clone(),
        ..domain_event
    };
    let event_record = ProductEventRecord::Domain(domain_event);

    repo.transact_write_product_create(event_record.clone(), product_record.clone())
        .await
        .unwrap();

    // Product record should be present.
    let stored_product = repo
        .get_product_record(&product_record.shop_id, &product_record.shops_product_id)
        .await
        .unwrap()
        .expect("product record must exist after transact_write_product_create");
    assert_eq!(product_record.event_id, stored_product.event_id);

    // Event record should be present.
    let all_items = get_dynamodb_client()
        .await
        .scan()
        .table_name("table_1")
        .send()
        .await
        .unwrap()
        .items
        .unwrap_or_default();
    let event_count = all_items
        .iter()
        .filter(|item| item.contains_key("event_type"))
        .count();
    assert_eq!(1, event_count, "exactly one event record must be written");
}

#[localstack_test(services = [DynamoDB()])]
async fn should_fail_transact_write_create_when_product_already_exists() {
    let repo = get_repository().await;

    let product_record: ProductRecord = Faker.fake();
    // Pre-write the product record so the condition attribute_not_exists(pk) fails.
    repo.put_product_records(Batch::from([product_record.clone()]))
        .await
        .unwrap();

    let domain_event: ProductDomainEventRecord = Faker.fake();
    let domain_event = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(
            &product_record.shop_id,
            &product_record.shops_product_id,
        ),
        sk: product_event_record::domain::mk_sk(&domain_event.event_id),
        shop_id: product_record.shop_id,
        shops_product_id: product_record.shops_product_id.clone(),
        ..domain_event
    };
    let event_record = ProductEventRecord::Domain(domain_event);

    let result = repo
        .transact_write_product_create(event_record, product_record.clone())
        .await;

    assert!(
        result.is_err(),
        "transact_write_product_create must fail when product already exists"
    );
}

// ---------------------------------------------------------------------------
// transact_write_product_update
// ---------------------------------------------------------------------------

#[localstack_test(services = [DynamoDB()])]
async fn should_write_event_and_update_product_record_atomically_when_event_id_matches() {
    let repo = get_repository().await;

    let initial: ProductRecord = Faker.fake();
    let expected_event_id = initial.event_id;
    repo.put_product_records(Batch::from([initial.clone()]))
        .await
        .unwrap();

    let new_event_id = EventId::new();
    let domain_event: ProductDomainEventRecord = Faker.fake();
    let domain_event = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&initial.shop_id, &initial.shops_product_id),
        sk: product_event_record::domain::mk_sk(&new_event_id),
        event_id: new_event_id,
        shop_id: initial.shop_id,
        shops_product_id: initial.shops_product_id.clone(),
        new_state: Some(ProductStateRecord::Sold),
        ..domain_event
    };
    let event_record = ProductEventRecord::Domain(domain_event);

    let now = time::OffsetDateTime::now_utc();
    let update = ProductRecordUpdate {
        event_id: Some(new_event_id),
        state: Some(ProductStateRecord::Sold),
        updated: now,
        ..ProductRecordUpdate::default()
    };

    let product_key = ProductKey {
        shop_id: initial.shop_id,
        shops_product_id: initial.shops_product_id.clone(),
    };
    repo.transact_write_product_update(
        vec![event_record],
        update,
        product_key.clone(),
        expected_event_id,
    )
    .await
    .unwrap();

    let stored = repo
        .get_product_record(&initial.shop_id, &initial.shops_product_id)
        .await
        .unwrap()
        .expect("product record must exist after transact_write_product_update");
    assert_eq!(new_event_id, stored.event_id);
    assert_eq!(ProductStateRecord::Sold, stored.state);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_fail_transact_write_update_when_event_id_does_not_match() {
    let repo = get_repository().await;

    let initial: ProductRecord = Faker.fake();
    repo.put_product_records(Batch::from([initial.clone()]))
        .await
        .unwrap();

    // Use a wrong expected_event_id so the condition `event_id = :expected_event_id` fails.
    let wrong_event_id = EventId::new();
    let new_event_id = EventId::new();
    let domain_event: ProductDomainEventRecord = Faker.fake();
    let domain_event = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&initial.shop_id, &initial.shops_product_id),
        sk: product_event_record::domain::mk_sk(&new_event_id),
        event_id: new_event_id,
        shop_id: initial.shop_id,
        shops_product_id: initial.shops_product_id.clone(),
        new_state: Some(ProductStateRecord::Sold),
        ..domain_event
    };

    let update = ProductRecordUpdate {
        event_id: Some(new_event_id),
        state: Some(ProductStateRecord::Sold),
        updated: time::OffsetDateTime::now_utc(),
        ..ProductRecordUpdate::default()
    };

    let product_key = ProductKey {
        shop_id: initial.shop_id,
        shops_product_id: initial.shops_product_id.clone(),
    };
    let result = repo
        .transact_write_product_update(
            vec![ProductEventRecord::Domain(domain_event)],
            update,
            product_key,
            wrong_event_id, // wrong — triggers ConditionalCheckFailed
        )
        .await;

    assert!(
        result.is_err(),
        "transact_write_product_update must fail when expected_event_id does not match stored event_id"
    );

    // Product record must remain unchanged.
    let stored = repo
        .get_product_record(&initial.shop_id, &initial.shops_product_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(initial.event_id, stored.event_id);
    assert_eq!(initial.state, stored.state);
}
