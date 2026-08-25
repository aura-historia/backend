use crawler::scraper::css_selector::removed_page_schema::{
    RemovedPageSchema, ShopsRemovedPageSchema,
};
use crawler::scraper::css_selector::removed_page_schema_repository::{
    RemovedPageSchemaRepository, RemovedPageSchemaRepositoryImpl,
};
use crawler::scraper::css_selector::rule::CssSelector;
use shop_core::shop_id::ShopId;
use sqlx::PgPool;
use test_api::*;
use time::OffsetDateTime;
use uuid::Uuid;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn removed_schema(selector: &str, text: &str) -> RemovedPageSchema {
    RemovedPageSchema {
        selector: CssSelector::from(selector),
        text: Some(text.to_string()),
        regex: None,
    }
}

fn make_shops_removed_page_schema(shop_id: ShopId) -> ShopsRemovedPageSchema {
    let now = OffsetDateTime::now_utc();
    ShopsRemovedPageSchema {
        shop_id,
        removed_page_schemas: vec![removed_schema("#main h1", "ProductListing removed")],
        created: now,
        updated: now,
    }
}

async fn insert_shop(pool: &PgPool, shop_id: ShopId) {
    sqlx::query("INSERT INTO shops (shop_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(Uuid::from(shop_id))
        .execute(pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_insert_and_find_removed_schema_for_shop() {
    let pool = get_postgres_client().await;
    let repository = RemovedPageSchemaRepositoryImpl::new(&pool);
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    let row = make_shops_removed_page_schema(shop_id);
    repository
        .insert_removed_page_schema(&shop_id, &row)
        .await
        .unwrap();

    let found = repository
        .find_removed_page_schema(&shop_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.shop_id, shop_id);
    assert_eq!(
        found.removed_page_schemas,
        vec![removed_schema("#main h1", "ProductListing removed")]
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_update_removed_schema_for_only_target_shop() {
    let pool = get_postgres_client().await;
    let repository = RemovedPageSchemaRepositoryImpl::new(&pool);
    let shop_a = ShopId::new();
    let shop_b = ShopId::new();
    insert_shop(&pool, shop_a).await;
    insert_shop(&pool, shop_b).await;

    repository
        .insert_removed_page_schema(&shop_a, &make_shops_removed_page_schema(shop_a))
        .await
        .unwrap();
    repository
        .insert_removed_page_schema(&shop_b, &make_shops_removed_page_schema(shop_b))
        .await
        .unwrap();

    let updated = vec![removed_schema(".missing", "No longer here")];
    repository
        .update_removed_page_schema(&shop_a, &updated)
        .await
        .unwrap();

    let found_a = repository
        .find_removed_page_schema(&shop_a)
        .await
        .unwrap()
        .unwrap();
    let found_b = repository
        .find_removed_page_schema(&shop_b)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found_a.removed_page_schemas, updated);
    assert_eq!(
        found_b.removed_page_schemas,
        vec![removed_schema("#main h1", "ProductListing removed")]
    );
}

#[aura_integration_test(services = [POSTGRES])]
async fn should_delete_removed_schema_when_parent_shop_is_deleted() {
    let pool = get_postgres_client().await;
    let repository = RemovedPageSchemaRepositoryImpl::new(&pool);
    let shop_id = ShopId::new();
    insert_shop(&pool, shop_id).await;

    repository
        .insert_removed_page_schema(&shop_id, &make_shops_removed_page_schema(shop_id))
        .await
        .unwrap();

    sqlx::query("DELETE FROM shops WHERE shop_id = $1")
        .bind(Uuid::from(shop_id))
        .execute(&pool)
        .await
        .unwrap();

    let found = repository.find_removed_page_schema(&shop_id).await.unwrap();
    assert!(found.is_none());
}
