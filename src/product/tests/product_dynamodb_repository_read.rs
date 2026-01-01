use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use test_api::*;

async fn get_repository() -> ProductDynamoDbRepositoryImpl<'static> {
    ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

mod get_product_record {
    use crate::get_repository;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::product_id::ProductId;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use product::dynamodb::product_event_record::{self, ProductEventRecord};
    use product::dynamodb::product_event_type_record::ProductEventTypeRecord;
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_record_when_table_is_empty() {
        let repository = get_repository().await;
        let actual = repository
            .get_product_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_record_for_get_product_record_when_exists() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let expected = ProductRecord {
            pk: product_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_record::mk_sk().to_string(),
            product_id: ProductId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "Foo".to_string(),
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
            state: ProductStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            created: now,
            updated: now,
        };

        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&expected).ok())
            .send()
            .await
            .unwrap();

        let repository = get_repository().await;
        let actual = repository
            .get_product_record(&shop_id, &shops_product_id)
            .await
            .unwrap();

        assert!(actual.is_some());
        assert_eq!(expected, actual.unwrap());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_record_when_only_others_exist() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let other = ProductRecord {
            pk: product_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_record::mk_sk().to_string(),
            product_id: ProductId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "Foo".to_string(),
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
            state: ProductStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            created: now,
            updated: now,
        };

        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&other).ok())
            .send()
            .await
            .unwrap();

        let repository = get_repository().await;
        let actual = repository
            .get_product_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_record_when_only_others_exist_mix() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let other1 = ProductRecord {
            pk: product_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_record::mk_sk().to_string(),
            product_id: ProductId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "Foo".to_string(),
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
            state: ProductStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            created: now,
            updated: now,
        };
        let other2 = ProductEventRecord {
            pk: product_event_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_event_record::mk_sk(&now).unwrap(),
            product_id: ProductId::new(),
            event_id: EventId::new(),
            event_type: ProductEventTypeRecord::StateListed,
            event_type_schema_version: 0,
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: None,
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
            new_price_native: None,
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
            new_state: Some(ProductStateRecord::Listed),
            old_state: Some(ProductStateRecord::Available),
            url: None,
            images: Some(vec![Url::parse("https://foo.bar/123456/image").unwrap()]),
            timestamp: OffsetDateTime::now_utc(),
        };

        let repository = get_repository().await;
        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&other1).ok())
            .send()
            .await
            .unwrap();
        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&other2).ok())
            .send()
            .await
            .unwrap();

        let actual = repository
            .get_product_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }
}

