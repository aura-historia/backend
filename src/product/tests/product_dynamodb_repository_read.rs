use product::dynamodb::repository::ProductDynamoDbRepositoryImpl;
use test_api::*;

async fn get_repository() -> ProductDynamoDbRepositoryImpl<'static> {
    ProductDynamoDbRepositoryImpl::new(get_dynamodb_client().await, "table_1")
}

mod get_product_record {
    use crate::get_repository;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_record::ProductRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_record_when_table_is_empty() {
        let repository = get_repository().await;
        let actual = repository
            .get_product_record(&ShopId::new(), &"non-existent".into())
            .await
            .unwrap();

        assert!(actual.is_none());
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_record_for_get_product_record_when_exists() {
        let expected = Faker.fake::<ProductRecord>();

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
            .get_product_record(&expected.shop_id, &expected.shops_product_id)
            .await
            .unwrap();

        assert!(actual.is_some());
        assert_eq!(expected, actual.unwrap());
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_record_when_only_others_exist() {
        let other = Faker.fake::<ProductRecord>();

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

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_record_when_only_others_exist_mix() {
        let other1 = Faker.fake::<ProductRecord>();
        let other2 = Faker.fake::<ProductDomainEventRecord>();

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
    use product::core::product_event::ProductDomainEvent;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductStateChangeDomainEventPayload,
    };
    use product::core::product_image::ProductImage;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
    use product::dynamodb::product_record::ProductRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_none_when_partition_empty() {
        let repository = get_repository().await;

        let actual = repository
            .query_product_record_and_domain_event_records(&ShopId::new(), &ShopsProductId::new())
            .await
            .unwrap();

        assert!(actual.is_none())
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_none_when_events_exist_but_materialized_does_not() {
        let event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(Faker.fake()),
        }
        .into();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_event_records([event.clone().into()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual = repository
            .query_product_record_and_domain_event_records(&event.shop_id, &event.shops_product_id)
            .await
            .unwrap();

        assert!(actual.is_none())
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_some_when_materialized_exists_but_events_do_not() {
        let expected = Faker.fake::<ProductRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_records([expected.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual, events) = repository
            .query_product_record_and_domain_event_records(
                &expected.shop_id,
                &expected.shops_product_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected, actual);
        assert!(events.is_empty())
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_some_when_materialized_and_events_exist() {
        let expected_materialized = Faker.fake::<ProductRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_records([expected_materialized.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let created_event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(ProductCreatedDomainEventPayload {
                product_slug_id: expected_materialized.product_slug_id.clone(),
                shop_slug_id: expected_materialized.shop_slug_id.clone(),
                seller_slug_id: expected_materialized.seller_slug_id.clone(),
                shop_id: expected_materialized.shop_id,
                seller_id: expected_materialized.seller_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                seller_name: expected_materialized.seller_name.clone().into(),
                shop_type: expected_materialized.shop_type.into(),
                structured_address: None,
                geo_address: None,
                native_title: expected_materialized.title_native.clone().into(),
                native_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                view_url: expected_materialized.view_url.clone(),
                images: expected_materialized
                    .images
                    .clone()
                    .into_iter()
                    .map(ProductImage::from)
                    .collect(),
                auction_start: expected_materialized.auction_start,
                auction_end: expected_materialized.auction_end,
            }),
        }
        .into();
        let updated_event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::StateChanged(
                ProductStateChangeDomainEventPayload {
                    shop_id: expected_materialized.shop_id,
                    seller_id: expected_materialized.seller_id,
                    shops_product_id: expected_materialized.shops_product_id.clone(),
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                },
            ),
        }
        .into();
        let insert_res = repository
            .put_product_event_records(
                [created_event.clone().into(), updated_event.clone().into()].into(),
            )
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual_materialized, actual_events) = repository
            .query_product_record_and_domain_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_product_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected_materialized, actual_materialized);
        assert_eq!(2, actual_events.len());
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_oldest_event_first() {
        let expected_materialized = Faker.fake::<ProductRecord>();
        let repository = get_repository().await;
        let insert_res = repository
            .put_product_records([expected_materialized.clone()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let created_event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(ProductCreatedDomainEventPayload {
                product_slug_id: expected_materialized.product_slug_id.clone(),
                shop_slug_id: expected_materialized.shop_slug_id.clone(),
                seller_slug_id: expected_materialized.seller_slug_id.clone(),
                shop_id: expected_materialized.shop_id,
                seller_id: expected_materialized.seller_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                seller_name: expected_materialized.seller_name.clone().into(),
                shop_type: expected_materialized.shop_type.into(),
                structured_address: None,
                geo_address: None,
                native_title: expected_materialized.title_native.clone().into(),
                native_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                view_url: expected_materialized.view_url.clone(),
                images: expected_materialized
                    .images
                    .clone()
                    .into_iter()
                    .map(ProductImage::from)
                    .collect(),
                auction_start: expected_materialized.auction_start,
                auction_end: expected_materialized.auction_end,
            }),
        }
        .into();
        let updated_event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::StateChanged(
                ProductStateChangeDomainEventPayload {
                    shop_id: expected_materialized.shop_id,
                    seller_id: expected_materialized.seller_id,
                    shops_product_id: expected_materialized.shops_product_id.clone(),
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                },
            ),
        }
        .into();
        let insert_res = repository
            .put_product_event_records(
                [created_event.clone().into(), updated_event.clone().into()].into(),
            )
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let (actual_materialized, actual_events) = repository
            .query_product_record_and_domain_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_product_id,
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(expected_materialized, actual_materialized);
        assert_eq!(2, actual_events.len());
        assert_eq!(
            ProductDomainEventTypeRecord::DomainCreated,
            actual_events[0].event_type
        );
        assert_eq!(
            ProductDomainEventTypeRecord::DomainStateChanged,
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
    use fake::{Fake, Faker};
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_records_for_batch_get_product_records_when_all_exist() {
        let repository = get_repository().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
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
                lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                view_url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: Default::default(),
                embedding: None,
                auction_start: None,
                auction_end: None,
                created_by: common::actor::record::ActorRecord::System,
                updated_by: common::actor::record::ActorRecord::System,
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

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_records_for_batch_get_product_records_when_some_do_not_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
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
                lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                view_url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: Default::default(),
                embedding: None,
                auction_start: None,
                auction_end: None,
                created_by: common::actor::record::ActorRecord::System,
                updated_by: common::actor::record::ActorRecord::System,
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

    #[aura_integration_test(services = [DynamoDB()])]
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
                lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                view_url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: Default::default(),
                embedding: None,
                auction_start: None,
                auction_end: None,
                created_by: common::actor::record::ActorRecord::System,
                updated_by: common::actor::record::ActorRecord::System,
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
    use fake::{Fake, Faker};
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_keys_for_batch_exist_product_records_when_all_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
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
                lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                view_url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: Default::default(),
                embedding: None,
                auction_start: None,
                auction_end: None,
                created_by: common::actor::record::ActorRecord::System,
                updated_by: common::actor::record::ActorRecord::System,
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

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_keys_for_batch_exist_product_records_when_some_do_not_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
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
                lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                view_url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: Default::default(),
                embedding: None,
                auction_start: None,
                auction_end: None,
                created_by: common::actor::record::ActorRecord::System,
                updated_by: common::actor::record::ActorRecord::System,
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

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_keys_for_batch_exist_product_records_when_more_than_100_exist() {
        let client = get_dynamodb_client().await;
        let shop_id = ShopId::new();
        let mk_expected = |n: i32| {
            let now = OffsetDateTime::now_utc();
            let shops_product_id: ShopsProductId = n.to_string().into();
            ProductRecord {
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
                lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
                url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                view_url: Url::parse(&format!("https://foo.bar/{n}")).unwrap(),
                images: Default::default(),
                embedding: None,
                auction_start: None,
                auction_end: None,
                created_by: common::actor::record::ActorRecord::System,
                updated_by: common::actor::record::ActorRecord::System,
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
    use fake::{Fake, Faker};
    use product::dynamodb::product_record::{self, ProductRecord};
    use product::dynamodb::product_state_record::ProductStateRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;
    use url::Url;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_id_for_get_product_id_when_exists() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let product_id = ProductId::new();
        let record = ProductRecord {
            pk: product_record::mk_pk(&shop_id, &shops_product_id),
            sk: product_record::mk_sk().to_string(),
            gsi2_pk: Faker.fake(),
            gsi2_sk: Faker.fake(),
            product_id,
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
            lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            view_url: Url::parse("https://foo.bar/123456").unwrap(),
            images: Default::default(),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created_by: common::actor::record::ActorRecord::System,
            updated_by: common::actor::record::ActorRecord::System,
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

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_nothing_for_get_product_id_when_only_others_exist() {
        let now = OffsetDateTime::now_utc();
        let shop_id = ShopId::new();
        let shops_product_id: ShopsProductId = "123465".into();
        let other = ProductRecord {
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
            lifecycle: common::product_lifecycle::record::ProductLifecycleRecord::Active,
            url: Url::parse("https://foo.bar/123456").unwrap(),
            view_url: Url::parse("https://foo.bar/123456").unwrap(),
            images: Default::default(),
            embedding: None,
            auction_start: None,
            auction_end: None,
            created_by: common::actor::record::ActorRecord::System,
            updated_by: common::actor::record::ActorRecord::System,
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

mod query_product_key {
    use std::time::Duration;

    use crate::get_repository;
    use common::product_id::ProductKey;
    use fake::{Fake, Faker};
    use product::dynamodb::product_record::ProductRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_product_key_for_query_product_key_when_exists() {
        let record = Faker.fake::<ProductRecord>();
        get_dynamodb_client()
            .await
            .put_item()
            .table_name("table_1")
            .set_item(serde_dynamo::to_item(&record).ok())
            .send()
            .await
            .unwrap();

        // wait gsi
        tokio::time::sleep(Duration::from_secs(1)).await;

        let repository = get_repository().await;
        let actual = repository
            .query_product_key(&record.shop_slug_id, &record.product_slug_id)
            .await
            .unwrap()
            .unwrap();

        let expected = ProductKey {
            shop_id: record.shop_id,
            shops_product_id: record.shops_product_id,
        };
        assert_eq!(expected, actual);
    }
}

mod query_product_event_records {
    use crate::get_repository;
    use common::price::domain::Price;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use product::core::product_event::ProductDomainEvent;
    use product::core::product_event::domain::{
        ProductCreatedDomainEventPayload, ProductDomainEventPayload,
        ProductStateChangeDomainEventPayload,
    };
    use product::core::product_image::ProductImage;
    use product::dynamodb::product_event_record::domain::ProductDomainEventRecord;
    use product::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
    use product::dynamodb::product_record::ProductRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;
    use time::OffsetDateTime;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_empty_vec_when_partition_empty() {
        let repository = get_repository().await;

        let actual = repository
            .query_product_domain_event_records(&ShopId::new(), &ShopsProductId::new())
            .await
            .unwrap();

        assert!(actual.is_empty())
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_oldest_event_first() {
        let repository = get_repository().await;

        let expected_materialized = Faker.fake::<ProductRecord>();
        let created_event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::Created(ProductCreatedDomainEventPayload {
                product_slug_id: expected_materialized.product_slug_id.clone(),
                shop_slug_id: expected_materialized.shop_slug_id.clone(),
                seller_slug_id: expected_materialized.seller_slug_id.clone(),
                shop_id: expected_materialized.shop_id,
                seller_id: expected_materialized.seller_id,
                shops_product_id: expected_materialized.shops_product_id.clone(),
                shop_name: expected_materialized.shop_name.clone().into(),
                seller_name: expected_materialized.seller_name.clone().into(),
                shop_type: expected_materialized.shop_type.into(),
                structured_address: None,
                geo_address: None,
                native_title: expected_materialized.title_native.clone().into(),
                native_description: Default::default(),
                native_price: expected_materialized.price_native.map(Price::from),
                other_price: Default::default(),
                native_price_estimate_min: None,
                other_price_estimate_min: Default::default(),
                native_price_estimate_max: None,
                other_price_estimate_max: Default::default(),
                state: expected_materialized.state.into(),
                url: expected_materialized.url.clone(),
                view_url: expected_materialized.view_url,
                images: expected_materialized
                    .images
                    .clone()
                    .into_iter()
                    .map(ProductImage::from)
                    .collect(),
                auction_start: expected_materialized.auction_start,
                auction_end: expected_materialized.auction_end,
            }),
        }
        .into();
        let updated_event: ProductDomainEventRecord = ProductDomainEvent {
            aggregate_id: Default::default(),
            event_id: Default::default(),
            timestamp: OffsetDateTime::now_utc(),
            payload: ProductDomainEventPayload::StateChanged(
                ProductStateChangeDomainEventPayload {
                    shop_id: expected_materialized.shop_id,
                    seller_id: expected_materialized.seller_id,
                    shops_product_id: expected_materialized.shops_product_id.clone(),
                    old_state: ProductState::Listed,
                    new_state: ProductState::Available,
                },
            ),
        }
        .into();
        let insert_res = repository
            .put_product_event_records(
                [created_event.clone().into(), updated_event.clone().into()].into(),
            )
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual_events = repository
            .query_product_domain_event_records(
                &expected_materialized.shop_id,
                &expected_materialized.shops_product_id,
            )
            .await
            .unwrap();

        assert_eq!(2, actual_events.len());
        assert_eq!(
            ProductDomainEventTypeRecord::DomainCreated,
            actual_events[0].event_type
        );
        assert_eq!(
            ProductDomainEventTypeRecord::DomainStateChanged,
            actual_events[1].event_type
        );
    }
}

mod query_product_enrichment_event_records {
    use crate::get_repository;
    use common::event_id::EventId;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use fake::{Fake, Faker};
    use product::dynamodb::product_event_record::enrichment::{
        ProductEnrichmentEventRecord, mk_pk, mk_sk,
    };
    use product::dynamodb::product_event_type_record::enrichment::ProductEnrichmentEventTypeRecord;
    use product::dynamodb::repository::ProductDynamoDbRepository;
    use test_api::*;

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_empty_vec_when_partition_empty() {
        let repository = get_repository().await;

        let actual = repository
            .query_product_enrichment_event_records(&ShopId::new(), &ShopsProductId::new())
            .await
            .unwrap();

        assert!(actual.is_empty())
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_enrichment_event_records_when_they_exist() {
        let repository = get_repository().await;

        let first: ProductEnrichmentEventRecord = Faker.fake();

        // Build a second record in the same product partition with a different event id.
        let second_event_id = EventId::new();
        let mut second: ProductEnrichmentEventRecord = Faker.fake();
        second.pk = mk_pk(&first.shop_id, &first.shops_product_id);
        second.sk = mk_sk(&second_event_id);
        second.event_id = second_event_id;
        second.shop_id = first.shop_id;
        second.seller_id = first.seller_id;
        second.shops_product_id = first.shops_product_id.clone();

        let insert_res = repository
            .put_product_event_records([first.clone().into(), second.clone().into()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual = repository
            .query_product_enrichment_event_records(&first.shop_id, &first.shops_product_id)
            .await
            .unwrap();

        assert_eq!(2, actual.len());
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_return_enrichment_event_type_when_record_exists() {
        let repository = get_repository().await;

        let mut record: ProductEnrichmentEventRecord = Faker.fake();
        record.event_type = ProductEnrichmentEventTypeRecord::EnrichmentEmbedded;

        let insert_res = repository
            .put_product_event_records([record.clone().into()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual = repository
            .query_product_enrichment_event_records(&record.shop_id, &record.shops_product_id)
            .await
            .unwrap();

        assert_eq!(1, actual.len());
        assert_eq!(
            ProductEnrichmentEventTypeRecord::EnrichmentEmbedded,
            actual[0].event_type
        );
    }

    #[aura_integration_test(services = [DynamoDB()])]
    async fn should_not_return_records_for_different_product() {
        let repository = get_repository().await;

        let record: ProductEnrichmentEventRecord = Faker.fake();
        let insert_res = repository
            .put_product_event_records([record.clone().into()].into())
            .await
            .unwrap();
        assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

        let actual = repository
            .query_product_enrichment_event_records(&ShopId::new(), &ShopsProductId::new())
            .await
            .unwrap();

        assert!(actual.is_empty())
    }
}
