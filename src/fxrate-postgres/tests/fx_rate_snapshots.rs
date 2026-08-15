use common::{
    currency::domain::Currency,
    fx_rate_id::FxRateId,
    postgres::SqlxUnitOfWork,
    transaction::{Transaction, UnitOfWork},
};
use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
use fxrate_postgres::{SqlxFxRateSnapshotReader, SqlxFxRateSnapshotRepositoryFactory};
use fxrate_service::ports::{
    FxRateSnapshotInsertOutcome, FxRateSnapshotReader, FxRateSnapshotRepository,
    FxRateSnapshotRepositoryFactory,
};
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn snapshot(captured_at: OffsetDateTime) -> NewFxRateSnapshot {
    let result = NewFxRateSnapshot::capture_eur(
        FxRateId::new(),
        captured_at,
        FxRateSource::FxRatesApi,
        Currency::Eur,
        Currency::iter().map(|currency| {
            FxRateQuote::new(
                currency,
                if currency == Currency::Eur {
                    FX_RATE_SCALE
                } else {
                    1_250_000
                },
            )
        }),
    );
    result.unwrap_or_else(|error| panic!("test snapshot must be valid: {error}"))
}

async fn insert(
    pool: sqlx::PgPool,
    snapshot: &NewFxRateSnapshot,
    source_event_id: &str,
) -> Result<FxRateSnapshotInsertOutcome, Box<dyn std::error::Error>> {
    let mut transaction = SqlxUnitOfWork::new(pool).begin().await?;
    let result = SqlxFxRateSnapshotRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(snapshot, source_event_id)
        .await?;
    transaction.commit().await?;
    Ok(result)
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_insert_idempotently_and_read_persisted_snapshots() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let earlier = snapshot(OffsetDateTime::UNIX_EPOCH);
        let later = snapshot(OffsetDateTime::UNIX_EPOCH + Duration::hours(1));
        let inserted_earlier = insert(pool.clone(), &earlier, "event-earlier").await?;
        let inserted_later = insert(pool.clone(), &later, "event-later").await?;
        assert!(matches!(
            inserted_earlier,
            FxRateSnapshotInsertOutcome::Inserted(_)
        ));
        assert!(matches!(
            inserted_later,
            FxRateSnapshotInsertOutcome::Inserted(_)
        ));
        assert!(matches!(
            insert(
                pool.clone(),
                &snapshot(OffsetDateTime::UNIX_EPOCH),
                "event-earlier"
            )
            .await?,
            FxRateSnapshotInsertOutcome::Duplicate
        ));

        let reader = SqlxFxRateSnapshotReader::new(pool);
        let latest = reader.latest().await?.ok_or("latest snapshot missing")?;
        assert_eq!(later.id(), latest.id());
        assert_eq!(2, latest.generation().as_i64());
        assert_eq!(Currency::iter().count(), latest.quotes().len());
        assert_eq!(
            Some(earlier.id()),
            reader
                .latest_at_or_before(OffsetDateTime::UNIX_EPOCH)
                .await?
                .map(|snapshot| snapshot.id())
        );
        assert_eq!(
            Some(later.id()),
            reader
                .find_by_id(later.id())
                .await?
                .map(|snapshot| snapshot.id())
        );
        let batch = reader.find_by_ids(&[later.id(), earlier.id()]).await?;
        assert_eq!(
            vec![earlier.id(), later.id()],
            batch
                .iter()
                .map(|snapshot| snapshot.id())
                .collect::<Vec<_>>()
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "FX snapshot repository test failed: {result:?}"
    );
}
