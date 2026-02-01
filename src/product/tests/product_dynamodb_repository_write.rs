use common::batch::Batch;
use common::currency::record::CurrencyRecord;
use common::event_id::EventId;
use common::language::record::{LanguageRecord, TextRecord};
use common::price::record::PriceRecord;
use common::product_id::ProductId;
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use fake::{Fake, Faker};
use product::dynamodb::product_event_record;
use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
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
        event_id: EventId::new(),
        shop_id,
        shops_product_id: shops_product_id_1.clone(),
        shop_name: "Foo".to_string(),
        shop_type: Faker.fake(),
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        description_fr: Some("Bazzz".to_string()),
        description_es: Some("Bazzzz".to_string()),
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
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        state: ProductStateRecord::Available,
        url: Url::parse("https://foo.bar/123456").unwrap(),
        images: Faker.fake(),
        text_embedding: Faker.fake(),
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
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
        event_id: EventId::new(),
        shop_id,
        shops_product_id: shops_product_id_2.clone(),
        shop_name: "Foo".to_string(),
        shop_type: Faker.fake(),
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        description_fr: Some("Bazzz".to_string()),
        description_es: Some("Bazzzz".to_string()),
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
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        state: ProductStateRecord::Available,
        url: Url::parse("https://foo.bar/123456").unwrap(),
        images: Faker.fake(),
        text_embedding: Faker.fake(),
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
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

#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_event_records_for_single_record() {
    let now = OffsetDateTime::now_utc();
    let shop_id = ShopId::new();
    let shops_product_id: ShopsProductId = "123465".into();
    let price = Some(PriceRecord {
        amount: 110,
        currency: CurrencyRecord::Eur,
    });
    let expected = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&shop_id, &shops_product_id),
        sk: product_event_record::domain::mk_sk(&now).unwrap(),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        event_id: EventId::new(),
        event_type: ProductDomainEventTypeRecord::DomainCreated,
        event_type_schema_version: 0,
        shop_id,
        shops_product_id: shops_product_id.clone(),
        shop_name: Some("Foo".to_string()),
        shop_type: Faker.fake(),
        title_native: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        description_fr: Some("Bazzz".to_string()),
        description_es: Some("Bazzzz".to_string()),
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
        new_state: Some(ProductStateRecord::Available),
        old_state: Some(ProductStateRecord::Listed),
        url: Some(Url::parse("https://foo.bar/123456").unwrap()),
        images: Faker.fake(),
        auction_start: None,
        auction_end: None,
        timestamp: now,
        new_price_nzd: None,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
    };

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
        .collect::<Result<Vec<ProductDomainEventRecord>, _>>()
        .unwrap();

    assert_eq!(vec![expected], actual);
}