mod query_product_record_and_event_records {
    use crate::get_repository;
    use common::price::domain::Price;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use product::core::product_event::{
        ProductCreatedEventPayload, ProductEvent, ProductEventPayload,
        ProductStateChangeEventPayload,
    };
    use product::dynamodb::product_event_record::ProductEventRecord;
    use product::dynamodb::product_event_type_record::ProductEventTypeRecord;
    use product::dynamodb::product_record::ProductRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_none_when_partition_empty() {
        let repository = get_repository().await;

        let actual = repository
            .query_product_record_and_event_records(&ShopId::new(), &ShopsProductId::new())
            .await
            .unwrap();

        assert!(actual.is_none())
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_none_when_events_exist_but_materialized_does_not() {
        let event: ProductEventRecord = ProductEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(Faker.fake()),
        }
        .try_into()
        .unwrap();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_event_records([event.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual = repository
            .query_product_record_and_event_records(&event.shop_id, &event.shops_product_id)
            .await
            .unwrap();

        assert!(actual.is_none())
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_some_when_materialized_exists_but_events_do_not() {
        let expected = Faker.fake::<ProductRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_records([expected.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual, events) = repository
            .query_product_record_and_event_records(&expected.shop_id, &expected.shops_product_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected, actual);
        assert!(events.is_empty())
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_some_when_materialized_and_events_exist() {
        let expected_materialized = Faker.fake::<ProductRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_records([expected_materialized.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let created_event: ProductEventRecord = ProductEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(ProductCreatedEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                native_title: expected_materialized.title_native.clone().into(),
                native_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                images: expected_materialized.images.clone(),
            }),
        }
        .try_into()
        .unwrap();
        let updated_event: ProductEventRecord = ProductEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::StateAvailable(ProductStateChangeEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                old_state: ProductState::Listed,
            }),
        }
        .try_into()
        .unwrap();
        let insert_res = repository
            .put_product_event_records([created_event.clone(), updated_event.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual_materialized, actual_events) = repository
            .query_product_record_and_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_product_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected_materialized, actual_materialized);
        assert_eq!(2, actual_events.len());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_oldest_event_first() {
        let expected_materialized = Faker.fake::<ProductRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_records([expected_materialized.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let created_event: ProductEventRecord = ProductEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::Created(ProductCreatedEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                native_title: expected_materialized.title_native.clone().into(),
                native_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                images: expected_materialized.images.clone(),
            }),
        }
        .try_into()
        .unwrap();
        let updated_event: ProductEventRecord = ProductEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductEventPayload::StateAvailable(ProductStateChangeEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                old_state: ProductState::Listed,
            }),
        }
        .try_into()
        .unwrap();
        let insert_res = repository
            .put_product_event_records([created_event.clone(), updated_event.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual_materialized, actual_events) = repository
            .query_product_record_and_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_product_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected_materialized, actual_materialized);
        assert_eq!(2, actual_events.len());
        assert_eq!(ProductEventTypeRecord::Created, actual_events[0].event_type);
        assert_eq!(
            ProductEventTypeRecord::StateAvailable,
            actual_events[1].event_type
        );
    }
}

mod batch_get_product_records {
    use crate::get_repository;
    use common::batch::Batch;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::product_id::{ProductId, ProductKey};
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_records_for_batch_get_product_records_when_all_exist() {
        let repository = get_repository().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
                pk: product_record::mk_pk(&shop_id, &shops_product_id),
                sk: product_record::mk_sk().to_string(),
                product_id: ProductId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                shop_name: "Foo".to_string(),
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
                state: ProductStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: now,
                updated: now,
            }
        };
        let client = get_dynamodb_client().await;
        let mut expecteds = Vec::with_capacity(100);
        for n in 1..=100 {
            let expected = mk_expected(n);
            client
                .put_item()
                .table_name("table_1")
                .set_item(serde_dynamo::to_item(&expected).ok())
                .send()
                .await
                .unwrap();
            expecteds.push(expected);
        }

        let mut actuals = repository
            .get_product_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ProductKey::new(shop_id, ShopsProductId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        actuals
            .items
            .sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_records_for_batch_get_product_records_when_some_do_not_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
                pk: product_record::mk_pk(&shop_id, &shops_product_id),
                sk: product_record::mk_sk().to_string(),
                product_id: ProductId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                shop_name: "Foo".to_string(),
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
                state: ProductStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: now,
                updated: now,
            }
        };
        let mut expecteds = Vec::with_capacity(100);
        for n in 1..=10 {
            let expected = mk_expected(n);
            client
                .put_item()
                .table_name("table_1")
                .set_item(serde_dynamo::to_item(&expected).ok())
                .send()
                .await
                .unwrap();
            expecteds.push(expected);
        }

        let mut actuals = get_repository()
            .await
            .get_product_records(
                &Batch::try_from(
                    (1..=14)
                        .map(|n| ProductKey::new(shop_id, ShopsProductId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 10);

        expecteds.sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        actuals
            .items
            .sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_records_for_batch_get_product_records_when_more_than_100_exist()
    {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
                pk: product_record::mk_pk(&shop_id, &shops_product_id),
                sk: product_record::mk_sk().to_string(),
                product_id: ProductId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                shop_name: "Foo".to_string(),
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
                state: ProductStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: now,
                updated: now,
            }
        };
        let mut expecteds = Vec::with_capacity(100);
        for n in 1..=120 {
            let expected = mk_expected(n);
            client
                .put_item()
                .table_name("table_1")
                .set_item(serde_dynamo::to_item(&expected).ok())
                .send()
                .await
                .unwrap();
            if n <= 100 {
                expecteds.push(expected);
            }
        }

        let mut actuals = get_repository()
            .await
            .get_product_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ProductKey::new(shop_id, ShopsProductId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        actuals
            .items
            .sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        assert_eq!(actuals.items, expecteds);
    }
}

mod batch_exist_product_records {
    use crate::get_repository;
    use common::batch::Batch;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::has_key::HasKey;
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::product_id::{ProductId, ProductKey};
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_keys_for_batch_exist_product_records_when_all_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
                pk: product_record::mk_pk(&shop_id, &shops_product_id),
                sk: product_record::mk_sk().to_string(),
                product_id: ProductId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                shop_name: "Foo".to_string(),
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
                state: ProductStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: now,
                updated: now,
            }
        };
        let mut expecteds = Vec::with_capacity(100);
        for n in 1..=100 {
            let expected = mk_expected(n);
            client
                .put_item()
                .table_name("table_1")
                .set_item(serde_dynamo::to_item(&expected).ok())
                .send()
                .await
                .unwrap();
            expecteds.push(expected.key());
        }

