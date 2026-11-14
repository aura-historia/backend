use crate::{
    ports::{
        ContentNoticeClock, ContentNoticeRepository, ContentNoticeRepositoryError,
        ContentNoticeRepositoryFactory, ContentNoticeSubmissionFingerprint,
        ContentNoticeSubmissionHistory, ContentNoticeSubmissionHistoryError,
        ContentNoticeSubmissionHistoryFactory, ContentNoticeTargetReadError,
        ContentNoticeTargetReader, ContentNoticeTargetReaderFactory,
    },
    submission_fingerprint::submission_fingerprint,
    submission_policy::{RetryAfter, SubmissionDecision, decide, window_start},
};
use application::{
    error::{BoxError, box_error},
    operation_context::OperationContext,
    transaction::{Transaction, UnitOfWork},
};
use content_notice_core::{
    additional_url::AdditionalUrls,
    content_notice::{ContentNotice, ContentNoticeInvariantError, NewContentNotice},
    content_notice_id::ContentNoticeId,
    content_notice_target::{ContentNoticeTarget, TargetSnapshot},
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail},
    submitter_role::SubmitterRole,
};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};

/// The target a submitter names. A product listing's listing source is resolved by the use
/// case, never taken from the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentNoticeTargetReference {
    ProductListing {
        product_listing_id: ProductListingId,
    },
    ListingSource {
        listing_source_id: ListingSourceId,
    },
}