#[localstack_test(services = [DynamoDB()])]
async fn should_put_product_event_records_for_multiple_records() {
    let shop_id = ShopId::new();
    let now1 = OffsetDateTime::now_utc();
    let shops_product_id1: ShopsProductId = "123465".into();
    let price = Some(PriceRecord {
        amount: 110,
        currency: CurrencyRecord::Eur,
    });
    let expected1 = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&shop_id, &shops_product_id1),
        sk: product_event_record::domain::mk_sk(&now1).unwrap(),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        event_id: EventId::new(),
        event_type: ProductDomainEventTypeRecord::DomainCreated,
        event_type_schema_version: 0,
        shop_id,
        shops_product_id: shops_product_id1.clone(),
        shop_name: Some("Foo".to_string()),
        shop_type: Faker.fake(),
        title_native: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        description_fr: Some("Bazzz".to_string()),
        description_es: Some("Bazzzz".to_string()),
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
        new_state: Some(ProductStateRecord::Available),
        old_state: Some(ProductStateRecord::Listed),
        url: Some(Url::parse("https://foo.bar/123456").unwrap()),
        images: Faker.fake(),
        auction_start: None,
        auction_end: None,
        timestamp: now1,
        new_price_nzd: None,
        new_price_estimate_min_native: None,
        new_price_estimate_min_eur: None,
        new_price_estimate_min_usd: None,
        new_price_estimate_min_gbp: None,
        new_price_estimate_min_aud: None,
        new_price_estimate_min_cad: None,
        new_price_estimate_min_nzd: None,
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
    };

    let now2 = OffsetDateTime::now_utc();
    let shops_product_id2: ShopsProductId = "123465".into();
    let expected2 = ProductDomainEventRecord {
        pk: product_event_record::domain::mk_pk(&shop_id, &shops_product_id2),
        sk: product_event_record::domain::mk_sk(&now2).unwrap(),
        product_id: ProductId::new(),
        product_slug_id: Faker.fake(),
        shop_slug_id: Faker.fake(),
        event_id: EventId::new(),
        event_type: ProductDomainEventTypeRecord::DomainCreated,
        event_type_schema_version: 0,
        shop_id,
        shops_product_id: shops_product_id2.clone(),
        shop_name: Some("Foo".to_string()),
        shop_type: Faker.fake(),
        title_native: Some(TextRecord::new("Bar", LanguageRecord::De)),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        description_fr: Some("Bazzz".to_string()),
        description_es: Some("Bazzzz".to_string()),
        new_price_native: price,
        new_price_eur: None,
        new_price_usd: None,
        new_price_gbp: None,
        new_price_aud: None,
        new_price_cad: None,
        new_price_nzd: None,
        old_price_native: None,
        old_price_eur: None,
        old_price_usd: None,
        old_price_gbp: None,
        old_price_aud: None,
        old_price_cad: None,
        old_price_nzd: None,
        new_state: Some(ProductStateRecord::Available),
        old_state: Some(ProductStateRecord::Listed),
        url: Some(Url::parse("https://foo.bar/123456").unwrap()),
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
        new_price_estimate_max_native: None,
        new_price_estimate_max_eur: None,
        new_price_estimate_max_usd: None,
        new_price_estimate_max_gbp: None,
        new_price_estimate_max_aud: None,
        new_price_estimate_max_cad: None,
        new_price_estimate_max_nzd: None,
    };

    get_repository()
        .await
        .put_product_event_records(Batch::from([expected1.clone(), expected2.clone()]))
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
        event_id: EventId::new(),
        shop_id,
        shops_product_id: shops_product_id.clone(),
        shop_name: "Foo".to_string(),
        shop_type: Faker.fake(),
        title_native: TextRecord::new("Bar", LanguageRecord::De),
        title_de: Some("Bar".to_string()),
        title_en: Some("Barr".to_string()),
        title_fr: Some("Barrr".to_string()),
        title_es: Some("Barrrr".to_string()),
        description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
        description_de: Some("Baz".to_string()),
        description_en: Some("Bazz".to_string()),
        description_fr: Some("Bazzz".to_string()),
        description_es: Some("Bazzzz".to_string()),
        price_native: Some(price),
        price_eur: None,
        price_usd: None,
        price_gbp: None,
        price_aud: None,
        price_cad: None,
        price_nzd: None,
        price_estimate_min_native: None,
        price_estimate_min_eur: None,
        price_estimate_min_usd: None,
        price_estimate_min_gbp: None,
        price_estimate_min_aud: None,
        price_estimate_min_cad: None,
        price_estimate_min_nzd: None,
        price_estimate_max_native: None,
        price_estimate_max_eur: None,
        price_estimate_max_usd: None,
        price_estimate_max_gbp: None,
        price_estimate_max_aud: None,
        price_estimate_max_cad: None,
        price_estimate_max_nzd: None,
        state: ProductStateRecord::Available,
        url: Url::parse("https://foo.bar/123456").unwrap(),
        images: Faker.fake(),
        text_embedding: Faker.fake(),
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
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
        state: Some(ProductStateRecord::Sold),
        title_de: None,
        title_en: None,
        title_fr: None,
        title_es: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        images: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
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
