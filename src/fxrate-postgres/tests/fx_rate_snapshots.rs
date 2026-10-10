use application::transaction::{Transaction, UnitOfWork};
use fxrate_core::{FX_RATE_SCALE, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot};
use fxrate_postgres::{SqlxFxRateSnapshotReader, SqlxFxRateSnapshotRepositoryFactory};
use fxrate_service::ports::{
    FxRateSnapshotInsertOutcome, FxRateSnapshotReader, FxRateSnapshotRepository,
    FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
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
async fn should_insert_idempotently_and_rehydrate_persisted_snapshots() {
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

        let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let repository_factory = SqlxFxRateSnapshotRepositoryFactory::new();
        let latest = repository_factory
            .in_transaction(&mut transaction)
            .find_latest()
            .await?
            .ok_or("latest snapshot missing")?;
        assert_eq!(later.id(), latest.id());
        assert_eq!(2, latest.generation().as_i64());
        assert_eq!(Currency::iter().count(), latest.quotes().len());
        assert_eq!(
            Some(earlier.id()),
            repository_factory
                .in_transaction(&mut transaction)
                .find_latest_at_or_before(OffsetDateTime::UNIX_EPOCH)
                .await?
                .map(|snapshot| snapshot.id())
        );
        assert_eq!(
            Some(later.id()),
            repository_factory
                .in_transaction(&mut transaction)
                .find_by_id(later.id())
                .await?
                .map(|snapshot| snapshot.id())
        );
        let batch = repository_factory
            .in_transaction(&mut transaction)
            .find_by_ids(&[later.id(), earlier.id()])
            .await?;
        assert_eq!(
            vec![earlier.id(), later.id()],
            batch
                .iter()
                .map(|snapshot| snapshot.id())
                .collect::<Vec<_>>()
        );
        transaction.commit().await?;

        let reader = SqlxFxRateSnapshotReader::new(pool);
        assert_eq!(
            Some(latest),
            reader
                .find_latest_at_or_before(OffsetDateTime::now_utc())
                .await?
        );
        assert_eq!(
            Some(earlier.id()),
            reader
                .find_latest_at_or_before(OffsetDateTime::UNIX_EPOCH)
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
        assert_eq!(
            None,
            reader
                .find_latest_at_or_before(OffsetDateTime::UNIX_EPOCH - Duration::seconds(1))
                .await?
        );
        assert_eq!(None, reader.find_by_id(FxRateId::new()).await?);
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "FX snapshot repository test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_select_distinct_historical_snapshots_in_one_batch() {
    let pool = get_postgres_client().await;
    let at = OffsetDateTime::UNIX_EPOCH;
    let first = snapshot(at);
    let second = snapshot(at + Duration::hours(1));
    let unselected = snapshot(at + Duration::hours(2));
    let last = snapshot(at + Duration::hours(3));
    for (snapshot, event) in [
        (&first, "batch-first"),
        (&second, "batch-second"),
        (&unselected, "batch-unselected"),
        (&last, "batch-last"),
    ] {
        insert(pool.clone(), snapshot, event).await.unwrap();
    }
    let reader = SqlxFxRateSnapshotReader::new(pool.clone());
    let selected = reader
        .find_latest_at_or_before_many(&[
            last.captured_at(),
            at - Duration::seconds(1),
            second.captured_at(),
            at,
            at + Duration::minutes(30),
            at,
            second.captured_at(),
        ])
        .await
        .unwrap();
    assert_eq!(
        vec![first.id(), second.id(), last.id()],
        selected
            .iter()
            .map(|snapshot| snapshot.id())
            .collect::<Vec<_>>()
    );
    assert!(
        reader
            .find_latest_at_or_before_many(&[])
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        reader
            .find_latest_at_or_before_many(&[at - Duration::seconds(1)])
            .await
            .unwrap()
            .is_empty()
    );
    // A valid complete legacy snapshot stays usable by the batch reader.
    sqlx::query("DELETE FROM fx_rate_quotes WHERE fx_rate_id = $1 AND currency = ANY($2)")
        .bind(first.id().into_uuid())
        .bind(vec![
            "SEK", "DKK", "NOK", "KRW", "INR", "TWD", "HUF", "RON", "MXN", "THB",
        ])
        .execute(&pool)
        .await
        .unwrap();
    let legacy = reader.find_latest_at_or_before_many(&[at]).await.unwrap();
    assert_eq!(19, legacy[0].quotes().len());
    // Partial persisted snapshots must still fail instead of looking unavailable.
    sqlx::query("DELETE FROM fx_rate_quotes WHERE fx_rate_id = $1 AND currency = 'USD'")
        .bind(first.id().into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        reader.find_latest_at_or_before_many(&[at]).await,
        Err(fxrate_service::ports::FxRateSnapshotReadError::InvalidPersistedSnapshot { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_reject_retroactive_or_tied_canonical_capture_except_duplicate_source_event() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let captured_at = OffsetDateTime::UNIX_EPOCH + Duration::hours(2);
        let canonical = snapshot(captured_at);
        assert!(matches!(
            insert(pool.clone(), &canonical, "event-canonical").await?,
            FxRateSnapshotInsertOutcome::Inserted(_)
        ));

        let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let retroactive = SqlxFxRateSnapshotRepositoryFactory::new()
            .in_transaction(&mut transaction)
            .insert(
                &snapshot(captured_at - Duration::seconds(1)),
                "event-retroactive",
            )
            .await;
        assert!(matches!(
            retroactive,
            Err(FxRateSnapshotRepositoryError::CapturedAtNotMonotonic)
        ));
        drop(transaction);

        let mut transaction = SqlxUnitOfWork::new(pool.clone()).begin().await?;
        let tied = SqlxFxRateSnapshotRepositoryFactory::new()
            .in_transaction(&mut transaction)
            .insert(&snapshot(captured_at), "event-tied")
            .await;
        assert!(matches!(
            tied,
            Err(FxRateSnapshotRepositoryError::CapturedAtNotMonotonic)
        ));
        drop(transaction);

        assert!(matches!(
            insert(
                pool,
                &snapshot(captured_at - Duration::hours(1)),
                "event-canonical"
            )
            .await?,
            FxRateSnapshotInsertOutcome::Duplicate
        ));
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "FX canonical capture monotonicity test failed: {result:?}"
    );
}
