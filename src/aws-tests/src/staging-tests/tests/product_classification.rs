use aws_tests_common::get_cfn_output;
use common::batch::Batch;
use fake::{Fake, Faker};
use opensearch::{IndexParts, params::Refresh};
use product::{
    core::product_event::{
        ProductEvent, ProductEventPayload,
        enrichment::{EmbeddedTextProductEnrichmentEventPayload, ProductEnrichmentEventPayload},
    },
    dynamodb::{
        product_event_record::ProductEventRecord,
        product_record::{ProductRecord, mk_pk},
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
    },
};
use product_classification::category::{
    core::Category,
    dynamodb_repository::CategoryDynamoDbRepositoryImpl,
    opensearch_repository::CategoryOpenSearchRepositoryImpl,
    service::{CategoryService, CategoryServiceImpl},
};
use shop::{
    core::shop::Shop,
    dynamodb::{
        repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
        shop_record::ShopRecord,
    },
};
use staging_tests::{get_dynamodb_client, get_opensearch_client, staging_test};
use std::time::{Duration, Instant};
use time::OffsetDateTime;

async fn prepare_test_shop() -> Shop {
    let stack = get_cfn_output();
    let shop = Faker.fake::<Shop>();

    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    let _ = dynamodb_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();

    shop
}

async fn prepare_categories() -> Category {
    let dynamodb_repository = CategoryDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let opensearch = get_opensearch_client().await;
    let opensearch_repository = CategoryOpenSearchRepositoryImpl::new(opensearch);
    let category_service = CategoryServiceImpl::new(&dynamodb_repository, &opensearch_repository);

    let categories = Category::load_categories();
    let category = categories.first().unwrap().clone();
    for category in categories {
        let _ = category_service.upsert_category(category).await.unwrap();
    }
    refresh_index("categories").await;
    category
}

pub async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .index(IndexParts::Index(index))
        .refresh(Refresh::True)
        .send()
        .await
        .unwrap();
}

#[staging_test]
async fn should_materialize_product_in_dynamodb_for_embed_text_triggering_classification() {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let shop = prepare_test_shop().await;

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    materialized_old.text_embedding = None;
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let expected_category = prepare_categories().await;

    let product_events: [ProductEvent; 1] = [ProductEvent {
        aggregate_id: materialized_old.product_id,
        event_id: materialized_old.event_id,
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductEnrichmentEvent(
            ProductEnrichmentEventPayload::EmbeddedText(
                EmbeddedTextProductEnrichmentEventPayload {
                    shop_id: materialized_old.shop_id,
                    shops_product_id: materialized_old.shops_product_id.clone(),
                    embedding: expected_category.embedding.clone(),
                },
            ),
        ),
    }];
    let product_event_records = Batch::try_from_iter(
        product_events
            .into_iter()
            .map(|event| ProductEventRecord::try_from(event).unwrap()),
    )
    .unwrap();
    let _ = repository
        .put_product_event_records(product_event_records)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && let Some(category_id) = materialized.category_id
        {
            assert_eq!(expected_category.category_id, category_id);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord with shop_id '{}' and shops_product_id '{}' \
                    has not been updated in DynamoDB or been updated with expected state after 60 seconds",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
