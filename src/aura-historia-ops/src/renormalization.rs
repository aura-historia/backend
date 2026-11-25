//! `renormalize-raw`: normalizes the latest raw revision of every caught-up stream again with the
//! current normalizer ([production reapplication](../../../docs/normalization-replay.md#production-reapplication)).

use crate::OpsError;
use platform_postgres::SqlxUnitOfWork;
use platform_process::Shutdown;
use product_listing_postgres::{
    SqlxNormalizationDiagnosticCaseWriterFactory, SqlxPendingProductListingRawStreamReader,
    SqlxProductListingEventAppenderFactory, SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::ports::ProductListingRawStreamId;
use product_service::use_cases::{
    CompletedRawRevisionReapplication, NormalizeProductListingRawRevisionHandler,
    RawRevisionReapplication, RawRevisionReapplicationMode, ReappliedProductListingRawStream,
    ReapplyProductListingRawRevisionsCommand, ReapplyProductListingRawRevisionsUseCase,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const COMMAND: &str = "renormalize-raw";
const BACKLOG_POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Stream failures in a row that end a run, as no single listing's fault explains them.
const MAX_CONSECUTIVE_FAILURES: u64 = 50;

/// What a `renormalize-raw` run covers and how fast it may write.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenormalizationOptions {
    pub mode: RawRevisionReapplicationMode,
    pub listing_source_id: Option<listing_source_core::ListingSourceId>,
    pub page_size: NonZeroU32,
    /// Stop after scanning this many streams.
    pub limit: Option<u64>,
    pub after: Option<ProductListingRawStreamId>,
    /// Applying waits before each page while the outbox and job queue hold more than this.
    pub max_backlog: u64,
}

pub(crate) async fn renormalize_raw(
    pool: &PgPool,
    options: &RenormalizationOptions,
) -> Result<Value, OpsError> {
    let stop = stop_on_signal()?;
    let use_case = compose(pool.clone());
    let mut report = Report::default();
    let mut after = options.after;
    let mut remaining = options.limit;
    while !stop.load(Ordering::Relaxed) && remaining != Some(0) {
        if options.mode == RawRevisionReapplicationMode::Apply {
            wait_for_backlog(pool, options.max_backlog, &stop).await?;
            if stop.load(Ordering::Relaxed) {
                break;
            }
        }
        let page_size = remaining.map_or(options.page_size.get(), |remaining| {
            u32::try_from(remaining.min(u64::from(options.page_size.get())))
                .unwrap_or(options.page_size.get())
        });
        let page = use_case
            .reapply_page(ReapplyProductListingRawRevisionsCommand {
                mode: options.mode,
                after,
                page_size,
                listing_source_id: options.listing_source_id,
            })
            .await
            .map_err(|error| OpsError::failed(COMMAND, error))?;
        let Some(last) = page.last else {
            report.finished = true;
            break;
        };
        report.pages += 1;
        remaining = remaining.map(|remaining| remaining.saturating_sub(u64::from(page_size)));
        for stream in &page.streams {
            report.add(stream);
        }
        if report.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
            // A systemic cause, such as an unreachable database. Failed streams recorded nothing,
            // so resuming before this page retries them.
            let partial = report.into_json(options.mode, after);
            tracing::error!(event = "ops.renormalize_raw.aborted", report = %partial);
            return Err(OpsError::failed(
                COMMAND,
                std::io::Error::other("too many consecutive stream failures"),
            ));
        }
        after = Some(last);
        // A run takes hours; the last logged position resumes one that was killed.
        tracing::info!(
            event = "ops.renormalize_raw.page",
            last = %last,
            reapplied = report.reapplied,
            changed_listings = report.changed_listings,
            failed = report.failed.values().sum::<u64>(),
        );
    }
    Ok(report.into_json(options.mode, after))
}

