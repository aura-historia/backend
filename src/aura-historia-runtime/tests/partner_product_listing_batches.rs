use aura_historia_runtime::Runtime;
use std::sync::Arc;
#[tokio::test]
async fn should_reuse_batch_handle_without_connections_or_unrelated_provider_configuration() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
        .unwrap();
    let runtime = Runtime::from_pool(pool.clone());
    assert!(Arc::ptr_eq(
        &runtime.partner_product_listing_batches().batches,
        &runtime.partner_product_listing_batches().batches
    ));
    assert_eq!(0, pool.size());
    pool.close().await;
}
