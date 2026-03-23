use aura_spider::classification::link_metadata_repository::{
    LinkMetadataRepository, LinkMetadataRepositoryImpl,
};

use test_api::*;

const RDS: Rds = Rds {
    sql_setup_file: "src/aura-spider/sql/schema.sql",
};

// ---------------------------------------------------------------------------
// upsert_link
// ---------------------------------------------------------------------------

#[localstack_test(services = [RDS])]
#[serial_test::serial]
async fn should_persist_and_return_link_metadata_for_insert() {
    let pool = get_postgres_client().await;
    let repository = LinkMetadataRepositoryImpl::new(pool);

    let shop_url = "https://example.com";
    let url = "https://example.com/product/123";
    let link_class = "product";
    let main_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    let result = repository
        .upsert_link(shop_url, url, link_class, main_hash)
        .await
        .unwrap();

    assert_eq!(result.shop_url, shop_url);
    assert_eq!(result.url, url);
    assert_eq!(result.link_class, link_class);
    assert_eq!(result.main_hash, main_hash);
    assert_eq!(result.state, "UNKNOWN");
}

#[localstack_test(services = [RDS])]
#[serial_test::serial]
async fn should_update_existing_link_metadata() {
    let pool = get_postgres_client().await;
    let repository = LinkMetadataRepositoryImpl::new(pool);

    let shop_url = "https://example.com";
    let url = "https://example.com/product/123";
    let old_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    let new_hash = "8a32a6886d3e387cfce5e9d936166ab2dd0bf3bbcd37f594539ef8a183594df5";

    let result1 = repository
        .upsert_link(shop_url, url, "other", old_hash)
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let result2 = repository
        .upsert_link(shop_url, url, "product", new_hash)
        .await
        .unwrap();

    assert_eq!(result2.shop_url, shop_url);
    assert_eq!(result2.url, url);
    assert_eq!(result2.link_class, "product");
    assert_eq!(result2.main_hash, new_hash);
    assert_eq!(result2.state, "UNKNOWN");

    assert!(
        (result2.created - result1.created).abs() < time::Duration::microseconds(1000),
        "created timestamp drifted unexpectedly"
    );
    assert!(
        result2.updated > result1.updated,
        "updated timestamp should be strictly newer after an update"
    );
}

#[localstack_test(services = [RDS])]
#[serial_test::serial]
async fn should_mark_link_as_scraped() {
    let pool = get_postgres_client().await;
    let repository = LinkMetadataRepositoryImpl::new(pool);

    let shop_url = "https://example.com";
    let url = "https://example.com/product/123";
    let link_class = "product";
    let main_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    let result = repository
        .upsert_link(shop_url, url, link_class, main_hash)
        .await
        .unwrap();

    assert!(result.last_scraped.is_none());

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let marked = repository.mark_as_scraped(shop_url, url).await.unwrap();

    assert!(marked.last_scraped.is_some());
    assert!(marked.updated > result.updated);
}

#[localstack_test(services = [RDS])]
#[serial_test::serial]
async fn should_set_state() {
    let pool = get_postgres_client().await;
    let repository = LinkMetadataRepositoryImpl::new(pool);

    let shop_url = "https://example.com";
    let url = "https://example.com/product/123";
    let link_class = "product";
    let main_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    let result = repository
        .upsert_link(shop_url, url, link_class, main_hash)
        .await
        .unwrap();

    assert_eq!(result.state, "UNKNOWN");

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let marked = repository.set_state(shop_url, url, "SOLD").await.unwrap();

    assert_eq!(marked.state, "SOLD");
    assert!(marked.updated > result.updated);
}

#[localstack_test(services = [RDS])]
#[serial_test::serial]
async fn should_upsert_links_batch() {
    let pool = get_postgres_client().await;
    let repository = LinkMetadataRepositoryImpl::new(pool);

    let shop_url = "https://example.com";
    let urls = vec![
        "https://example.com/product/1".to_string(),
        "https://example.com/product/2".to_string(),
    ];
    let classes = vec!["product".to_string(), "product".to_string()];
    let hashes = vec![
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
        "8a32a6886d3e387cfce5e9d936166ab2dd0bf3bbcd37f594539ef8a183594df5".to_string(),
    ];

    let inserted = repository
        .upsert_links_batch(shop_url, &urls, &classes, &hashes)
        .await
        .unwrap();

    assert_eq!(inserted.len(), 2);
    assert!(inserted.iter().any(|r| r.url == urls[0]));
    assert!(inserted.iter().any(|r| r.url == urls[1]));

    let updated_hashes = vec![
        "4e07408562bedb8b60ce05c1decfe3ad16b72230967de01f640b7e4729b49fce".to_string(),
        "ef2d127de37b94285b7b67f2f2f1e9f6b8e0f58f2f8f8be6ed7d5d4fcbf0f915".to_string(),
    ];

    let updated = repository
        .upsert_links_batch(shop_url, &urls, &classes, &updated_hashes)
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
