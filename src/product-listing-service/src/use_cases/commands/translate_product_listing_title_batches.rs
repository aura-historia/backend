//! One round of batched title translation: collect ended provider batches and commit their
//! results under the translation source-marker guard, translate titles the primary model refused
//! on the refusal fallback, and submit pending work as a new batch.
//!
//! Work is durable in the translation queue, so a round that fails part-way leaves every title
//! pending, submitted or settled, and the next round continues from there.

use crate::ports::{
    BatchedTitleTranslation, PendingTitleTranslation, PositionedTitle,
    ProductListingTitleBatchTranslator, ProductListingTitleTranslationError,
    ProductListingTitleTranslationQueue, ProductListingTitleTranslationQueueError,
    ProductListingTitleTranslationQueueFactory, ProductListingTitleTranslationQueueReader,
    ProductListingTitleTranslator, ProductListingTranslationWrite,
    ProductListingTranslationWriteError, ProductListingTranslationWriteOutcome,
    ProductListingTranslationWriter, ProductListingTranslationWriterFactory,
    SubmittedTitleTranslationBatch, TitleTranslationAssignment, TitleTranslationBatchId,
    TitleTranslationBatchKey, TitleTranslationBatchPoll, TitleTranslationClaim,
    TitleTranslationGroup, TitleTranslationGroupOutcome, TitleTranslationMode,
    TitleTranslationWorkRef,
};
use application::error::{BoxError, box_error};
use application::operation_context::{OperationAuthorizationError, OperationContext};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::IndexMap;
use localization::Language;
use product_listing_core::title::Title;
use std::collections::BTreeMap;
use std::time::Duration;

/// Titles one batch submits at most.
pub const MAX_TITLES_PER_BATCH: u32 = 10_000;
/// Failed attempts after which a title is parked until its title changes.
pub const MAX_TRANSLATION_ATTEMPTS: u16 = 3;
/// Refused titles one round translates synchronously at most.
const REFUSAL_FALLBACKS_PER_ROUND: u32 = 50;
/// A batch whose submission was not recorded within this time never reached the provider, or
/// reached it without its ID being kept; either way its work returns to the queue.
const ABANDONED_SUBMISSION_AGE: Duration = Duration::from_secs(10 * 60);

/// The catalogue languages every title is translated into, except its own.
pub(crate) fn translation_targets(source_language: Language) -> Vec<Language> {
    [
        Language::De,
        Language::En,
        Language::Fr,
        Language::Es,
        Language::It,
    ]
    .into_iter()
    .filter(|language| *language != source_language)
    .collect()
}

/// What one round did, for its log record.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TitleTranslationRoundReport {
    pub abandoned_batches: u64,
    pub batches_in_progress: usize,
    pub batches_unavailable: usize,
    pub batches_collected: usize,
    /// Batches whose collection failed this round; the next round retries them.
    pub batches_failed: usize,
    pub applied: usize,
    pub duplicate: usize,
    pub stale: usize,
    pub requeued: usize,
    pub parked: usize,
    /// Results whose guarded write failed; their titles spent an attempt.
    pub write_failures: usize,
    pub refusal_fallbacks: usize,
    pub superseded_discarded: u64,
    pub submitted_groups: usize,
    pub submitted_titles: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum TranslateProductListingTitleBatchesError {
    #[error("system principal required for product title translation batches")]
    SystemPrincipalRequired,
    #[error("product title translation queue failed")]
    QueueFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin product title translation transaction")]
    BeginTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to persist product translations")]
    TranslationWriteFailed {
        #[source]
        source: BoxError,
    },
    #[error("failed to commit product title translation transaction")]
    CommitTransactionFailed {
        #[source]
        source: BoxError,
    },
    #[error("product title translation batch submission failed")]
    SubmissionFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait TranslateProductListingTitleBatchesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<TitleTranslationRoundReport, TranslateProductListingTitleBatchesError>;
}

pub struct TranslateProductListingTitleBatchesHandler<R, B, F, U, Q, W> {
    reader: R,
    batch_translator: B,
    refusal_fallback: F,
    unit_of_work: U,
    queue: Q,
    translations: W,
}

impl<R, B, F, U, Q, W> TranslateProductListingTitleBatchesHandler<R, B, F, U, Q, W> {
    pub fn new(
        reader: R,
        batch_translator: B,
        refusal_fallback: F,
        unit_of_work: U,
        queue: Q,
        translations: W,
    ) -> Self {
        Self {
            reader,
            batch_translator,
            refusal_fallback,
            unit_of_work,
            queue,
            translations,
        }
    }
}

/// What becomes of one title's work.
#[derive(Debug, PartialEq)]
enum Settlement {
    Commit(IndexMap<Language, Title>),
    Retry(Retry),
}

/// Work that is not settled yet: another attempt later, or none until its title changes.
#[derive(Debug, PartialEq)]
enum Retry {
    Requeue(TitleTranslationMode, u16),
    Park(u16),
}

type Error = TranslateProductListingTitleBatchesError;

#[async_trait::async_trait]
impl<R, B, F, U, Q, W> TranslateProductListingTitleBatchesUseCase
    for TranslateProductListingTitleBatchesHandler<R, B, F, U, Q, W>
