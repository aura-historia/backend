//! Batched product title translation: a round every [`ROUND_INTERVAL`] collects ended Anthropic
//! Message Batches, translates titles the primary model refused on the fallback model, and
//! submits the pending work as a new batch. It runs in every worker that consumes
//! `product-translation`, whose jobs only record the work.

use crate::composition::CompositionError;
use application::error::find_source;
use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use large_language_model::{
    AnthropicClaude, AnthropicConfig, ClaudeHaiku55Thinking, ClaudeModel,
    ClaudeReducedThinkingEffort, ClaudeSonnet55Thinking, LargeLanguageModel,
    LargeLanguageModelBatches, LargeLanguageModelError,
};
use platform_postgres::SqlxUnitOfWork;
use platform_process::{Dependency, FailureDiagnostic, UNCLASSIFIED_FAILURE, config::required};
use product_listing_postgres::{
    SqlxProductListingTitleTranslationQueueFactory, SqlxProductListingTitleTranslationQueueReader,
    SqlxProductListingTranslationWriterFactory,
};
use product_listing_service::use_cases::{
    TitleTranslationRoundReport, TranslateProductListingTitleBatchesError,
    TranslateProductListingTitleBatchesHandler, TranslateProductListingTitleBatchesUseCase,
};
use product_listing_translation_llm::{
    LargeLanguageModelProductListingTitleBatchTranslator,
    LargeLanguageModelRefusalFallbackTitleTranslator,
};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

const SCOPE: &str = "product-translation";
const ANTHROPIC_API_KEY_ENV: &str = "ANTHROPIC_API_KEY";
/// How often a round runs. Pending work accumulates in between, so titles of one source language
/// fill their requests; most batches end within an hour anyway, and reads fall back to the
/// original title until translations land.
pub const ROUND_INTERVAL: Duration = Duration::from_secs(15 * 60);
// Title translation needs no reasoning, so thinking stays off and the output budget remains
// reserved for the structured answer.
const TRANSLATION_MODEL: ClaudeModel = ClaudeModel::Haiku55(ClaudeHaiku55Thinking::Disabled(
    ClaudeReducedThinkingEffort::Low,
));
// Haiku's safety classifier refuses some old scientific titles (pathogens, aerosols) that Sonnet
// translates (#3076). Only a title Haiku refused alone reaches Sonnet. Without tools,
// `BetweenTools` answers without thinking first, the closest Sonnet 5.5 gets to thinking disabled.
const REFUSAL_FALLBACK_MODEL: ClaudeModel = ClaudeModel::Sonnet55(
    ClaudeSonnet55Thinking::BetweenTools(ClaudeReducedThinkingEffort::Low),
);

/// Builds the PostgreSQL-backed round with Haiku batches and the Sonnet refusal fallback.
pub async fn compose(
    pool: &PgPool,
) -> Result<Arc<dyn TranslateProductListingTitleBatchesUseCase>, CompositionError> {
    let api_key = required(ANTHROPIC_API_KEY_ENV).map_err(|e| CompositionError::new(SCOPE, e))?;
    // The client error is replaced, never chained, so no part of the key can reach a log.
    let client = |model| {
        AnthropicClaude::new(AnthropicConfig::new(model), &api_key)
            .map_err(|_| CompositionError::new(SCOPE, "failed to configure the Anthropic client"))
    };
    Ok(compose_round_use_case(
        pool.clone(),
        client(TRANSLATION_MODEL)?,
        client(REFUSAL_FALLBACK_MODEL)?,
    ))
}

pub fn compose_round_use_case<B, F>(
    pool: PgPool,
    batches: B,
    refusal_fallback: F,
) -> Arc<dyn TranslateProductListingTitleBatchesUseCase>
where
    B: LargeLanguageModelBatches + 'static,
    F: LargeLanguageModel + 'static,
{
    Arc::new(TranslateProductListingTitleBatchesHandler::new(
        SqlxProductListingTitleTranslationQueueReader::new(pool.clone()),
        LargeLanguageModelProductListingTitleBatchTranslator::new(batches),
        LargeLanguageModelRefusalFallbackTitleTranslator::new(refusal_fallback),
        SqlxUnitOfWork::new(pool),
        SqlxProductListingTitleTranslationQueueFactory::new(),
        SqlxProductListingTranslationWriterFactory::new(),
    ))
}

