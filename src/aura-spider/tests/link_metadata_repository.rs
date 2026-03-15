use aura_spider::classification::link_metadata_repository::{
    LinkMetadataRepository, LinkMetadataRepositoryImpl
};

use test_api::*;

const RDS: Rds = Rds {
    sql_setup_file: "src/aura-spider/sql/schema.sql",
};

// ---------------------------------------------------------------------------
// upsert_link
// ---------------------------------------------------------------------------

#[localstack_test(services = [RDS])]
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
}

#[localstack_test(services = [RDS])]
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
    
    assert!(
        (result2.created - result1.created).abs() < time::Duration::microseconds(1000),
        "created timestamp drifted unexpectedly"
    );
    assert!(
        result2.updated > result1.updated,
        "updated timestamp should be strictly newer after an update"
    );
}