where
    R: ProductListingTitleTranslationQueueReader,
    B: ProductListingTitleBatchTranslator,
    F: ProductListingTitleTranslator,
    U: UnitOfWork,
    Q: ProductListingTitleTranslationQueueFactory<U::Tx>,
    W: ProductListingTranslationWriterFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "translate_product_title_batches",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<TitleTranslationRoundReport, Error> {
        context.require().system().authorize::<Error>()?;
        let mut report = TitleTranslationRoundReport {
            abandoned_batches: self.close_abandoned_submissions().await?,
            ..Default::default()
        };
        // One batch that cannot be collected must not hold back the others or new work.
        for batch in self.reader.submitted_batches().await? {
            if let Err(error) = self.collect(&batch, &mut report).await {
                report.batches_failed += 1;
                tracing::warn!(
                    event = "product.translation.batch_failed",
                    provider_batch_id = %batch.provider_batch_id,
                    error_kind = error.kind(),
                    "Translation batch collection failed; the next round retries it"
                );
            }
        }
        self.translate_refusal_fallbacks(&mut report).await?;
        self.submit_pending(&mut report).await?;
        Ok(report)
    }
}

impl<R, B, F, U, Q, W> TranslateProductListingTitleBatchesHandler<R, B, F, U, Q, W>
where
    R: ProductListingTitleTranslationQueueReader,
    B: ProductListingTitleBatchTranslator,
    F: ProductListingTitleTranslator,
    U: UnitOfWork,
    Q: ProductListingTitleTranslationQueueFactory<U::Tx>,
    W: ProductListingTranslationWriterFactory<U::Tx>,
{
    /// Settles every current title of an ended batch, then closes it. A batch whose results
    /// cannot be read now is left for the next round; one whose results the provider rejected
    /// for good fails every title once.
    async fn collect(
        &self,
        batch: &SubmittedTitleTranslationBatch,
        report: &mut TitleTranslationRoundReport,
    ) -> Result<(), Error> {
        let items = self.reader.batch_items(batch.key).await?;
        let groups = groups_of(&items);
        let results = match self
            .batch_translator
            .poll(&batch.provider_batch_id, &groups)
            .await
        {
            Ok(TitleTranslationBatchPoll::InProgress) => {
                report.batches_in_progress += 1;
                return Ok(());
            }
            Ok(TitleTranslationBatchPoll::Ended(results)) => results
                .into_iter()
                .map(|result| (result.group, result.outcome))
                .collect(),
            Err(ProductListingTitleTranslationError::TemporarilyUnavailable { .. }) => {
                report.batches_unavailable += 1;
                return Ok(());
            }
            Err(error) => {
                tracing::warn!(
                    event = "product.translation.batch_unreadable",
                    provider_batch_id = %batch.provider_batch_id,
                    "Translation batch results were rejected; its titles are retried"
                );
                groups
                    .iter()
                    .map(|group| {
                        (
                            group.group,
                            TitleTranslationGroupOutcome::Failed(rejected_batch(&error)),
                        )
                    })
                    .collect()
            }
        };
        let claim = TitleTranslationClaim::Batch(batch.key);
        for (pending, settlement) in settlements(items, results) {
            self.settle(&pending, claim, settlement, report).await?;
        }
        self.close_batch(batch.key).await?;
        report.batches_collected += 1;
        Ok(())
    }

    async fn translate_refusal_fallbacks(
        &self,
        report: &mut TitleTranslationRoundReport,
    ) -> Result<(), Error> {
        for pending in self
            .reader
            .refusal_fallbacks(REFUSAL_FALLBACKS_PER_ROUND)
            .await?
        {
            report.refusal_fallbacks += 1;
            let result = self
                .refusal_fallback
                .translate(
                    &pending.title,
                    pending.source_language,
                    &translation_targets(pending.source_language),
                )
                .await;
            let settlement = fallback_settlement(&pending, result);
            self.settle(
                &pending,
                TitleTranslationClaim::RefusalFallback,
                settlement,
                report,
            )
            .await?;
        }
        Ok(())
    }

    async fn submit_pending(&self, report: &mut TitleTranslationRoundReport) -> Result<(), Error> {
        let key = TitleTranslationBatchKey::new();
        let mut tx = self.begin().await?;
        let groups = {
            let mut queue = self.queue.in_transaction(&mut tx);
            report.superseded_discarded = queue.discard_superseded().await?;
            let pending = queue.lock_pending(MAX_TITLES_PER_BATCH).await?;
            let (groups, assignments) = self.plan(pending);
            if !assignments.is_empty() {
                queue.create_batch(key, &assignments).await?;
            }
            groups
        };
        commit(tx).await?;
        if groups.is_empty() {
            return Ok(());
        }
        match self.batch_translator.submit(&groups).await {
            Ok(provider_batch_id) => {
                self.record_submission(key, &provider_batch_id).await?;
                report.submitted_groups = groups.len();
                report.submitted_titles = groups.iter().map(|group| group.titles.len()).sum();
                Ok(())
            }
            Err(error) => {
                self.close_batch(key).await?;
                Err(Error::SubmissionFailed {
                    source: box_error(error),
                })
            }
        }
    }

    /// Groups grouped work by source language as the translator packs it; single work gets a
    /// request of its own.
    fn plan(
        &self,
        pending: Vec<PendingTitleTranslation>,
    ) -> (Vec<TitleTranslationGroup>, Vec<TitleTranslationAssignment>) {
        let mut grouped = IndexMap::<Language, Vec<PendingTitleTranslation>>::new();
        let mut planned = Vec::new();
        for item in pending {
            match item.mode {
                TitleTranslationMode::Grouped => {
                    grouped.entry(item.source_language).or_default().push(item);
                }
                TitleTranslationMode::Single | TitleTranslationMode::RefusalFallback => {
                    planned.push(vec![item]);
                }
            }
        }
        for (language, items) in grouped {
            let titles = items
                .iter()
                .map(|item| item.title.clone())
                .collect::<Vec<_>>();
            let sizes = self.batch_translator.group_sizes(
                language,
                &translation_targets(language),
                &titles,
            );
            let mut items = items.into_iter();
            for size in sizes {
                let group = items.by_ref().take(size).collect::<Vec<_>>();
                if !group.is_empty() {
                    planned.push(group);
                }
            }
            // Sizes that fall short of the titles leave the rest to requests of their own.
            planned.extend(items.map(|item| vec![item]));
        }
        let mut groups = Vec::with_capacity(planned.len());
        let mut assignments = Vec::new();
        for (group, items) in (1..).zip(planned) {
            let source_language = items[0].source_language;
            let mut titles = Vec::with_capacity(items.len());
            for (position, item) in (1..).zip(items) {
                assignments.push(TitleTranslationAssignment {
                    work: item.work,
                    group,
                    position,
                });
                titles.push(PositionedTitle {
                    position,
                    title: item.title,
                });
            }
            groups.push(TitleTranslationGroup {
                group,
                source_language,
                target_languages: translation_targets(source_language),
                titles,
            });
        }
        (groups, assignments)
    }

    async fn settle(
        &self,
        pending: &PendingTitleTranslation,
        claim: TitleTranslationClaim,
        settlement: Settlement,
        report: &mut TitleTranslationRoundReport,
    ) -> Result<(), Error> {
        let work = pending.work;
        let retry = match settlement {
            Settlement::Retry(retry) => retry,
            Settlement::Commit(titles) => match self.commit(pending, claim, titles).await {
                Ok(outcome) => {
                    report.count(outcome);
                    if outcome == ProductListingTranslationWriteOutcome::Applied {
                        tracing::info!(
                            event = "product.translated",
                            actor_type = "system",
                            product_listing_id = %work.product_listing_id,
                            source_event_id = %work.source_event_id,
                            outcome = "success",
                        );
                    }
                    return Ok(());
                }
                // A write this listing's state rejects fails only its title.
                Err(error @ Error::TranslationWriteFailed { .. }) => {
                    report.write_failures += 1;
                    tracing::warn!(
                        event = "product.translation.write_failed",
                        product_listing_id = %work.product_listing_id,
                        source_event_id = %work.source_event_id,
                        error_kind = error.kind(),
                        "Translated title could not be written; it spends an attempt"
                    );
                    failed(pending)
                }
                Err(error) => return Err(error),
            },
        };
        self.retry(work, claim, retry, report).await
    }

    async fn retry(
        &self,
        work: TitleTranslationWorkRef,
        claim: TitleTranslationClaim,
        retry: Retry,
        report: &mut TitleTranslationRoundReport,
    ) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        match retry {
            Retry::Requeue(mode, attempts) => {
                self.queue
                    .in_transaction(&mut tx)
                    .requeue(work, claim, mode, attempts)
                    .await?;
                commit(tx).await?;
                report.requeued += 1;
            }
            Retry::Park(attempts) => {
                self.queue
                    .in_transaction(&mut tx)
                    .park(work, claim, attempts)
                    .await?;
                commit(tx).await?;
                report.parked += 1;
                tracing::warn!(
                    event = "product.translation.parked",
                    product_listing_id = %work.product_listing_id,
                    source_event_id = %work.source_event_id,
                    attempts,
                    "Title translation parked until the listing's title changes"
                );
            }
        }
        Ok(())
    }

    /// Writes the translations under the source-marker guard and completes the work together.
    async fn commit(
        &self,
        pending: &PendingTitleTranslation,
        claim: TitleTranslationClaim,
        titles: IndexMap<Language, Title>,
    ) -> Result<ProductListingTranslationWriteOutcome, Error> {
        let work = pending.work;
        let mut tx = self.begin().await?;
        let outcome = self
            .translations
            .in_transaction(&mut tx)
            .apply(&ProductListingTranslationWrite {
                product_listing_id: work.product_listing_id,
                source_event_id: work.source_event_id,
                enrichment_event_id: EventId::new(),
                source_language: pending.source_language,
                titles,
            })
            .await?;
        self.queue
            .in_transaction(&mut tx)
            .complete(work, claim)
            .await?;
        commit(tx).await?;
        Ok(outcome)
    }

    async fn begin(&self) -> Result<U::Tx, Error> {
        self.unit_of_work
            .begin()
            .await
            .map_err(|source| Error::BeginTransactionFailed {
                source: box_error(source),
            })
    }

    async fn close_abandoned_submissions(&self) -> Result<u64, Error> {
        let mut tx = self.begin().await?;
        let closed = self
            .queue
            .in_transaction(&mut tx)
            .close_abandoned_submissions(ABANDONED_SUBMISSION_AGE)
            .await?;
        commit(tx).await?;
        Ok(closed)
    }

    async fn record_submission(
        &self,
        key: TitleTranslationBatchKey,
        provider_batch_id: &TitleTranslationBatchId,
    ) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        self.queue
            .in_transaction(&mut tx)
            .record_submission(key, provider_batch_id)
            .await?;
        commit(tx).await
    }

    async fn close_batch(&self, key: TitleTranslationBatchKey) -> Result<(), Error> {
        let mut tx = self.begin().await?;
        self.queue.in_transaction(&mut tx).close_batch(key).await?;
        commit(tx).await
    }
}

