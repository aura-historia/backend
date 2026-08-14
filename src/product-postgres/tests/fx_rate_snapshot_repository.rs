use common::{
    currency::domain::Currency,
    postgres::SqlxUnitOfWork,
    transaction::{Transaction, UnitOfWork},
};
use product_core::{
    fx_rate_id::FxRateId,
    fx_rate_snapshot::{FxRateSnapshot, FxRateSource},
};
use product_postgres::SqlxFxRateSnapshotRepositoryFactory;
use product_service::ports::{
    FxRateSnapshotInsertOutcome, FxRateSnapshotRepository, FxRateSnapshotRepositoryFactory,
};
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn snapshot() -> FxRateSnapshot {
    match FxRateSnapshot::capture_eur(
        FxRateId::new(),
        OffsetDateTime::UNIX_EPOCH,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter()
            .filter(|currency| *currency != Currency::Eur)
            .map(|currency| (currency, 1_250_000)),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => panic!("test snapshot must be valid: {error}"),
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_store_one_immutable_snapshot_with_all_eur_conversions() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let snapshot = snapshot();
        let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let outcome = SqlxFxRateSnapshotRepositoryFactory::new()
            .in_transaction(&mut tx)
            .insert(&snapshot, "event-insert")
            .await?;
        tx.commit().await?;

        assert_eq!(FxRateSnapshotInsertOutcome::Inserted, outcome);
        let source: String = sqlx::query_scalar("SELECT source FROM fx_rates WHERE fx_rate_id = $1")
            .bind(uuid::Uuid::from(snapshot.id()))
            .fetch_one(&pool)
            .await?;
        let conversions: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fx_rate_conversions WHERE fx_rate_id = $1 AND from_currency = 'EUR'",
        )
        .bind(uuid::Uuid::from(snapshot.id()))
        .fetch_one(&pool)
        .await?;
        assert_eq!("fxratesapi", source);
        assert_eq!(17, conversions);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "snapshot repository integration failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_deduplicate_eventbridge_redelivery_and_rollback_uncommitted_snapshot() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let factory = SqlxFxRateSnapshotRepositoryFactory::new();
        let first = snapshot();
        let mut first_tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        assert_eq!(
            FxRateSnapshotInsertOutcome::Inserted,
            factory
                .in_transaction(&mut first_tx)
                .insert(&first, "event-duplicate")
                .await?
        );
        first_tx.commit().await?;

        let mut duplicate_tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        assert_eq!(
            FxRateSnapshotInsertOutcome::Duplicate,
            factory
                .in_transaction(&mut duplicate_tx)
                .insert(&snapshot(), "event-duplicate")
                .await?
        );
        duplicate_tx.commit().await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fx_rates WHERE source_event_id = 'event-duplicate'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(1, count);

        let rolled_back = snapshot();
        let mut rollback_tx = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        factory
            .in_transaction(&mut rollback_tx)
            .insert(&rolled_back, "event-rollback")
            .await?;
        drop(rollback_tx);
        let rolled_back_count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM fx_rates WHERE source_event_id = 'event-rollback'",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(0, rolled_back_count);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "snapshot repository transaction integration failed: {result:?}"
    );
}
