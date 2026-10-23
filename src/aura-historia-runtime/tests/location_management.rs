use aura_historia_runtime::Runtime;
use std::sync::Arc;
#[tokio::test]
async fn should_reuse_all_nine_location_handles_without_opening_connections_or_loading_providers() {
    let pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://fixture:fixture@localhost:9/fixture")
        .unwrap();
    let runtime = Runtime::from_pool(pool.clone());
    let locations = runtime.party_locations();
    let same = runtime.party_locations();
    assert!(Arc::ptr_eq(&locations.create, &same.create));
    assert!(Arc::ptr_eq(&locations.update, &same.update));
    assert!(Arc::ptr_eq(&locations.lifecycle, &same.lifecycle));
    assert!(Arc::ptr_eq(&locations.get, &same.get));
    assert!(Arc::ptr_eq(&locations.list, &same.list));
    let defaults = runtime.source_location_defaults();
    let same = runtime.source_location_defaults();
    assert!(Arc::ptr_eq(&defaults.assign, &same.assign));
    assert!(Arc::ptr_eq(&defaults.clear, &same.clear));
    assert!(Arc::ptr_eq(&defaults.change, &same.change));
    assert!(Arc::ptr_eq(&defaults.get, &same.get));
    assert_eq!(0, pool.size());
    pool.close().await;
}