async fn commit<T: Transaction>(tx: T) -> Result<(), Error> {
    tx.commit()
        .await
        .map_err(|source| Error::CommitTransactionFailed {
            source: box_error(source),
        })
}

/// The batch's current titles in the groups and positions they were submitted with.
fn groups_of(items: &[BatchedTitleTranslation]) -> Vec<TitleTranslationGroup> {
    let mut groups = BTreeMap::<u32, TitleTranslationGroup>::new();
    for item in items {
        let language = item.translation.source_language;
        groups
            .entry(item.group)
            .or_insert_with(|| TitleTranslationGroup {
                group: item.group,
                source_language: language,
                target_languages: translation_targets(language),
                titles: Vec::new(),
            })
            .titles
            .push(PositionedTitle {
                position: item.position,
                title: item.translation.title.clone(),
            });
    }
    groups.into_values().collect()
}

/// Decides each current title's settlement from its group's outcome. A group the provider did
/// not return was not processed.
fn settlements(
    items: Vec<BatchedTitleTranslation>,
    mut outcomes: BTreeMap<u32, TitleTranslationGroupOutcome>,
) -> Vec<(PendingTitleTranslation, Settlement)> {
    let mut by_group = BTreeMap::<u32, Vec<BatchedTitleTranslation>>::new();
    for item in items {
        by_group.entry(item.group).or_default().push(item);
    }
    let mut settled = Vec::new();
    for (group, items) in by_group {
        match outcomes
            .remove(&group)
            .unwrap_or(TitleTranslationGroupOutcome::NotProcessed)
        {
            TitleTranslationGroupOutcome::Translated(results) => {
                let mut results = results.into_iter().collect::<BTreeMap<_, _>>();
                for item in items {
                    let settlement = match results.remove(&item.position) {
                        Some(Ok(titles)) => Settlement::Commit(titles),
                        Some(Err(_)) | None => Settlement::Retry(failed(&item.translation)),
                    };
                    settled.push((item.translation, settlement));
                }
            }
            TitleTranslationGroupOutcome::Refused => {
                settled.extend(items.into_iter().map(|item| {
                    let pending = item.translation;
                    let next = if pending.mode != TitleTranslationMode::Grouped {
                        TitleTranslationMode::RefusalFallback
                    } else {
                        TitleTranslationMode::Single
                    };
                    let attempts = pending.attempts;
                    (pending, Settlement::Retry(Retry::Requeue(next, attempts)))
                }));
            }
            TitleTranslationGroupOutcome::Truncated => {
                settled.extend(items.into_iter().map(|item| {
                    let pending = item.translation;
                    let settlement = if pending.mode != TitleTranslationMode::Grouped {
                        Settlement::Retry(failed(&pending))
                    } else {
                        Settlement::Retry(Retry::Requeue(
                            TitleTranslationMode::Single,
                            pending.attempts,
                        ))
                    };
                    (pending, settlement)
                }));
            }
            TitleTranslationGroupOutcome::Failed(error) => {
                settled.extend(items.into_iter().map(|item| {
                    let settlement = Settlement::Retry(after_error(&item.translation, &error));
                    (item.translation, settlement)
                }));
            }
            TitleTranslationGroupOutcome::NotProcessed => {
                settled.extend(items.into_iter().map(|item| {
                    let pending = item.translation;
                    let settlement =
                        Settlement::Retry(Retry::Requeue(pending.mode, pending.attempts));
                    (pending, settlement)
                }));
            }
        }
    }
    settled
}

