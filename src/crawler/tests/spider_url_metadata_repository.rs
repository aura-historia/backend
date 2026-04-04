use common::shop_id::ShopId;
use crawler::spider::classification::url_metadata::{UrlClass, UrlState};
use crawler::spider::classification::url_metadata_repository::{
    MainHash, UrlMetadataRepository, UrlMetadataRepositoryImpl,
};
use test_api::*;

const RDS: Rds = Rds {
    sql_setup_file: "src/crawler/sql/schema.sql",
};
use url::Url;

/// Helper: inserts a shop + domain and returns the generated domain_id.
async fn insert_shop_with_domain(
    pool: &sqlx::PgPool,
    shop_id_uuid: uuid::Uuid,
    domain: &str,
) -> uuid::Uuid {
    sqlx::query("INSERT INTO shops (shop_id, created, updated) VALUES ($1, NOW(), NOW())")
        .bind(shop_id_uuid)
        .execute(pool)
        .await
        .unwrap();

    let row: (uuid::Uuid,) = sqlx::query_as(
        "INSERT INTO shop_domains (shop_id, shop_domain) VALUES ($1, $2) RETURNING domain_id",
    )
    .bind(shop_id_uuid)
    .bind(domain)
    .fetch_one(pool)
    .await
    .unwrap();

    row.0
}

#[serial]
#[localstack_test(services = [RDS])]
async fn should_insert_new_url_when_url_does_not_exist() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    let shop_id_uuid: uuid::Uuid = shop_id.into();
    let domain_id = insert_shop_with_domain(&pool, shop_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let url_class = UrlClass::Product;
    let main_hash = MainHash("a".repeat(64));

    let result = repository
        .upsert_link(&shop_id, &domain_id, &url, &url_class, &main_hash)
        .await
        .unwrap();

    assert_eq!(result.url, url);
    assert_eq!(result.url_class, url_class);
    assert_eq!(result.main_hash, main_hash);
    assert_eq!(result.state, UrlState::Unknown);
    assert_eq!(result.domain_id, domain_id);
}

#[serial]
#[localstack_test(services = [RDS])]
async fn should_update_existing_url_when_url_already_exists() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    let shop_id_uuid: uuid::Uuid = shop_id.into();
    let domain_id = insert_shop_with_domain(&pool, shop_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let old_class = UrlClass::Other;
    let old_hash = MainHash("o".repeat(64));

    repository
        .upsert_link(&shop_id, &domain_id, &url, &old_class, &old_hash)
        .await
        .unwrap();

    let new_class = UrlClass::Product;
    let new_hash = MainHash("n".repeat(64));

    let result2 = repository
        .upsert_link(&shop_id, &domain_id, &url, &new_class, &new_hash)
        .await
        .unwrap();

    assert_eq!(result2.url, url);
    assert_eq!(result2.url_class, new_class);
    assert_eq!(result2.main_hash, new_hash);
    assert_eq!(result2.state, UrlState::Unknown);
}

#[serial]
#[localstack_test(services = [RDS])]
async fn should_update_last_scraped_timestamp_when_marking_as_scraped() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    let shop_id_uuid: uuid::Uuid = shop_id.into();
    let domain_id = insert_shop_with_domain(&pool, shop_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let url_class = UrlClass::Product;
    let main_hash = MainHash("a".repeat(64));

    repository
        .upsert_link(&shop_id, &domain_id, &url, &url_class, &main_hash)
        .await
        .unwrap();

    let marked = repository
        .mark_as_scraped(&shop_id, &url, "dummy_hash")
        .await
        .unwrap();

    assert!(marked.last_scraped.is_some());
}

#[serial]
#[localstack_test(services = [RDS])]
async fn should_update_state_when_setting_new_state() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    let shop_id_uuid: uuid::Uuid = shop_id.into();
    let domain_id = insert_shop_with_domain(&pool, shop_id_uuid, "example.com").await;

    let url = Url::parse("https://example.com/product/123").unwrap();
    let url_class = UrlClass::Product;
    let main_hash = MainHash("a".repeat(64));

    let result = repository
        .upsert_link(&shop_id, &domain_id, &url, &url_class, &main_hash)
        .await
        .unwrap();

    assert_eq!(result.state, UrlState::Unknown);

    let marked = repository
        .set_state(&shop_id, &url, &UrlState::Sold)
        .await
        .unwrap();

    assert_eq!(marked.state, UrlState::Sold);
}

#[serial]
#[localstack_test(services = [RDS])]
async fn should_upsert_multiple_links_when_inserting_batch() {
    let pool = get_postgres_client().await;
    let repository = UrlMetadataRepositoryImpl::new(pool.clone());

    let shop_id: ShopId = uuid::Uuid::new_v4().into();
    let shop_id_uuid: uuid::Uuid = shop_id.into();
    let domain_id = insert_shop_with_domain(&pool, shop_id_uuid, "example.com").await;

    let urls = vec![
        Url::parse("https://example.com/product/1").unwrap(),
        Url::parse("https://example.com/product/2").unwrap(),
    ];
    let classes = vec![UrlClass::Product, UrlClass::Product];
    let hashes = vec![MainHash("a".repeat(64)), MainHash("b".repeat(64))];

    let inserted = repository
        .upsert_links_batch(&shop_id, &domain_id, &urls, &classes, &hashes)
        .await
        .unwrap();

    assert_eq!(inserted.len(), 2);
    assert!(inserted.iter().any(|r| r.url == urls[0]));
    assert!(inserted.iter().any(|r| r.url == urls[1]));

    let updated_hashes = vec![MainHash("c".repeat(64)), MainHash("d".repeat(64))];

    let updated = repository
        .upsert_links_batch(&shop_id, &domain_id, &urls, &classes, &updated_hashes)
        .await
        .unwrap();

    assert_eq!(updated.len(), 2);
    assert!(
        updated
            .iter()
            .any(|r| r.url == urls[0] && r.main_hash == updated_hashes[0])
    );
    assert!(
        updated
            .iter()
            .any(|r| r.url == urls[1] && r.main_hash == updated_hashes[1])
    );
}
