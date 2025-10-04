use item_dynamodb::repository::ItemDynamoDbRepositoryImpl;
use test_api::*;

async fn get_repository() -> ItemDynamoDbRepositoryImpl<'static> {
    ItemDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

mod get_item_record {
    use crate::get_repository;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::item_id::ItemId;
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use item_dynamodb::item_event_record::ItemEventRecord;
    use item_dynamodb::item_event_type_record::ItemEventTypeRecord;
    use item_dynamodb::item_record::ItemRecord;
    use item_dynamodb::item_state_record::ItemStateRecord;
    use item_dynamodb::repository::ItemDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use time::format_description::well_known;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_item_record_when_table_is_empty() {
        let repository = get_repository().await;
        let actual = repository
            .get_item_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_record_for_get_item_record_when_exists() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_item_id: ShopsItemId = "123465".into();
        let expected = ItemRecord {
            pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
            sk: "item#materialized".to_string(),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title_native: TextRecord::new("Bar", LanguageRecord::De),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
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
            state: ItemStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
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
            .get_item_record(&shop_id, &shops_item_id)
            .await
            .unwrap();

        assert!(actual.is_some());
        assert_eq!(expected, actual.unwrap());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_item_record_when_only_others_exist() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_item_id: ShopsItemId = "123465".into();
        let other = ItemRecord {
            pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
            sk: "item#materialized".to_string(),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title_native: TextRecord::new("Bar", LanguageRecord::De),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
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
            state: ItemStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
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
            .get_item_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_item_record_when_only_others_exist_mix() {
        let now = OffsetDateTime::now_utc();
        let now_str = now.format(&well_known::Rfc3339).unwrap();
        let shop_id = ShopId::new();
        let shops_item_id: ShopsItemId = "123465".into();
        let other1 = ItemRecord {
            pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
            sk: "item#materialized".to_string(),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            shop_id,
            shops_item_id: shops_item_id.clone(),
            shop_name: "Foo".to_string(),
            title_native: TextRecord::new("Bar", LanguageRecord::De),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
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
            state: ItemStateRecord::Available,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            images: vec![Url::parse("https://foo.bar/123456/image").unwrap()],
            created: now,
            updated: now,
        };
        let other2 = ItemEventRecord {
            pk: format!("item#shop_id#{shop_id}#shops_item_id#{shops_item_id}"),
            sk: format!("item#event#{now_str}"),
            item_id: ItemId::new(),
            event_id: EventId::new(),
            event_type: ItemEventTypeRecord::StateListed,
            shop_id,
            shops_item_id: shops_item_id.clone(),
            shop_name: None,
            title_native: Some(TextRecord::new("Bar", LanguageRecord::De)),
            title_de: Some("Bar".to_string()),
            title_en: Some("Barr".to_string()),
            description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
            description_de: Some("Baz".to_string()),
            description_en: Some("Bazz".to_string()),
            price_native: None,
            price_eur: None,
            price_usd: None,
            price_gbp: None,
            price_aud: None,
            price_cad: None,
            price_nzd: None,
            state: Some(ItemStateRecord::Listed),
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
            .get_item_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }
}

mod query_item_record_and_event_records {
    use crate::get_repository;
    use common::price::domain::Price;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use fake::{Fake, Faker};
    use item_core::item_event::{
        ItemCreatedEventPayload, ItemEvent, ItemEventPayload, ItemStateChangeEventPayload,
    };
    use item_dynamodb::item_event_record::ItemEventRecord;
    use item_dynamodb::item_event_type_record::ItemEventTypeRecord;
    use item_dynamodb::item_record::ItemRecord;
    use item_dynamodb::repository::ItemDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_none_when_partition_empty() {
        let repository = get_repository().await;

        let actual = repository
            .query_item_record_and_event_records(&ShopId::new(), &ShopsItemId::new())
            .await
            .unwrap();

        assert!(actual.is_none())
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_none_when_events_exist_but_materialized_does_not() {
        let event: ItemEventRecord = ItemEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(Faker.fake()),
        }
        .try_into()
        .unwrap();
        let repository = get_repository().await;
        let insert_res = repository
            .put_item_event_records([event.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual = repository
            .query_item_record_and_event_records(&event.shop_id, &event.shops_item_id)
            .await
            .unwrap();

        assert!(actual.is_none())
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_some_when_materialized_exists_but_events_do_not() {
        let expected = Faker.fake::<ItemRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_item_records([expected.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual, events) = repository
            .query_item_record_and_event_records(&expected.shop_id, &expected.shops_item_id)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected, actual);
        assert!(events.is_empty())
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_some_when_materialized_and_events_exist() {
        let expected_materialized = Faker.fake::<ItemRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_item_records([expected_materialized.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let created_event: ItemEventRecord = ItemEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(ItemCreatedEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_item_id: expected_materialized.shops_item_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                native_title: expected_materialized.title_native.clone().into(),
                other_title: Default::default(),
                native_description: Default::default(),
                other_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                images: expected_materialized.images.clone(),
            }),
        }
        .try_into()
        .unwrap();
        let updated_event: ItemEventRecord = ItemEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateAvailable(ItemStateChangeEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_item_id: expected_materialized.shops_item_id.clone(),
            }),
        }
        .try_into()
        .unwrap();
        let insert_res = repository
            .put_item_event_records([created_event.clone(), updated_event.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual_materialized, actual_events) = repository
            .query_item_record_and_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_item_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected_materialized, actual_materialized);
        assert_eq!(2, actual_events.len());
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_oldest_event_first() {
        let expected_materialized = Faker.fake::<ItemRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_item_records([expected_materialized.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let created_event: ItemEventRecord = ItemEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::Created(ItemCreatedEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_item_id: expected_materialized.shops_item_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                native_title: expected_materialized.title_native.clone().into(),
                other_title: Default::default(),
                native_description: Default::default(),
                other_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                images: expected_materialized.images.clone(),
            }),
        }
        .try_into()
        .unwrap();
        let updated_event: ItemEventRecord = ItemEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ItemEventPayload::StateAvailable(ItemStateChangeEventPayload {
                shop_id: expected_materialized.shop_id,
                shops_item_id: expected_materialized.shops_item_id.clone(),
            }),
        }
        .try_into()
        .unwrap();
        let insert_res = repository
            .put_item_event_records([created_event.clone(), updated_event.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual_materialized, actual_events) = repository
            .query_item_record_and_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_item_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected_materialized, actual_materialized);
        assert_eq!(2, actual_events.len());
        assert_eq!(ItemEventTypeRecord::Created, actual_events[0].event_type);
        assert_eq!(
            ItemEventTypeRecord::StateAvailable,
            actual_events[1].event_type
        );
    }
}

mod batch_get_item_records {
    use crate::get_repository;
    use common::batch::Batch;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::item_id::{ItemId, ItemKey};
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use item_dynamodb::item_record::ItemRecord;
    use item_dynamodb::item_state_record::ItemStateRecord;
    use item_dynamodb::repository::ItemDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_records_for_batch_get_item_records_when_all_exist() {
        let repository = get_repository().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_item_id: ShopsItemId = n.to_string().into();
            ItemRecord {
                pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
                sk: "item#materialized".to_string(),
                item_id: ItemId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                shop_name: "Foo".to_string(),
                title_native: TextRecord::new("Bar", LanguageRecord::De),
                title_de: Some("Bar".to_string()),
                title_en: Some("Barr".to_string()),
                description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
                description_de: Some("Baz".to_string()),
                description_en: Some("Bazz".to_string()),
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
                state: ItemStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
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
            .get_item_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ItemKey::new(shop_id, ShopsItemId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        actuals
            .items
            .sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_records_for_batch_get_item_records_when_some_do_not_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_item_id: ShopsItemId = n.to_string().into();
            ItemRecord {
                pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
                sk: "item#materialized".to_string(),
                item_id: ItemId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                shop_name: "Foo".to_string(),
                title_native: TextRecord::new("Bar", LanguageRecord::De),
                title_de: Some("Bar".to_string()),
                title_en: Some("Barr".to_string()),
                description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
                description_de: Some("Baz".to_string()),
                description_en: Some("Bazz".to_string()),
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
                state: ItemStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
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
            .get_item_records(
                &Batch::try_from(
                    (1..=14)
                        .map(|n| ItemKey::new(shop_id, ShopsItemId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 10);

        expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        actuals
            .items
            .sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_records_for_batch_get_item_records_when_more_than_100_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_item_id: ShopsItemId = n.to_string().into();
            ItemRecord {
                pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
                sk: "item#materialized".to_string(),
                item_id: ItemId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                shop_name: "Foo".to_string(),
                title_native: TextRecord::new("Bar", LanguageRecord::De),
                title_de: Some("Bar".to_string()),
                title_en: Some("Barr".to_string()),
                description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
                description_de: Some("Baz".to_string()),
                description_en: Some("Bazz".to_string()),
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
                state: ItemStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
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
            .get_item_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ItemKey::new(shop_id, ShopsItemId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        actuals
            .items
            .sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        assert_eq!(actuals.items, expecteds);
    }
}

mod batch_exist_item_records {
    use crate::get_repository;
    use common::batch::Batch;
    use common::currency::record::CurrencyRecord;
    use common::event_id::EventId;
    use common::has_key::HasKey;
    use common::item_id::{ItemId, ItemKey};
    use common::language::record::{LanguageRecord, TextRecord};
    use common::price::record::PriceRecord;
    use common::shop_id::ShopId;
    use common::shops_item_id::ShopsItemId;
    use item_dynamodb::item_record::ItemRecord;
    use item_dynamodb::item_state_record::ItemStateRecord;
    use item_dynamodb::repository::ItemDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_keys_for_batch_exist_item_records_when_all_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_item_id: ShopsItemId = n.to_string().into();
            ItemRecord {
                pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
                sk: "item#materialized".to_string(),
                item_id: ItemId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                shop_name: "Foo".to_string(),
                title_native: TextRecord::new("Bar", LanguageRecord::De),
                title_de: Some("Bar".to_string()),
                title_en: Some("Barr".to_string()),
                description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
                description_de: Some("Baz".to_string()),
                description_en: Some("Bazz".to_string()),
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
                state: ItemStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
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
            .exist_item_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ItemKey::new(shop_id, ShopsItemId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        actuals.items.sort();
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_keys_for_batch_exist_item_records_when_some_do_not_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_item_id: ShopsItemId = n.to_string().into();
            ItemRecord {
                pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
                sk: "item#materialized".to_string(),
                item_id: ItemId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                shop_name: "Foo".to_string(),
                title_native: TextRecord::new("Bar", LanguageRecord::De),
                title_de: Some("Bar".to_string()),
                title_en: Some("Barr".to_string()),
                description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
                description_de: Some("Baz".to_string()),
                description_en: Some("Bazz".to_string()),
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
                state: ItemStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
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
            .exist_item_records(
                &Batch::try_from(
                    (1..=14)
                        .map(|n| ItemKey::new(shop_id, ShopsItemId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 10);

        expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        actuals.items.sort();
        assert_eq!(actuals.items, expecteds);
    }

    #[localstack_test(services = [DynamoDB()])]
    async fn should_return_item_keys_for_batch_exist_item_records_when_more_than_100_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_item_id: ShopsItemId = n.to_string().into();
            ItemRecord {
                pk: format!("item#shop_id#{}#shops_item_id#{shops_item_id}", shop_id),
                sk: "item#materialized".to_string(),
                item_id: ItemId::new(),
                event_id: EventId::new(),
                shop_id,
                shops_item_id: shops_item_id.clone(),
                shop_name: "Foo".to_string(),
                title_native: TextRecord::new("Bar", LanguageRecord::De),
                title_de: Some("Bar".to_string()),
                title_en: Some("Barr".to_string()),
                description_native: Some(TextRecord::new("Baz", LanguageRecord::De)),
                description_de: Some("Baz".to_string()),
                description_en: Some("Bazz".to_string()),
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
                state: ItemStateRecord::Available,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: vec![Url::parse(&format!("https://foo.bar/{n}/image")).unwrap()],
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
            .exist_item_records(
                &Batch::try_from(
                    (1..=100)
                        .map(|n| ItemKey::new(shop_id, ShopsItemId::from(n.to_string())))
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        assert!(actuals.unprocessed.is_none());
        assert_eq!(actuals.items.len(), 100);

        expecteds.sort_by(|x, y| x.shops_item_id.cmp(&y.shops_item_id));
        actuals.items.sort();
        assert_eq!(actuals.items, expecteds);
    }
}