impl ContentNoticeTargetReference {
    /// The same code as the resolved [`ContentNoticeTarget`] kind.
    pub fn type_code(self) -> &'static str {
        match self {
            Self::ProductListing { .. } => "PRODUCT_LISTING",
            Self::ListingSource { .. } => "LISTING_SOURCE",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubmitContentNoticeCommand {
    pub target: ContentNoticeTargetReference,
    pub submitter_role: SubmitterRole,
    pub reason_category: ReasonCategory,
    pub explanation: Option<NoticeExplanation>,
    pub additional_urls: AdditionalUrls,
    pub submitter: Submitter,
    pub good_faith_statement_version: GoodFaithStatementVersion,
}

/// Both outcomes are answered alike, so a resubmission is indistinguishable from a new notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitContentNoticeResult {
    Stored {
        content_notice_id: ContentNoticeId,
    },
    /// The same submission from the same address is already stored within the submission
    /// window. Nothing was stored.
    Duplicate,
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitContentNoticeError {
    #[error("content notice breaks a submission rule")]
    InvalidNotice(#[source] ContentNoticeInvariantError),
    #[error("content notice target not found")]
    TargetNotFound,
    /// The submitter's address has used its notice budget for the submission window.
    #[error("too many content notices from one address")]
    RateLimited { retry_after: RetryAfter },
    #[error("temporary content notice failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted content notice target or state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal content notice failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("failed to begin transaction")]
    BeginTransactionFailed(#[source] BoxError),
    #[error("failed to commit transaction")]
    CommitTransactionFailed(#[source] BoxError),
}

/// Records a content notice from anyone, with or without an account, within the
/// [submission limits](crate::submission_policy). The caller's identity is used for logging
/// only and is never stored with the notice.
#[async_trait::async_trait]
pub trait SubmitContentNoticeUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitContentNoticeCommand,
    ) -> Result<SubmitContentNoticeResult, SubmitContentNoticeError>;
}

pub struct SubmitContentNoticeHandler<U, T, R, H, C> {
    unit_of_work: U,
    targets: T,
    notices: R,
    history: H,
    clock: C,
}

impl<U, T, R, H, C> SubmitContentNoticeHandler<U, T, R, H, C> {
    pub fn new(unit_of_work: U, targets: T, notices: R, history: H, clock: C) -> Self {
        Self {
            unit_of_work,
            targets,
            notices,
            history,
            clock,
        }
    }
}

#[async_trait::async_trait]
impl<U, T, R, H, C> SubmitContentNoticeUseCase for SubmitContentNoticeHandler<U, T, R, H, C>
where
    U: UnitOfWork,
    T: ContentNoticeTargetReaderFactory<U::Tx>,
    R: ContentNoticeRepositoryFactory<U::Tx>,
    H: ContentNoticeSubmissionHistoryFactory<U::Tx>,
    C: ContentNoticeClock,
{
    // The target ID stays out of the span: a rate-limited request must not log it, because it
    // can indicate who complained about what.
    #[tracing::instrument(
        name = "submit_content_notice",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            target_type = command.target.type_code(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitContentNoticeCommand,
    ) -> Result<SubmitContentNoticeResult, SubmitContentNoticeError> {
        if let Some(actor_id) = context.principal.actor_id() {
            tracing::Span::current().record("actor_id", tracing::field::display(actor_id));
        }
        ContentNotice::check_reason(
            command.submitter_role,
            command.reason_category,
            command.explanation.as_ref(),
        )
        .map_err(SubmitContentNoticeError::InvalidNotice)?;

        let mut tx =
            self.unit_of_work.begin().await.map_err(|source| {
                SubmitContentNoticeError::BeginTransactionFailed(Box::new(source))
            })?;
        let (target, target_snapshot) =
            resolve_target(&mut self.targets.in_transaction(&mut tx), command.target).await?;
        let notice = ContentNotice::submit(NewContentNotice {
            id: ContentNoticeId::new(),
            target,
            target_snapshot,
            submitter_role: command.submitter_role,
            reason_category: command.reason_category,
            explanation: command.explanation,
            additional_urls: command.additional_urls,
            submitter: command.submitter,
            good_faith_statement_version: command.good_faith_statement_version,
        })
        .map_err(|source| SubmitContentNoticeError::Internal {
            source: box_error(source),
        })?;
        let fingerprint = submission_fingerprint(&notice);
        let decision = self
            .check_submission_limits(&mut tx, &notice.submitter().email, fingerprint)
            .await?;
        let result = match decision {
            SubmissionDecision::RateLimited { retry_after } => {
                log_rate_limited(context, retry_after);
                return Err(SubmitContentNoticeError::RateLimited { retry_after });
            }
            SubmissionDecision::Duplicate => SubmitContentNoticeResult::Duplicate,
            SubmissionDecision::Store => {
                self.record(&mut tx, &notice, fingerprint).await?;
                SubmitContentNoticeResult::Stored {
                    content_notice_id: notice.id(),
                }
            }
        };
        tx.commit().await.map_err(|source| {
            SubmitContentNoticeError::CommitTransactionFailed(Box::new(source))
        })?;

        log_accepted(context, &notice, result);
        Ok(result)
    }
}

impl<U, T, R, H, C> SubmitContentNoticeHandler<U, T, R, H, C>
where
    U: UnitOfWork,
    R: ContentNoticeRepositoryFactory<U::Tx>,
    H: ContentNoticeSubmissionHistoryFactory<U::Tx>,
    C: ContentNoticeClock,
{
    /// Serializes the address's submissions for the rest of the transaction, then decides the
    /// submission from the address's notices within the window.
    async fn check_submission_limits(
        &self,
        tx: &mut U::Tx,
        email: &SubmitterEmail,
        fingerprint: ContentNoticeSubmissionFingerprint,
    ) -> Result<SubmissionDecision, SubmitContentNoticeError> {
        let mut history = self.history.in_transaction(tx);
        history.lock_address(email).await?;
        let now = self.clock.now_utc();
        let submissions = history.submissions_after(email, window_start(now)).await?;
        Ok(decide(now, fingerprint, &submissions))
    }

    /// Stores the notice in the submission transaction. Everything that must commit or roll
    /// back together with the notice belongs here.
    async fn record(
        &self,
        tx: &mut U::Tx,
        notice: &ContentNotice,
        fingerprint: ContentNoticeSubmissionFingerprint,
    ) -> Result<(), SubmitContentNoticeError> {
        self.notices
            .in_transaction(tx)
            .insert(notice, fingerprint)
            .await?;
        Ok(())
    }
}

/// Logs a committed submission, stored or duplicate, without personal data.
fn log_accepted(
    context: &OperationContext,
    notice: &ContentNotice,
    result: SubmitContentNoticeResult,
) {
    let target = notice.target();
    match result {
        SubmitContentNoticeResult::Stored { content_notice_id } => tracing::info!(
            event = "content_notice.submitted",
            content_notice_id = %content_notice_id,
            target_type = target.type_code(),
            listing_source_id = %target.listing_source_id(),
            product_listing_id = target.product_listing_id().map(tracing::field::display),
            submitter_role = notice.submitter_role().as_str(),
            reason_category = notice.reason_category().as_str(),
            actor_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            outcome = "success",
        ),
        SubmitContentNoticeResult::Duplicate => tracing::info!(
            event = "content_notice.duplicate",
            target_type = target.type_code(),
            listing_source_id = %target.listing_source_id(),
            product_listing_id = target.product_listing_id().map(tracing::field::display),
            actor_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            outcome = "duplicate",
        ),
    }
}

/// Logs a rejected submission without personal data and without the target, which can
/// indicate who complained about what.
fn log_rate_limited(context: &OperationContext, retry_after: RetryAfter) {
    tracing::warn!(
        event = "content_notice.rate_limited",
        retry_after_seconds = retry_after.seconds(),
        actor_type = context.principal.kind(),
        request_id = %context.request_id,
        correlation_id = %context.correlation_id,
        outcome = "rate_limited",
    );
}

/// Resolves the named target and snapshots it. A withdrawn listing is not found, as in the
/// public listing detail.
async fn resolve_target(
    reader: &mut impl ContentNoticeTargetReader,
    reference: ContentNoticeTargetReference,
) -> Result<(ContentNoticeTarget, TargetSnapshot), SubmitContentNoticeError> {
    match reference {
        ContentNoticeTargetReference::ListingSource { listing_source_id } => {
            let source = reader
                .find_listing_source(listing_source_id)
                .await?
                .ok_or(SubmitContentNoticeError::TargetNotFound)?;
            Ok((
                ContentNoticeTarget::ListingSource {
                    listing_source_id: source.listing_source_id,
                },
                TargetSnapshot {
                    listing_source: source.snapshot,
                    product_listing: None,
                },
            ))
        }
        ContentNoticeTargetReference::ProductListing { product_listing_id } => {
            let listing = reader
                .find_product_listing(product_listing_id)
                .await?
                .filter(|listing| listing.lifecycle != ListingLifecycle::Withdrawn)
                .ok_or(SubmitContentNoticeError::TargetNotFound)?;
            Ok((
                ContentNoticeTarget::ProductListing {
                    product_listing_id: listing.product_listing_id,
                    listing_source_id: listing.listing_source.listing_source_id,
                },
                TargetSnapshot {
                    listing_source: listing.listing_source.snapshot,
                    product_listing: Some(listing.snapshot),
                },
            ))
        }
    }
}

impl From<ContentNoticeTargetReadError> for SubmitContentNoticeError {
    fn from(error: ContentNoticeTargetReadError) -> Self {
        match error {
            ContentNoticeTargetReadError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ContentNoticeTargetReadError::InvalidReadModel { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

impl From<ContentNoticeRepositoryError> for SubmitContentNoticeError {
    fn from(error: ContentNoticeRepositoryError) -> Self {
        match error {
            ContentNoticeRepositoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ContentNoticeRepositoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
            ContentNoticeRepositoryError::Internal { source } => Self::Internal { source },
        }
    }
}

impl From<ContentNoticeSubmissionHistoryError> for SubmitContentNoticeError {
    fn from(error: ContentNoticeSubmissionHistoryError) -> Self {
        match error {
            ContentNoticeSubmissionHistoryError::TemporarilyUnavailable { source } => {
                Self::TemporarilyUnavailable { source }
            }
            ContentNoticeSubmissionHistoryError::InvalidPersistedState { source } => {
                Self::InvalidPersistedState { source }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ContentNoticeStorageVersion, ListingSourceTarget, ProductListingTarget, RecentSubmission,
        VersionedContentNotice,
    };
    use crate::submission_policy::{MAX_NOTICES_PER_ADDRESS, SUBMISSION_WINDOW};
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
        transaction::TransactionError,
    };
    use content_notice_core::{
        content_notice_state::ContentNoticeState,
        content_notice_target::{
            ListingSourceSnapshot, ProductListingSnapshot, ProductListingTitleSnapshot,
        },
        submitter::{SubmitterEmail, SubmitterName},
    };
    use domain_primitives::versioned::Versioned;
    use listing_source_core::ListingSourceName;
    use localization::Language;
    use std::sync::{Arc, Mutex, MutexGuard};
    use time::{Duration, OffsetDateTime, macros::datetime};
    use url::Url;

    const NOW: OffsetDateTime = datetime!(2026-11-14 12:00:00 UTC);

    #[derive(Default)]
    struct State {
        begin_fails: bool,
        commit_fails: bool,
        begins: usize,
        commits: usize,
        listing_sources: Vec<ListingSourceTarget>,
        product_listings: Vec<ProductListingTarget>,
        read_error: Option<ContentNoticeTargetReadError>,
        insert_error: Option<ContentNoticeRepositoryError>,
        history_error: Option<ContentNoticeSubmissionHistoryError>,
        inserted: Vec<(ContentNotice, ContentNoticeSubmissionFingerprint)>,
        /// Stored submissions per lowercased address, as the history port groups them.
        history: Vec<(String, RecentSubmission)>,
        /// The limit steps in the order they ran.
        steps: Vec<String>,
    }

    type Shared = Arc<Mutex<State>>;

    fn lock(state: &Shared) -> MutexGuard<'_, State> {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    struct FakeUnitOfWork(Shared);
    struct FakeTransaction(Shared);
    struct FakeTargets(Shared);
    struct FakeTargetReader(Shared);
    struct FakeNotices(Shared);
    struct FakeNoticeRepository(Shared);
    struct FakeHistories(Shared);
    struct FakeHistory(Shared);
    struct FixedClock;

    fn address_key(email: &SubmitterEmail) -> String {
        email.as_str().to_lowercase()
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTransaction;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            let mut state = lock(&self.0);
            if state.begin_fails {
                return Err(TransactionError::BeginFailed(static_error("begin failed")));
            }
            state.begins += 1;
            Ok(FakeTransaction(Arc::clone(&self.0)))
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTransaction {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock(&self.0);
            if state.commit_fails {
                return Err(TransactionError::CommitFailed(static_error(
                    "commit failed",
                )));
            }
            state.commits += 1;
            Ok(())
        }
    }

    impl ContentNoticeTargetReaderFactory<FakeTransaction> for FakeTargets {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ContentNoticeTargetReader + 'tx {
            FakeTargetReader(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ContentNoticeTargetReader for FakeTargetReader {
        async fn find_listing_source(
            &mut self,
            listing_source_id: ListingSourceId,
        ) -> Result<Option<ListingSourceTarget>, ContentNoticeTargetReadError> {
            let mut state = lock(&self.0);
            if let Some(error) = state.read_error.take() {
                return Err(error);
            }
            Ok(state
                .listing_sources
                .iter()
                .find(|source| source.listing_source_id == listing_source_id)
                .cloned())
        }

        async fn find_product_listing(
            &mut self,
            product_listing_id: ProductListingId,
        ) -> Result<Option<ProductListingTarget>, ContentNoticeTargetReadError> {
            let mut state = lock(&self.0);
            if let Some(error) = state.read_error.take() {
                return Err(error);
            }
            Ok(state
                .product_listings
                .iter()
                .find(|listing| listing.product_listing_id == product_listing_id)
                .cloned())
        }
    }

    impl ContentNoticeRepositoryFactory<FakeTransaction> for FakeNotices {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ContentNoticeRepository + 'tx {
            FakeNoticeRepository(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ContentNoticeRepository for FakeNoticeRepository {
        async fn insert(
            &mut self,
            notice: &ContentNotice,
            fingerprint: ContentNoticeSubmissionFingerprint,
        ) -> Result<VersionedContentNotice, ContentNoticeRepositoryError> {
            let mut state = lock(&self.0);
            if let Some(error) = state.insert_error.take() {
                return Err(error);
            }
            state.steps.push("insert".to_owned());
            state.inserted.push((notice.clone(), fingerprint));
            state.history.push((
                address_key(&notice.submitter().email),
                RecentSubmission {
                    fingerprint,
                    created: NOW,
                },
            ));
            Ok(Versioned::new(
                notice.clone(),
                ContentNoticeStorageVersion::INITIAL,
            ))
        }

        async fn find_by_id(
            &mut self,
            _id: ContentNoticeId,
        ) -> Result<Option<VersionedContentNotice>, ContentNoticeRepositoryError> {
            unreachable!("submission never reads a notice back")
        }
    }

    impl ContentNoticeSubmissionHistoryFactory<FakeTransaction> for FakeHistories {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTransaction,
        ) -> impl ContentNoticeSubmissionHistory + 'tx {
            FakeHistory(Arc::clone(&self.0))
        }
    }

    #[async_trait::async_trait]
    impl ContentNoticeSubmissionHistory for FakeHistory {
        async fn lock_address(
            &mut self,
            email: &SubmitterEmail,
        ) -> Result<(), ContentNoticeSubmissionHistoryError> {
            let mut state = lock(&self.0);
            if let Some(error) = state.history_error.take() {
                return Err(error);
            }
            state.steps.push(format!("lock {}", address_key(email)));
            Ok(())
        }

        async fn submissions_after(
            &mut self,
            email: &SubmitterEmail,
            after: OffsetDateTime,
        ) -> Result<Vec<RecentSubmission>, ContentNoticeSubmissionHistoryError> {
            let mut state = lock(&self.0);
            state.steps.push(format!("read after {after}"));
            let key = address_key(email);
            Ok(state
                .history
                .iter()
                .filter(|(address, submission)| *address == key && submission.created > after)
                .map(|(_, submission)| *submission)
                .collect())
        }
    }

    impl ContentNoticeClock for FixedClock {
        fn now_utc(&self) -> OffsetDateTime {
            NOW
        }
    }

    fn handler(state: &Shared) -> impl SubmitContentNoticeUseCase {
        SubmitContentNoticeHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeTargets(Arc::clone(state)),
            FakeNotices(Arc::clone(state)),
            FakeHistories(Arc::clone(state)),
            FixedClock,
        )
    }

    /// `count` distinct notices from `email`, stored within the window.
    fn stored_from(email: &str, count: usize) -> Vec<(String, RecentSubmission)> {
        (0..count)
            .map(|index| {
                (
                    email.to_lowercase(),
                    RecentSubmission {
                        fingerprint: ContentNoticeSubmissionFingerprint::from_sha256(
                            [index as u8; 32],
                        ),
                        created: NOW - Duration::hours(23) + Duration::minutes(index as i64),
                    },
                )
            })
            .collect()
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::Anonymous,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn listing_source(partner: bool) -> ListingSourceTarget {
        ListingSourceTarget {
            listing_source_id: ListingSourceId::new(),
            snapshot: ListingSourceSnapshot {
                name: ListingSourceName::try_from("Antik Shop").unwrap(),
                url: Some(Url::parse("https://shop.example/").unwrap()),
                partner,
            },
        }
    }

    fn product_listing(lifecycle: ListingLifecycle) -> ProductListingTarget {
        ProductListingTarget {
            product_listing_id: ProductListingId::new(),
            lifecycle,
            snapshot: ProductListingSnapshot {
                title: Some(ProductListingTitleSnapshot::try_from("Jugendstil-Vase").unwrap()),
                url: Url::parse("https://shop.example/vase").unwrap(),
            },
            listing_source: listing_source(false),
        }
    }

    fn command(target: ContentNoticeTargetReference) -> SubmitContentNoticeCommand {
        SubmitContentNoticeCommand {
            target,
            submitter_role: SubmitterRole::RightsHolder,
            reason_category: ReasonCategory::RightsInfringement,
            explanation: Some(NoticeExplanation::try_from("My photograph.").unwrap()),
            additional_urls: AdditionalUrls::default(),
            submitter: Submitter {
                name: SubmitterName::try_from("Jane Doe").unwrap(),
                email: SubmitterEmail::try_from("jane@example.com").unwrap(),
                language: Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        }
    }

    fn source_reference(source: &ListingSourceTarget) -> ContentNoticeTargetReference {
        ContentNoticeTargetReference::ListingSource {
            listing_source_id: source.listing_source_id,
        }
    }

    fn listing_reference(listing: &ProductListingTarget) -> ContentNoticeTargetReference {
        ContentNoticeTargetReference::ProductListing {
            product_listing_id: listing.product_listing_id,
        }
    }

    fn state(configure: impl FnOnce(&mut State)) -> Shared {
        let mut state = State::default();
        configure(&mut state);
        Arc::new(Mutex::new(state))
    }

    #[tokio::test]
    async fn should_store_notice_for_active_listing_with_listing_and_source_snapshot() {
        let listing = product_listing(ListingLifecycle::Active);
        let state = state(|state| state.product_listings.push(listing.clone()));

        let result = handler(&state)
            .execute(&context(), command(listing_reference(&listing)))
            .await
            .unwrap();

        let state = lock(&state);
        let (notice, fingerprint) = &state.inserted[0];
        assert_eq!(
            SubmitContentNoticeResult::Stored {
                content_notice_id: notice.id()
            },
            result
        );
        assert_eq!(
            ContentNoticeTarget::ProductListing {
                product_listing_id: listing.product_listing_id,
                listing_source_id: listing.listing_source.listing_source_id,
            },
            notice.target()
        );
        assert_eq!(
            &TargetSnapshot {
                listing_source: listing.listing_source.snapshot.clone(),
                product_listing: Some(listing.snapshot.clone()),
            },
            notice.target_snapshot()
        );
        assert_eq!(ContentNoticeState::Submitted, notice.state());
        assert_eq!(submission_fingerprint(notice), *fingerprint);
        assert_eq!((1, 1), (state.begins, state.commits));
    }

    #[tokio::test]
    async fn should_snapshot_partner_status_of_listing_source() {
        for partner in [true, false] {
            let source = listing_source(partner);
            let state = state(|state| state.listing_sources.push(source.clone()));

            handler(&state)
                .execute(&context(), command(source_reference(&source)))
                .await
                .unwrap();

            let state = lock(&state);
            let notice = &state.inserted[0].0;
            assert_eq!(
                ContentNoticeTarget::ListingSource {
                    listing_source_id: source.listing_source_id,
                },
                notice.target()
            );
            assert_eq!(
                &TargetSnapshot {
                    listing_source: source.snapshot.clone(),
                    product_listing: None,
                },
                notice.target_snapshot()
            );
            assert_eq!(partner, notice.target_snapshot().listing_source.partner);
        }
    }

    #[tokio::test]
    async fn should_report_missing_or_withdrawn_target_as_not_found_without_storing() {
        let withdrawn = product_listing(ListingLifecycle::Withdrawn);
        let state = state(|state| state.product_listings.push(withdrawn.clone()));

        for reference in [
            listing_reference(&withdrawn),
            listing_reference(&product_listing(ListingLifecycle::Active)),
            source_reference(&listing_source(true)),
        ] {
            let result = handler(&state)
                .execute(&context(), command(reference))
                .await;

            assert!(matches!(
                result,
                Err(SubmitContentNoticeError::TargetNotFound)
            ));
        }
        let state = lock(&state);
        assert!(state.inserted.is_empty());
        assert_eq!(0, state.commits);
    }

    #[tokio::test]
    async fn should_reject_broken_submission_rules_before_beginning_a_transaction() {
        let source = listing_source(false);
        let mut missing_explanation = command(source_reference(&source));
        missing_explanation.explanation = None;
        let mut opt_out_by_rights_holder = command(source_reference(&source));
        opt_out_by_rights_holder.reason_category = ReasonCategory::ProviderOptOut;
        let state = state(|state| state.listing_sources.push(source.clone()));

        for (command, expected) in [
            (
                missing_explanation,
                ContentNoticeInvariantError::ExplanationRequired {
                    reason_category: ReasonCategory::RightsInfringement,
                },
            ),
            (
                opt_out_by_rights_holder,
                ContentNoticeInvariantError::ProviderOptOutRequiresProvider {
                    submitter_role: SubmitterRole::RightsHolder,
                },
            ),
        ] {
            let result = handler(&state).execute(&context(), command).await;

            assert!(
                matches!(result, Err(SubmitContentNoticeError::InvalidNotice(error)) if error == expected)
            );
        }
        assert_eq!(0, lock(&state).begins);
    }

    #[tokio::test]
    async fn should_accept_provider_opt_out_without_explanation() {
        let source = listing_source(false);
        let mut opt_out = command(source_reference(&source));
        opt_out.submitter_role = SubmitterRole::Provider;
        opt_out.reason_category = ReasonCategory::ProviderOptOut;
        opt_out.explanation = None;
        let state = state(|state| state.listing_sources.push(source.clone()));

        handler(&state).execute(&context(), opt_out).await.unwrap();

        assert_eq!(None, lock(&state).inserted[0].0.explanation());
    }

    #[tokio::test]
    async fn should_report_begin_and_commit_failures_as_transaction_failures() {
        let source = listing_source(false);
        let begin_fails = state(|state| {
            state.begin_fails = true;
            state.listing_sources.push(source.clone());
        });
        let commit_fails = state(|state| {
            state.commit_fails = true;
            state.listing_sources.push(source.clone());
        });

        let begin = handler(&begin_fails)
            .execute(&context(), command(source_reference(&source)))
            .await;
        let commit = handler(&commit_fails)
            .execute(&context(), command(source_reference(&source)))
            .await;

        assert!(matches!(
            begin,
            Err(SubmitContentNoticeError::BeginTransactionFailed(_))
        ));
        assert!(lock(&begin_fails).inserted.is_empty());
        assert!(matches!(
            commit,
            Err(SubmitContentNoticeError::CommitTransactionFailed(_))
        ));
        assert_eq!(0, lock(&commit_fails).commits);
    }

    #[tokio::test]
    async fn should_keep_target_read_and_insert_failure_categories_without_committing() {
        let source = listing_source(false);
        type Configure = fn(&mut State);
        type Expect = fn(&SubmitContentNoticeError) -> bool;
        let cases: [(Configure, Expect); 6] = [
            (
                |state| {
                    state.read_error = Some(ContentNoticeTargetReadError::TemporarilyUnavailable {
                        source: static_error("read"),
                    })
                },
                |error| {
                    matches!(
                        error,
                        SubmitContentNoticeError::TemporarilyUnavailable { .. }
                    )
                },
            ),
            (
                |state| {
                    state.read_error = Some(ContentNoticeTargetReadError::InvalidReadModel {
                        source: static_error("read"),
                    })
                },
                |error| {
                    matches!(
                        error,
                        SubmitContentNoticeError::InvalidPersistedState { .. }
                    )
                },
            ),
            (
                |state| {
                    state.insert_error =
                        Some(ContentNoticeRepositoryError::TemporarilyUnavailable {
                            source: static_error("insert"),
                        })
                },
                |error| {
                    matches!(
                        error,
                        SubmitContentNoticeError::TemporarilyUnavailable { .. }
                    )
                },
            ),
            (
                |state| {
                    state.insert_error = Some(ContentNoticeRepositoryError::Internal {
                        source: static_error("insert"),
                    })
                },
                |error| matches!(error, SubmitContentNoticeError::Internal { .. }),
            ),
            (
                |state| {
                    state.history_error = Some(
                        ContentNoticeSubmissionHistoryError::TemporarilyUnavailable {
                            source: static_error("lock"),
                        },
                    )
                },
                |error| {
                    matches!(
                        error,
                        SubmitContentNoticeError::TemporarilyUnavailable { .. }
                    )
                },
            ),
            (
                |state| {
                    state.history_error =
                        Some(ContentNoticeSubmissionHistoryError::InvalidPersistedState {
                            source: static_error("history"),
                        })
                },
                |error| {
                    matches!(
                        error,
                        SubmitContentNoticeError::InvalidPersistedState { .. }
                    )
                },
            ),
        ];

        for (configure, expected) in cases {
            let state = state(|state| {
                state.listing_sources.push(source.clone());
                configure(state);
            });

            let error = handler(&state)
                .execute(&context(), command(source_reference(&source)))
                .await
                .unwrap_err();

            assert!(expected(&error), "{error:?}");
            assert_eq!(0, lock(&state).commits);
        }
    }
    #[tokio::test]
    async fn should_lock_the_address_and_read_its_window_before_storing() {
        let source = listing_source(false);
        let mut submission = command(source_reference(&source));
        submission.submitter.email = SubmitterEmail::try_from("Jane@Example.com").unwrap();
        let state = state(|state| state.listing_sources.push(source.clone()));

        handler(&state)
            .execute(&context(), submission)
            .await
            .unwrap();

        assert_eq!(
            vec![
                "lock jane@example.com".to_owned(),
                format!("read after {}", NOW - SUBMISSION_WINDOW),
                "insert".to_owned(),
            ],
            lock(&state).steps
        );
    }

    #[tokio::test]
    async fn should_store_nothing_but_commit_for_an_identical_resubmission() {
        let source = listing_source(false);
        let state = state(|state| state.listing_sources.push(source.clone()));

        let first = handler(&state)
            .execute(&context(), command(source_reference(&source)))
            .await
            .unwrap();
        let repeated = handler(&state)
            .execute(&context(), command(source_reference(&source)))
            .await
            .unwrap();

        assert!(matches!(first, SubmitContentNoticeResult::Stored { .. }));
        assert_eq!(SubmitContentNoticeResult::Duplicate, repeated);
        let state = lock(&state);
        assert_eq!(1, state.inserted.len());
        assert_eq!(2, state.commits);
    }

    #[tokio::test]
    async fn should_store_the_last_notice_of_the_budget() {
        let source = listing_source(false);
        let state = state(|state| {
            state.listing_sources.push(source.clone());
            state.history = stored_from("jane@example.com", MAX_NOTICES_PER_ADDRESS - 1);
        });

        let result = handler(&state)
            .execute(&context(), command(source_reference(&source)))
            .await
            .unwrap();

        assert!(matches!(result, SubmitContentNoticeResult::Stored { .. }));
        assert_eq!(1, lock(&state).inserted.len());
    }

    #[tokio::test]
    async fn should_rate_limit_without_storing_or_committing_once_the_budget_is_used() {
        let source = listing_source(false);
        let state = state(|state| {
            state.listing_sources.push(source.clone());
            state.history = stored_from("jane@example.com", MAX_NOTICES_PER_ADDRESS);
        });

        let error = handler(&state)
            .execute(&context(), command(source_reference(&source)))
            .await
            .unwrap_err();

        let SubmitContentNoticeError::RateLimited { retry_after } = error else {
            panic!("expected rate limit, got {error:?}");
        };
        assert_eq!(
            Duration::hours(1).whole_seconds() as u64,
            retry_after.seconds()
        );
        let state = lock(&state);
        assert!(state.inserted.is_empty());
        assert_eq!(0, state.commits);
    }
}