/// A refusal on the fallback model repeats, so the title fails closed at once.
fn fallback_settlement(
    pending: &PendingTitleTranslation,
    result: Result<IndexMap<Language, Title>, ProductListingTitleTranslationError>,
) -> Settlement {
    match result {
        Ok(titles) => Settlement::Commit(titles),
        Err(ProductListingTitleTranslationError::Refused { .. }) => {
            Settlement::Retry(Retry::Park(pending.attempts.saturating_add(1)))
        }
        Err(error) => Settlement::Retry(after_error(pending, &error)),
    }
}

/// A provider that is temporarily unavailable did not judge the title, so it returns to the
/// queue unchanged; any other failure spends an attempt.
fn after_error(
    pending: &PendingTitleTranslation,
    error: &ProductListingTitleTranslationError,
) -> Retry {
    match error {
        ProductListingTitleTranslationError::TemporarilyUnavailable { .. } => {
            Retry::Requeue(pending.mode, pending.attempts)
        }
        ProductListingTitleTranslationError::InvalidResponse { .. }
        | ProductListingTitleTranslationError::Refused { .. } => failed(pending),
    }
}

/// One more failed attempt, in the work's current mode, or parked once the attempts are spent.
fn failed(pending: &PendingTitleTranslation) -> Retry {
    let attempts = pending.attempts.saturating_add(1);
    if attempts >= MAX_TRANSLATION_ATTEMPTS {
        Retry::Park(attempts)
    } else {
        Retry::Requeue(pending.mode, attempts)
    }
}

fn rejected_batch(
    error: &ProductListingTitleTranslationError,
) -> ProductListingTitleTranslationError {
    ProductListingTitleTranslationError::InvalidResponse {
        source: box_error(std::io::Error::other(error.to_string())),
    }
}

impl TitleTranslationRoundReport {
    fn count(&mut self, outcome: ProductListingTranslationWriteOutcome) {
        match outcome {
            ProductListingTranslationWriteOutcome::Applied => self.applied += 1,
            ProductListingTranslationWriteOutcome::Duplicate => self.duplicate += 1,
            ProductListingTranslationWriteOutcome::Stale
            | ProductListingTranslationWriteOutcome::ProductListingNotFound => self.stale += 1,
        }
    }
}

impl TranslateProductListingTitleBatchesError {
    /// An allowlisted label of the failed step, safe to log.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SystemPrincipalRequired => "system_principal_required",
            Self::QueueFailed { .. } => "queue",
            Self::BeginTransactionFailed { .. } => "write_begin",
            Self::TranslationWriteFailed { .. } => "translation_write",
            Self::CommitTransactionFailed { .. } => "write_commit",
            Self::SubmissionFailed { .. } => "batch_submit",
        }
    }
}

impl From<OperationAuthorizationError> for TranslateProductListingTitleBatchesError {
    fn from(_error: OperationAuthorizationError) -> Self {
        Self::SystemPrincipalRequired
    }
}