        let mut actuals = get_repository()
            .await
            .exist_product_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ProductKey::new(shop_id, ShopsProductId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        actuals.items.sort();
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_keys_for_batch_exist_product_records_when_some_do_not_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
                pk: product_record::mk_pk(&shop_id, &shops_product_id),
                sk: product_record::mk_sk().to_string(),
                product_id: ProductId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                shop_name: "Foo".to_string(),
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
                state: ProductStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: now,
                updated: now,
            }
        };
        let mut expecteds = Vec::with_capacity(100);
        for n in 1..=10 {
            let expected = mk_expected(n);
            client
                .put_item()
                .table_name("table_1")
                .set_item(serde_dynamo::to_item(&expected).ok())
                .send()
                .await
                .unwrap();
            expecteds.push(expected.key());
        }

        let mut actuals = get_repository()
            .await
            .exist_product_records(
                &Batch::try_from(
                    (1..=14)
                        .map(|n| ProductKey::new(shop_id, ShopsProductId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 10);

        expecteds.sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        actuals.items.sort();
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_keys_for_batch_exist_product_records_when_more_than_100_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
                pk: product_record::mk_pk(&shop_id, &shops_product_id),
                sk: product_record::mk_sk().to_string(),
                product_id: ProductId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_product_id: shops_product_id.clone(),
                shop_name: "Foo".to_string(),
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
                state: ProductStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
                origin_year_min: None,
                origin_year: None,
                origin_year_max: None,
                authenticity: None,
                condition: None,
                provenance: None,
                restoration: None,
                created: now,
                updated: now,
            }
        };
        let mut expecteds = Vec::with_capacity(100);
        for n in 1..=120 {
            let expected = mk_expected(n);
            client
                .put_item()
                .table_name("table_1")
                .set_item(serde_dynamo::to_item(&expected).ok())
                .send()
                .await
                .unwrap();
            if n <= 100 {
                expecteds.push(expected.key());
            }
        }

        let mut actuals = get_repository()
            .await
            .exist_product_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ProductKey::new(shop_id, ShopsProductId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_product_id.cmp(&y.shops_product_id));
        actuals.items.sort();
        assert_eq!(actuals.items, expecteds);
    }
}

mod get_product_id {
    use crate::get_repository;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::product_id::ProductId;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_product_id_for_get_product_id_when_exists() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let product_id = ProductId::new();
        let record = ProductRecord {
            pk: product_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_record::mk_sk().to_string(),
            product_id,
            event_id: EventId::new(),
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "Foo".to_string(),
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
            state: ProductStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            created: now,
            updated: now,
        };

        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&record).ok())
            .send()
            .await
            .unwrap();

        let repository = get_repository().await;
        let actual = repository
            .get_product_id(&shop_id, &shops_product_id)
            .await
            .unwrap();

        assert!(actual.is_some());
        assert_eq!(product_id, actual.unwrap());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_id_when_only_others_exist() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let other = ProductRecord {
            pk: product_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_record::mk_sk().to_string(),
            product_id: ProductId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_product_id: shops_product_id.clone(),
            shop_name: "Foo".to_string(),
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
            state: ProductStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
            origin_year_min: None,
            origin_year: None,
            origin_year_max: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
            created: now,
            updated: now,
        };

        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&other).ok())
            .send()
            .await
            .unwrap();

        let repository = get_repository().await;
        let actual = repository
            .get_product_id(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }
}