type Handler = NormalizeProductListingRawRevisionHandler<
    SqlxUnitOfWork,
    SqlxProductListingRawNormalizationWriterFactory,
    SqlxProductListingRepositoryFactory,
    SqlxProductListingEventAppenderFactory,
    SqlxPendingProductListingRawStreamReader,
    SqlxNormalizationDiagnosticCaseWriterFactory,
>;

/// The normalization composition of the worker's `product-listing-normalization` scope.
fn compose(pool: PgPool) -> Handler {
    let shipping = product_listing_service::crawler_shipping::ApplyCrawlerShippingHandler::new(
        product_listing_postgres::delivery::SqlxCrawlerShippingSourceReader::new(pool.clone()),
        product_listing_service::use_cases::ChangeProductListingDeliveryHandler::new(
            SqlxUnitOfWork::new(pool.clone()),
            SqlxProductListingRepositoryFactory::new(),
            SqlxProductListingEventAppenderFactory::new(),
            product_listing_postgres::delivery::SqlxProductListingDeliveryReferencesFactory,
        ),
    );
    NormalizeProductListingRawRevisionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRawNormalizationWriterFactory::new(),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        SqlxPendingProductListingRawStreamReader::new(pool),
    )
    .with_diagnostic_cases(SqlxNormalizationDiagnosticCaseWriterFactory::new())
    .with_crawler_shipping(Arc::new(shipping))
}

/// SIGTERM or SIGINT ends the run after the current page.
fn stop_on_signal() -> Result<Arc<AtomicBool>, OpsError> {
    let shutdown = Shutdown::register().map_err(|error| OpsError::failed(COMMAND, error))?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&stop);
    tokio::spawn(async move {
        shutdown.received().await;
        flag.store(true, Ordering::Relaxed);
    });
    Ok(stop)
}

/// Waits while the outbox and the job queue together hold more than `max_backlog` entries, so
/// the events of a run drain at the workers' pace instead of piling up.
async fn wait_for_backlog(
    pool: &PgPool,
    max_backlog: u64,
    stop: &AtomicBool,
) -> Result<(), OpsError> {
    loop {
        let backlog: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM delivery_outbox) + (SELECT count(*) FROM job_queue_jobs)",
        )
        .fetch_one(pool)
        .await
        .map_err(|error| OpsError::failed(COMMAND, error))?;
        if u64::try_from(backlog).unwrap_or(u64::MAX) <= max_backlog || stop.load(Ordering::Relaxed)
        {
            return Ok(());
        }
        tracing::info!(
            event = "ops.renormalize_raw.waiting_for_backlog",
            backlog,
            max_backlog
        );
        tokio::time::sleep(BACKLOG_POLL_INTERVAL).await;
    }
}

#[derive(Debug, Default)]
struct Report {
    pages: u64,
    finished: bool,
    reapplied: u64,
    skipped: BTreeMap<&'static str, u64>,
    failed: BTreeMap<&'static str, u64>,
    transitions: BTreeMap<String, u64>,
    changed_listings: u64,
    changes: BTreeMap<&'static str, u64>,
    error_codes: BTreeMap<&'static str, u64>,
    issue_reasons: BTreeMap<&'static str, u64>,
    consecutive_failures: u64,
}

impl Report {
    fn add(&mut self, stream: &ReappliedProductListingRawStream) {
        match &stream.result {
            Err(error) => {
                let code = error.diagnostic_code();
                // Only the static code: the error chain can carry database text.
                tracing::warn!(
                    event = "ops.renormalize_raw.stream_failed",
                    product_listing_raw_stream_id = %stream.product_listing_raw_stream_id,
                    code,
                );
                *self.failed.entry(code).or_default() += 1;
                self.consecutive_failures += 1;
            }
            Ok(RawRevisionReapplication::Skipped(skip)) => {
                *self.skipped.entry(skip.as_str()).or_default() += 1;
                self.consecutive_failures = 0;
            }
            Ok(RawRevisionReapplication::Completed(reapplication)) => {
                self.add_completed(reapplication);
                self.consecutive_failures = 0;
            }
        }
    }