impl From<ProductListingTitleTranslationQueueError> for TranslateProductListingTitleBatchesError {
    fn from(source: ProductListingTitleTranslationQueueError) -> Self {
        Self::QueueFailed {
            source: box_error(source),
        }
    }
}

impl From<ProductListingTranslationWriteError> for TranslateProductListingTitleBatchesError {
    fn from(source: ProductListingTranslationWriteError) -> Self {
        Self::TranslationWriteFailed {
            source: box_error(source),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        TitleTranslationGroupResult, TitleTranslationScheduleOutcome, TitleTranslationWorkRef,
    };
    use application::operation_context::{CorrelationId, Principal, RequestId};
    use application::transaction::TransactionError;
    use product_listing_core::product_listing_id::ProductListingId;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    fn pending(
        language: Language,
        mode: TitleTranslationMode,
        attempts: u16,
    ) -> PendingTitleTranslation {
        PendingTitleTranslation {
            work: TitleTranslationWorkRef {
                product_listing_id: ProductListingId::new(),
                source_event_id: EventId::new(),
            },
            title: Title::from("Antiker Eichenstuhl"),
            source_language: language,
            mode,
            attempts,
        }
    }

    fn batched(
        group: u32,
        position: u32,
        translation: PendingTitleTranslation,
    ) -> BatchedTitleTranslation {
        BatchedTitleTranslation {
            group,
            position,
            translation,
        }
    }

    fn translated() -> IndexMap<Language, Title> {
        IndexMap::from([(Language::En, Title::from("Antique oak chair"))])
    }

    fn invalid() -> ProductListingTitleTranslationError {
        ProductListingTitleTranslationError::InvalidResponse {
            source: application::error::static_error("invalid"),
        }
    }

    fn settle_one_group(
        items: Vec<BatchedTitleTranslation>,
        outcome: Option<TitleTranslationGroupOutcome>,
    ) -> Vec<Settlement> {
        settlements(
            items,
            outcome.into_iter().map(|outcome| (1, outcome)).collect(),
        )
        .into_iter()
        .map(|(_, settlement)| settlement)
        .collect()
    }

    #[test]
    fn should_fail_only_the_invalid_title_of_a_translated_group() {
        let items = vec![
            batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            ),
            batched(
                1,
                2,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            ),
            batched(
                1,
                3,
                pending(Language::De, TitleTranslationMode::Grouped, 2),
            ),
            batched(
                1,
                4,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            ),
        ];
        let outcome = TitleTranslationGroupOutcome::Translated(vec![
            (1, Ok(translated())),
            (2, Err(invalid())),
            (3, Err(invalid())),
        ]);

        assert_eq!(
            vec![
                Settlement::Commit(translated()),
                Settlement::Retry(Retry::Requeue(TitleTranslationMode::Grouped, 1)),
                Settlement::Retry(Retry::Park(MAX_TRANSLATION_ATTEMPTS)),
                Settlement::Retry(Retry::Requeue(TitleTranslationMode::Grouped, 1)),
            ],
            settle_one_group(items, Some(outcome))
        );
    }

