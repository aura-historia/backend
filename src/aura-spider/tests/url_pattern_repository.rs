use aura_spider::classification::url_pattern_repository::{
    ShopUrlPatternRepository, ShopUrlPatternRepositoryImpl
};

use test_api::*;

const RDS: Rds = Rds {
    sql_setup_file: "src/aura-spider/sql/schema.sql",
};

// ---------------------------------------------------------------------------
// find_pattern
// ---------------------------------------------------------------------------

#[localstack_test(services = [RDS])]
async fn should_return_none_when_no_pattern_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ShopUrlPatternRepositoryImpl::new(pool);

    let result = repository
        .find_pattern("https://nonexistent.com")
        .await
        .unwrap();

    assert!(result.is_none());
}

#[localstack_test(services = [RDS])]
async fn should_return_pattern_when_exists_for_find() {
    let pool = get_postgres_client().await;
    let repository = ShopUrlPatternRepositoryImpl::new(pool.clone());

    let shop_url = "https://example.com";
    let pattern = r"/product/\d+";
    
    repository.save_pattern(shop_url, Some(pattern)).await.unwrap();

    let result = repository
        .find_pattern(shop_url)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.shop_url, shop_url);
    assert_eq!(result.pattern.unwrap(), pattern);
}

// ---------------------------------------------------------------------------
// save_pattern
// ---------------------------------------------------------------------------

#[localstack_test(services = [RDS])]
async fn should_persist_and_return_pattern_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopUrlPatternRepositoryImpl::new(pool);

    let shop_url = "https://insert-example.com";
    let pattern = r"/item/\w+";
    
    repository.save_pattern(shop_url, Some(pattern)).await.unwrap();

    let returned = repository.find_pattern(shop_url).await.unwrap().unwrap();

    assert_eq!(returned.shop_url, shop_url);
    assert_eq!(returned.pattern.unwrap(), pattern);
}

#[localstack_test(services = [RDS])]
async fn should_preserve_created_and_updated_timestamps_for_insert() {
    let pool = get_postgres_client().await;
    let repository = ShopUrlPatternRepositoryImpl::new(pool);

    let shop_url = "https://ts-example.com";
    let pattern = "/ts-item";
    
    repository.save_pattern(shop_url, Some(pattern)).await.unwrap();
    let record1 = repository.find_pattern(shop_url).await.unwrap().unwrap();
    
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    
    repository.save_pattern(shop_url, Some("/ts-item-new")).await.unwrap();
    let record2 = repository.find_pattern(shop_url).await.unwrap().unwrap();

    assert!(
        (record2.created - record1.created).abs() < time::Duration::microseconds(1000),
        "created timestamp drifted unexpectedly"
    );
    assert!(
        record2.updated > record1.updated,
        "updated timestamp should be strictly newer after an update"
    );
}

#[localstack_test(services = [RDS])]
async fn should_allow_clearing_pattern() {
    let pool = get_postgres_client().await;
    let repository = ShopUrlPatternRepositoryImpl::new(pool);

    let shop_url = "https://clear-example.com";
    
    repository.save_pattern(shop_url, Some("/clear-item")).await.unwrap();
    
    // Explicitly clear pattern
    repository.save_pattern(shop_url, None).await.unwrap();

    let returned = repository.find_pattern(shop_url).await.unwrap().unwrap();

    assert_eq!(returned.shop_url, shop_url);
    assert!(returned.pattern.is_none());
}