    fn add_completed(&mut self, reapplication: &CompletedRawRevisionReapplication) {
        self.reapplied += 1;
        *self
            .transitions
            .entry(format!(
                "{}->{}",
                reapplication.recorded_outcome.as_str(),
                reapplication.outcome.as_str()
            ))
            .or_default() += 1;
        let changes = reapplication.changes.names();
        if !changes.is_empty() {
            self.changed_listings += 1;
        }
        for change in changes {
            *self.changes.entry(change).or_default() += 1;
        }
        if let Some(code) = reapplication.error_code {
            *self.error_codes.entry(code).or_default() += 1;
        }
        for reason in &reapplication.issue_reasons {
            *self.issue_reasons.entry(reason.as_str()).or_default() += 1;
        }
    }

    fn into_json(
        self,
        mode: RawRevisionReapplicationMode,
        last: Option<ProductListingRawStreamId>,
    ) -> Value {
        json!({
            "mode": match mode {
                RawRevisionReapplicationMode::DryRun => "DRY_RUN",
                RawRevisionReapplicationMode::Apply => "APPLY",
            },
            "pages": self.pages,
            // True once every stream was scanned; otherwise pass `last` as --after to continue.
            "finished": self.finished,
            "last": last.map(|id| id.to_string()),
            "reapplied": self.reapplied,
            "transitions": self.transitions,
            "changedListings": self.changed_listings,
            "changes": self.changes,
            "errorCodes": self.error_codes,
            "issueReasons": self.issue_reasons,
            "skipped": self.skipped,
            "failed": self.failed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_service::ports::ProductListingRawNormalizationOutcome;
    use product_service::use_cases::{
        NormalizeProductListingRawRevisionError, ProductListingChanges,
        RawRevisionReapplicationSkip,
    };

    fn stream(
        result: Result<RawRevisionReapplication, NormalizeProductListingRawRevisionError>,
    ) -> ReappliedProductListingRawStream {
        ReappliedProductListingRawStream {
            product_listing_raw_stream_id: ProductListingRawStreamId::new(),
            result,
        }
    }

    #[test]
    fn should_tally_outcomes_and_count_only_consecutive_failures() {
        let mut report = Report::default();

        report.add(&stream(Err(
            NormalizeProductListingRawRevisionError::InvalidLimit,
        )));
        report.add(&stream(Err(
            NormalizeProductListingRawRevisionError::InvalidLimit,
        )));
        report.add(&stream(Ok(RawRevisionReapplication::Completed(
            CompletedRawRevisionReapplication {
                recorded_outcome: ProductListingRawNormalizationOutcome::Rejected,
                outcome: ProductListingRawNormalizationOutcome::Applied,
                error_code: None,
                changes: ProductListingChanges {
                    price: true,
                    ..ProductListingChanges::default()
                },
                issue_reasons: Vec::new(),
            },
        ))));
        report.add(&stream(Ok(RawRevisionReapplication::Skipped(
            RawRevisionReapplicationSkip::ListingWithdrawn,
        ))));
        report.add(&stream(Err(
            NormalizeProductListingRawRevisionError::InvalidLimit,
        )));

        assert_eq!(1, report.consecutive_failures);
        let json = report.into_json(RawRevisionReapplicationMode::DryRun, None);
        assert_eq!(json!({"REJECTED->APPLIED": 1}), json["transitions"]);
        assert_eq!(json!({"price": 1}), json["changes"]);
        assert_eq!(json!(1), json["changedListings"]);
        assert_eq!(json!({"LISTING_WITHDRAWN": 1}), json["skipped"]);
        assert_eq!(json!({"INVALID_LIMIT": 3}), json["failed"]);
        assert_eq!(json!("DRY_RUN"), json["mode"]);
    }
}