    #[test]
    fn should_split_a_refused_group_and_send_a_title_refused_alone_to_the_fallback() {
        let group = || {
            vec![
                batched(
                    1,
                    1,
                    pending(Language::De, TitleTranslationMode::Grouped, 1),
                ),
                batched(
                    1,
                    2,
                    pending(Language::De, TitleTranslationMode::Grouped, 0),
                ),
            ]
        };
        assert_eq!(
            vec![
                Settlement::Retry(Retry::Requeue(TitleTranslationMode::Single, 1)),
                Settlement::Retry(Retry::Requeue(TitleTranslationMode::Single, 0)),
            ],
            settle_one_group(group(), Some(TitleTranslationGroupOutcome::Refused))
        );
        assert_eq!(
            vec![
                Settlement::Retry(Retry::Requeue(TitleTranslationMode::Single, 1)),
                Settlement::Retry(Retry::Requeue(TitleTranslationMode::Single, 0)),
            ],
            settle_one_group(group(), Some(TitleTranslationGroupOutcome::Truncated))
        );

        // A grouped title left alone because its siblings were superseded was still refused
        // among them, so it is retried alone before it reaches the fallback.
        let left_alone = || {
            vec![batched(
                1,
                2,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            )]
        };
        assert_eq!(
            vec![Settlement::Retry(Retry::Requeue(
                TitleTranslationMode::Single,
                0
            ))],
            settle_one_group(left_alone(), Some(TitleTranslationGroupOutcome::Refused))
        );
        let alone = || {
            vec![batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Single, 0),
            )]
        };
        assert_eq!(
            vec![Settlement::Retry(Retry::Requeue(
                TitleTranslationMode::RefusalFallback,
                0
            ))],
            settle_one_group(alone(), Some(TitleTranslationGroupOutcome::Refused))
        );
        assert_eq!(
            vec![Settlement::Retry(Retry::Requeue(
                TitleTranslationMode::Single,
                1
            ))],
            settle_one_group(alone(), Some(TitleTranslationGroupOutcome::Truncated))
        );
    }

    #[test]
    fn should_retry_titles_a_temporarily_unavailable_provider_did_not_judge_for_free() {
        let unavailable = || ProductListingTitleTranslationError::TemporarilyUnavailable {
            source: application::error::static_error("overloaded"),
        };
        let item = || {
            vec![batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Grouped, 2),
            )]
        };

        assert_eq!(
            vec![Settlement::Retry(Retry::Requeue(
                TitleTranslationMode::Grouped,
                2
            ))],
            settle_one_group(
                item(),
                Some(TitleTranslationGroupOutcome::Failed(unavailable()))
            )
        );
        let fallback = pending(Language::De, TitleTranslationMode::RefusalFallback, 2);
        assert_eq!(
            Settlement::Retry(Retry::Requeue(TitleTranslationMode::RefusalFallback, 2)),
            fallback_settlement(&fallback, Err(unavailable()))
        );
    }

    #[test]
    fn should_retry_unprocessed_requests_without_spending_an_attempt() {
        let item = || {
            vec![batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Single, 2),
            )]
        };

        for outcome in [None, Some(TitleTranslationGroupOutcome::NotProcessed)] {
            assert_eq!(
                vec![Settlement::Retry(Retry::Requeue(
                    TitleTranslationMode::Single,
                    2
                ))],
                settle_one_group(item(), outcome)
            );
        }
        assert_eq!(
            vec![Settlement::Retry(Retry::Park(MAX_TRANSLATION_ATTEMPTS))],
            settle_one_group(
                item(),
                Some(TitleTranslationGroupOutcome::Failed(invalid()))
            )
        );
    }

    #[test]
    fn should_park_a_title_the_fallback_refuses_and_retry_its_other_failures() {
        let item = pending(Language::De, TitleTranslationMode::RefusalFallback, 0);
        let refused = ProductListingTitleTranslationError::Refused {
            source: application::error::static_error("refused"),
        };

        assert_eq!(
            Settlement::Commit(translated()),
            fallback_settlement(&item, Ok(translated()))
        );
        assert_eq!(
            Settlement::Retry(Retry::Park(1)),
            fallback_settlement(&item, Err(refused))
        );
        assert_eq!(
            Settlement::Retry(Retry::Requeue(TitleTranslationMode::RefusalFallback, 1)),
            fallback_settlement(&item, Err(invalid()))
        );
    }

    #[derive(Default)]
    struct State {
        submitted: Vec<SubmittedTitleTranslationBatch>,
        batch_items: Vec<BatchedTitleTranslation>,
        poll: VecDeque<Result<TitleTranslationBatchPoll, ProductListingTitleTranslationError>>,
        fallbacks: Vec<PendingTitleTranslation>,
        fallback_results:
            VecDeque<Result<IndexMap<Language, Title>, ProductListingTitleTranslationError>>,
        pending: Vec<PendingTitleTranslation>,
        submit_fails: bool,
        batch_items_fail_once: bool,
        write_fails: bool,
        write_outcome: Option<ProductListingTranslationWriteOutcome>,
        submitted_groups: Vec<TitleTranslationGroup>,
        writes: Vec<ProductListingTranslationWrite>,
        log: Vec<String>,
    }

    type Shared = Arc<Mutex<State>>;

    fn lock(state: &Shared) -> std::sync::MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[derive(Clone)]
    struct Fake(Shared);
    struct Tx(Shared);
    struct Queue(Shared);
    struct Writer(Shared);

    type QueueResult<T> = Result<T, ProductListingTitleTranslationQueueError>;

    #[async_trait::async_trait]
    impl ProductListingTitleTranslationQueueReader for Fake {
        async fn submitted_batches(&self) -> QueueResult<Vec<SubmittedTitleTranslationBatch>> {
            Ok(lock(&self.0).submitted.clone())
        }

        async fn batch_items(
            &self,
            _: TitleTranslationBatchKey,
        ) -> QueueResult<Vec<BatchedTitleTranslation>> {
            let mut state = lock(&self.0);
            if std::mem::take(&mut state.batch_items_fail_once) {
                return Err(
                    ProductListingTitleTranslationQueueError::InvalidPersistedState {
                        source: application::error::static_error("invalid row"),
                    },
                );
            }
            Ok(state.batch_items.clone())
        }

        async fn refusal_fallbacks(&self, _: u32) -> QueueResult<Vec<PendingTitleTranslation>> {
            Ok(std::mem::take(&mut lock(&self.0).fallbacks))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingTitleBatchTranslator for Fake {
        fn group_sizes(&self, _: Language, _: &[Language], titles: &[Title]) -> Vec<usize> {
            titles.chunks(2).map(<[Title]>::len).collect()
        }

        async fn submit(
            &self,
            groups: &[TitleTranslationGroup],
        ) -> Result<TitleTranslationBatchId, ProductListingTitleTranslationError> {
            let mut state = lock(&self.0);
            state.submitted_groups = groups.to_vec();
            if state.submit_fails {
                return Err(
                    ProductListingTitleTranslationError::TemporarilyUnavailable {
                        source: application::error::static_error("down"),
                    },
                );
            }
            Ok(TitleTranslationBatchId::new("msgbatch_new").unwrap())
        }

        async fn poll(
            &self,
            _: &TitleTranslationBatchId,
            _: &[TitleTranslationGroup],
        ) -> Result<TitleTranslationBatchPoll, ProductListingTitleTranslationError> {
            lock(&self.0).poll.pop_front().expect("scripted poll")
        }
    }

    #[async_trait::async_trait]
    impl ProductListingTitleTranslator for Fake {
        async fn translate(
            &self,
            _: &Title,
            _: Language,
            _: &[Language],
        ) -> Result<IndexMap<Language, Title>, ProductListingTitleTranslationError> {
            lock(&self.0)
                .fallback_results
                .pop_front()
                .expect("scripted fallback")
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for Fake {
        type Tx = Tx;

        async fn begin(&self) -> Result<Tx, TransactionError> {
            Ok(Tx(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for Tx {
        async fn commit(self) -> Result<(), TransactionError> {
            lock(&self.0).log.push("commit".to_owned());
            Ok(())
        }
    }

    impl ProductListingTitleTranslationQueueFactory<Tx> for Fake {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut Tx,
        ) -> impl ProductListingTitleTranslationQueue + 'tx {
            Queue(Arc::clone(&tx.0))
        }
    }

    impl ProductListingTranslationWriterFactory<Tx> for Fake {
        fn in_transaction<'tx>(
            &'tx self,
            tx: &'tx mut Tx,
        ) -> impl ProductListingTranslationWriter + 'tx {
            Writer(Arc::clone(&tx.0))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingTranslationWriter for Writer {
        async fn apply(
            &mut self,
            write: &ProductListingTranslationWrite,
        ) -> Result<ProductListingTranslationWriteOutcome, ProductListingTranslationWriteError>
        {
            let mut state = lock(&self.0);
            if state.write_fails {
                return Err(ProductListingTranslationWriteError::WriteFailed {
                    source: application::error::static_error("orphan translation rows"),
                });
            }
            state.writes.push(write.clone());
            Ok(state
                .write_outcome
                .unwrap_or(ProductListingTranslationWriteOutcome::Applied))
        }
    }

    #[async_trait::async_trait]
    impl ProductListingTitleTranslationQueue for Queue {
        async fn schedule(
            &mut self,
            _: TitleTranslationWorkRef,
        ) -> QueueResult<TitleTranslationScheduleOutcome> {
            unreachable!()
        }

        async fn discard_superseded(&mut self) -> QueueResult<u64> {
            Ok(0)
        }

        async fn lock_pending(&mut self, _: u32) -> QueueResult<Vec<PendingTitleTranslation>> {
            Ok(std::mem::take(&mut lock(&self.0).pending))
        }

        async fn create_batch(
            &mut self,
            _: TitleTranslationBatchKey,
            assignments: &[TitleTranslationAssignment],
        ) -> QueueResult<()> {
            lock(&self.0)
                .log
                .push(format!("create_batch {}", assignments.len()));
            Ok(())
        }

        async fn record_submission(
            &mut self,
            _: TitleTranslationBatchKey,
            id: &TitleTranslationBatchId,
        ) -> QueueResult<()> {
            lock(&self.0).log.push(format!("record_submission {id}"));
            Ok(())
        }

        async fn close_batch(&mut self, _: TitleTranslationBatchKey) -> QueueResult<()> {
            lock(&self.0).log.push("close_batch".to_owned());
            Ok(())
        }

        async fn close_abandoned_submissions(&mut self, _: Duration) -> QueueResult<u64> {
            Ok(0)
        }

        async fn complete(
            &mut self,
            _: TitleTranslationWorkRef,
            _: TitleTranslationClaim,
        ) -> QueueResult<()> {
            lock(&self.0).log.push("complete".to_owned());
            Ok(())
        }

        async fn requeue(
            &mut self,
            _: TitleTranslationWorkRef,
            _: TitleTranslationClaim,
            mode: TitleTranslationMode,
            attempts: u16,
        ) -> QueueResult<()> {
            lock(&self.0)
                .log
                .push(format!("requeue {mode:?} {attempts}"));
            Ok(())
        }

        async fn park(
            &mut self,
            _: TitleTranslationWorkRef,
            _: TitleTranslationClaim,
            attempts: u16,
        ) -> QueueResult<()> {
            lock(&self.0).log.push(format!("park {attempts}"));
            Ok(())
        }
    }

    async fn run_round(state: &Shared) -> Result<TitleTranslationRoundReport, Error> {
        let fake = Fake(Arc::clone(state));
        TranslateProductListingTitleBatchesHandler::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
        )
        .execute(&OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        })
        .await
    }

    fn submitted_batch() -> SubmittedTitleTranslationBatch {
        SubmittedTitleTranslationBatch {
            key: TitleTranslationBatchKey::new(),
            provider_batch_id: TitleTranslationBatchId::new("msgbatch_old").unwrap(),
        }
    }

    #[tokio::test]
    async fn should_commit_an_ended_batch_under_the_guard_and_close_it() {
        let state = Shared::default();
        let item = pending(Language::De, TitleTranslationMode::Grouped, 0);
        {
            let mut state = lock(&state);
            state.submitted = vec![submitted_batch()];
            state.batch_items = vec![batched(1, 1, item.clone())];
            state
                .poll
                .push_back(Ok(TitleTranslationBatchPoll::Ended(vec![
                    TitleTranslationGroupResult {
                        group: 1,
                        outcome: TitleTranslationGroupOutcome::Translated(vec![(
                            1,
                            Ok(translated()),
                        )]),
                    },
                ])));
        }

        let report = run_round(&state).await.unwrap();

        assert_eq!((1, 1), (report.batches_collected, report.applied));
        let state = lock(&state);
        assert_eq!(1, state.writes.len());
        assert_eq!(item.work.source_event_id, state.writes[0].source_event_id);
        assert_eq!(Language::De, state.writes[0].source_language);
        assert_eq!(
            vec![
                "commit",
                "complete",
                "commit",
                "close_batch",
                "commit",
                "commit"
            ],
            state.log
        );
    }

    #[tokio::test]
    async fn should_drop_a_stale_result_and_complete_its_work() {
        let state = Shared::default();
        {
            let mut state = lock(&state);
            state.submitted = vec![submitted_batch()];
            state.batch_items = vec![batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            )];
            state.write_outcome = Some(ProductListingTranslationWriteOutcome::Stale);
            state
                .poll
                .push_back(Ok(TitleTranslationBatchPoll::Ended(vec![
                    TitleTranslationGroupResult {
                        group: 1,
                        outcome: TitleTranslationGroupOutcome::Translated(vec![(
                            1,
                            Ok(translated()),
                        )]),
                    },
                ])));
        }

        let report = run_round(&state).await.unwrap();

        assert_eq!((0, 1), (report.applied, report.stale));
        assert!(lock(&state).log.contains(&"complete".to_owned()));
    }

    fn ended_with_one_translation() -> TitleTranslationBatchPoll {
        TitleTranslationBatchPoll::Ended(vec![TitleTranslationGroupResult {
            group: 1,
            outcome: TitleTranslationGroupOutcome::Translated(vec![(1, Ok(translated()))]),
        }])
    }

    #[tokio::test]
    async fn should_fail_only_the_title_whose_write_fails_and_finish_the_round() {
        let state = Shared::default();
        {
            let mut state = lock(&state);
            state.submitted = vec![submitted_batch()];
            state.batch_items = vec![batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            )];
            state.write_fails = true;
            state.poll.push_back(Ok(ended_with_one_translation()));
            state.pending = vec![pending(Language::Fr, TitleTranslationMode::Grouped, 0)];
        }

        let report = run_round(&state).await.unwrap();

        assert_eq!(
            (1, 1, 1, 1),
            (
                report.write_failures,
                report.requeued,
                report.batches_collected,
                report.submitted_titles
            )
        );
        let log = &lock(&state).log;
        assert!(log.contains(&"requeue Grouped 1".to_owned()));
        assert!(!log.contains(&"complete".to_owned()));
    }

    #[tokio::test]
    async fn should_collect_other_batches_when_one_cannot_be_read() {
        let state = Shared::default();
        {
            let mut state = lock(&state);
            state.submitted = vec![submitted_batch(), submitted_batch()];
            state.batch_items_fail_once = true;
            state.batch_items = vec![batched(
                1,
                1,
                pending(Language::De, TitleTranslationMode::Grouped, 0),
            )];
            state.poll.push_back(Ok(ended_with_one_translation()));
        }

        let report = run_round(&state).await.unwrap();

        assert_eq!(
            (1, 1, 1),
            (
                report.batches_failed,
                report.batches_collected,
                report.applied
            )
        );
    }

    #[tokio::test]
    async fn should_leave_a_running_or_unreachable_batch_for_the_next_round() {
        let state = Shared::default();
        {
            let mut state = lock(&state);
            state.submitted = vec![submitted_batch(), submitted_batch()];
            state
                .poll
                .push_back(Ok(TitleTranslationBatchPoll::InProgress));
            state.poll.push_back(Err(
                ProductListingTitleTranslationError::TemporarilyUnavailable {
                    source: application::error::static_error("down"),
                },
            ));
        }

        let report = run_round(&state).await.unwrap();

        assert_eq!(
            (1, 1, 0),
            (
                report.batches_in_progress,
                report.batches_unavailable,
                report.batches_collected
            )
        );
        assert!(!lock(&state).log.contains(&"close_batch".to_owned()));
    }

    #[tokio::test]
    async fn should_translate_a_refused_title_on_the_fallback() {
        let state = Shared::default();
        {
            let mut state = lock(&state);
            state.fallbacks = vec![pending(
                Language::En,
                TitleTranslationMode::RefusalFallback,
                0,
            )];
            state.fallback_results.push_back(Ok(translated()));
        }

        let report = run_round(&state).await.unwrap();

        assert_eq!((1, 1), (report.refusal_fallbacks, report.applied));
        assert_eq!(Language::En, lock(&state).writes[0].source_language);
    }

    #[tokio::test]
    async fn should_submit_pending_titles_grouped_by_source_language() {
        let state = Shared::default();
        lock(&state).pending = vec![
            pending(Language::De, TitleTranslationMode::Grouped, 0),
            pending(Language::Fr, TitleTranslationMode::Grouped, 0),
            pending(Language::De, TitleTranslationMode::Grouped, 0),
            pending(Language::De, TitleTranslationMode::Grouped, 0),
            pending(Language::De, TitleTranslationMode::Single, 1),
        ];

        let report = run_round(&state).await.unwrap();

        assert_eq!((4, 5), (report.submitted_groups, report.submitted_titles));
        let state = lock(&state);
        let shape = state
            .submitted_groups
            .iter()
            .map(|group| {
                (
                    group.group,
                    group.source_language,
                    group
                        .titles
                        .iter()
                        .map(|title| title.position)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            vec![
                (1, Language::De, vec![1]),
                (2, Language::De, vec![1, 2]),
                (3, Language::De, vec![1]),
                (4, Language::Fr, vec![1]),
            ],
            shape
        );
        assert!(
            !state.submitted_groups[1]
                .target_languages
                .contains(&Language::De)
        );
        assert_eq!(
            vec![
                "commit",
                "create_batch 5",
                "commit",
                "record_submission msgbatch_new",
                "commit"
            ],
            state.log
        );
    }

    #[tokio::test]
    async fn should_return_work_to_the_queue_when_submission_fails() {
        let state = Shared::default();
        {
            let mut state = lock(&state);
            state.pending = vec![pending(Language::De, TitleTranslationMode::Grouped, 0)];
            state.submit_fails = true;
        }

        let error = run_round(&state).await.unwrap_err();

        assert!(matches!(error, Error::SubmissionFailed { .. }));
        assert_eq!(
            vec![
                "commit",
                "create_batch 1",
                "commit",
                "close_batch",
                "commit"
            ],
            lock(&state).log
        );
    }

    #[tokio::test]
    async fn should_require_the_system_principal() {
        let fake = Fake(Shared::default());
        let result = TranslateProductListingTitleBatchesHandler::new(
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake.clone(),
            fake,
        )
        .execute(&OperationContext {
            principal: Principal::User(user_core::user_id::UserId::new()),
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        })
        .await;

        assert!(matches!(result, Err(Error::SystemPrincipalRequired)));
    }
}