/// Runs a round at once and then every `interval` until `shutdown`. A failed round leaves its
/// work durable in the queue; the next round continues from there.
pub async fn run(
    use_case: Arc<dyn TranslateProductListingTitleBatchesUseCase>,
    interval: Duration,
    shutdown: CancellationToken,
) {
    let mut round = 0_u64;
    while !shutdown.is_cancelled() {
        round += 1;
        let context = OperationContext {
            principal: Principal::System,
            request_id: RequestId::new(format!("product-translation-round:{round}")),
            correlation_id: CorrelationId::new(format!("product-translation-round:{round}")),
        };
        let outcome = tokio::select! {
            () = shutdown.cancelled() => return,
            outcome = use_case.execute(&context) => outcome,
        };
        match outcome {
            Ok(report) => log_round(&report),
            Err(error) => log_failed_round(&error),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
    }
}

fn log_round(report: &TitleTranslationRoundReport) {
    tracing::info!(
        event = "product.translation.round.completed",
        abandoned_batches = report.abandoned_batches,
        batches_in_progress = report.batches_in_progress,
        batches_unavailable = report.batches_unavailable,
        batches_collected = report.batches_collected,
        applied = report.applied,
        duplicate = report.duplicate,
        stale = report.stale,
        requeued = report.requeued,
        parked = report.parked,
        refusal_fallbacks = report.refusal_fallbacks,
        superseded_discarded = report.superseded_discarded,
        submitted_groups = report.submitted_groups,
        submitted_titles = report.submitted_titles,
    );
}

/// Records the failed stage and safe cause facts; the error chain itself is never logged.
fn log_failed_round(error: &TranslateProductListingTitleBatchesError) {
    let Some(failure) = round_failure(error) else {
        tracing::error!(
            event = "product.translation.round.failed",
            stage = "authorization",
            "Title translation round was not authorized"
        );
        return;
    };
    tracing::warn!(
        event = "product.translation.round.failed",
        stage = failure.stage(),
        dependency = failure.dependency(),
        error_category = failure.category(),
        http_status = failure.http_status(),
        sqlstate = failure.sqlstate(),
        "Title translation round failed; the next round continues its work"
    );
}

fn round_failure(error: &TranslateProductListingTitleBatchesError) -> Option<FailureDiagnostic> {
    use TranslateProductListingTitleBatchesError as E;
    let stage = match error {
        E::SystemPrincipalRequired => return None,
        E::SubmissionFailed { .. } => {
            let category = find_source::<LargeLanguageModelError>(error).map_or(
                UNCLASSIFIED_FAILURE,
                LargeLanguageModelError::diagnostic_category,
            );
            return Some(FailureDiagnostic::new(
                "batch_submit",
                Dependency::Anthropic,
                category,
                None,
            ));
        }
        E::QueueFailed { .. } => "queue",
        E::BeginTransactionFailed { .. } => "write_begin",
        E::TranslationWriteFailed { .. } => "translation_write",
        E::CommitTransactionFailed { .. } => "write_commit",
    };
    Some(FailureDiagnostic::postgres(stage, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use test_api::CapturedLogs;

    struct FailingRounds {
        calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl TranslateProductListingTitleBatchesUseCase for FailingRounds {
        async fn execute(
            &self,
            _: &OperationContext,
        ) -> Result<TitleTranslationRoundReport, TranslateProductListingTitleBatchesError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(TranslateProductListingTitleBatchesError::SubmissionFailed {
                source: Box::new(LargeLanguageModelError::Authentication {
                    source: Box::new(std::io::Error::other("SENTINEL key detail")),
                }),
            })
        }
    }

    #[tokio::test]
    async fn should_keep_running_rounds_after_a_failure_and_log_only_safe_facts() {
        let rounds = Arc::new(FailingRounds {
            calls: AtomicUsize::new(0),
        });
        let shutdown = CancellationToken::new();
        let (logs, guard) = CapturedLogs::start();
        let task = tokio::spawn(run(
            rounds.clone(),
            Duration::from_millis(10),
            shutdown.clone(),
        ));

        while rounds.calls.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        shutdown.cancel();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("the round loop stops on shutdown")
            .unwrap();
        drop(guard);

        let failed = logs.events_where("event", "product.translation.round.failed");
        assert!(failed.len() >= 2);
        assert_eq!(Some("batch_submit"), failed[0]["stage"].as_str());
        assert_eq!(Some("anthropic"), failed[0]["dependency"].as_str());
        assert_eq!(Some("authentication"), failed[0]["error_category"].as_str());
        assert!(!logs.text().contains("SENTINEL"));
    }
}
