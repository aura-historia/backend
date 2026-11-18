use async_trait::async_trait;
use aura_historia_ops::{FxRefreshRun, capture_fx_rate_snapshot};
use fxrate_postgres::SqlxFxRateSnapshotRepositoryFactory;
use fxrate_service::{
    CaptureFxRateSnapshotHandler,
    ports::{FxRateQuote, FxRateQuoteProvider, FxRateQuoteProviderError, FxRateQuoteSet},
};
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use strum::IntoEnumIterator;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Date, Month};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

struct Quotes {
    complete: bool,
}

#[async_trait]
impl FxRateQuoteProvider for Quotes {
    async fn fetch_eur_quotes(&self) -> Result<FxRateQuoteSet, FxRateQuoteProviderError> {
        let mut quotes = Currency::iter()
            .filter(|currency| *currency != Currency::Eur)
            .map(|currency| FxRateQuote {
                currency,
                units_per_eur: 1_250_000,
            })
            .collect::<Vec<_>>();
        if !self.complete {
            quotes.pop();
        }
        Ok(FxRateQuoteSet {
            base: Currency::Eur,
            quotes,
        })
    }
}

/// A run started at `hour:minute` UTC on one fixed day.
fn run(hour: u8, minute: u8) -> FxRefreshRun {
    FxRefreshRun::at(
        Date::from_calendar_date(2026, Month::November, 17)
            .unwrap()
            .with_hms(hour, minute, 0)
            .unwrap()
            .assume_utc(),
    )
}

fn snapshots(
    pool: sqlx::PgPool,
    complete: bool,
) -> CaptureFxRateSnapshotHandler<Quotes, SqlxUnitOfWork, SqlxFxRateSnapshotRepositoryFactory> {
    CaptureFxRateSnapshotHandler::new(
        Quotes { complete },
        SqlxUnitOfWork::new(pool),
        SqlxFxRateSnapshotRepositoryFactory::new(),
    )
}

async fn stored(pool: &sqlx::PgPool, run: &FxRefreshRun) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM fx_rates WHERE source_event_id = $1")
        .bind(run.source_event_id())
        .fetch_one(pool)
        .await
        .expect("count stored snapshots")
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_not_persist_partial_snapshot_and_should_deduplicate_retry_of_the_same_slot() {
    let pool = get_postgres_client().await;

    let incomplete = run(5, 0);
    assert!(
        capture_fx_rate_snapshot(&snapshots(pool.clone(), false), &incomplete)
            .await
            .is_err()
    );
    assert_eq!(0, stored(&pool, &incomplete).await);

    let service = snapshots(pool.clone(), true);
    let scheduled = run(6, 0);
    let captured = capture_fx_rate_snapshot(&service, &scheduled)
        .await
        .expect("capture the scheduled snapshot");
    assert_eq!(Some("captured"), captured["outcome"].as_str());
    assert_eq!(
        Some("fxrate:2026-11-17T06:00:00Z"),
        captured["sourceEventId"].as_str()
    );
    let retried = capture_fx_rate_snapshot(&service, &run(6, 40))
        .await
        .expect("a retry of the same slot is a duplicate");
    assert_eq!(Some("duplicate"), retried["outcome"].as_str());
    assert_eq!(1, stored(&pool, &scheduled).await);

    let next_slot = run(18, 0);
    capture_fx_rate_snapshot(&service, &next_slot)
        .await
        .expect("capture the next slot");
    assert_eq!(1, stored(&pool, &next_slot).await);
}
