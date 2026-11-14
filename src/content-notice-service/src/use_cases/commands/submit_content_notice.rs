use crate::{
    ports::{
        ContentNoticeRepository, ContentNoticeRepositoryError, ContentNoticeRepositoryFactory,
        ContentNoticeTargetReadError, ContentNoticeTargetReader, ContentNoticeTargetReaderFactory,
    },
    submission_fingerprint::submission_fingerprint,
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
    submitter::Submitter,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmitContentNoticeResult {
    pub content_notice_id: ContentNoticeId,
}

#[derive(Debug, thiserror::Error)]
pub enum SubmitContentNoticeError {
    #[error("content notice breaks a submission rule")]
    InvalidNotice(#[source] ContentNoticeInvariantError),
    #[error("content notice target not found")]
    TargetNotFound,
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

/// Records a content notice from anyone, with or without an account. The caller's identity is
/// used for logging only and is never stored with the notice.
#[async_trait::async_trait]
pub trait SubmitContentNoticeUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SubmitContentNoticeCommand,
    ) -> Result<SubmitContentNoticeResult, SubmitContentNoticeError>;
}

pub struct SubmitContentNoticeHandler<U, T, R> {
    unit_of_work: U,
    targets: T,
    notices: R,
}

impl<U, T, R> SubmitContentNoticeHandler<U, T, R> {
    pub fn new(unit_of_work: U, targets: T, notices: R) -> Self {
        Self {
            unit_of_work,
            targets,
            notices,
        }
    }
}

#[async_trait::async_trait]
impl<U, T, R> SubmitContentNoticeUseCase for SubmitContentNoticeHandler<U, T, R>
where
    U: UnitOfWork,
    T: ContentNoticeTargetReaderFactory<U::Tx>,
    R: ContentNoticeRepositoryFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "submit_content_notice",
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
        command: SubmitContentNoticeCommand,
    ) -> Result<SubmitContentNoticeResult, SubmitContentNoticeError> {
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
        self.record(&mut tx, &notice).await?;
        tx.commit().await.map_err(|source| {
            SubmitContentNoticeError::CommitTransactionFailed(Box::new(source))
        })?;

        tracing::info!(
            event = "content_notice.submitted",
            content_notice_id = %notice.id(),
            target_type = notice.target().type_code(),
            listing_source_id = %notice.target().listing_source_id(),
            product_listing_id = notice.target().product_listing_id().map(tracing::field::display),
            submitter_role = notice.submitter_role().as_str(),
            reason_category = notice.reason_category().as_str(),
            actor_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
            outcome = "success",
        );
        Ok(SubmitContentNoticeResult {
            content_notice_id: notice.id(),
        })
    }
}

impl<U, T, R> SubmitContentNoticeHandler<U, T, R>
where
    U: UnitOfWork,
    R: ContentNoticeRepositoryFactory<U::Tx>,
{
    /// Stores the notice in the submission transaction. Everything that must commit or roll
    /// back together with the notice belongs here.
    async fn record(
        &self,
        tx: &mut U::Tx,
        notice: &ContentNotice,
    ) -> Result<(), SubmitContentNoticeError> {
        self.notices
            .in_transaction(tx)
            .insert(notice, submission_fingerprint(notice))
            .await?;
        Ok(())
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ContentNoticeStorageVersion, ContentNoticeSubmissionFingerprint, ListingSourceTarget,
        ProductListingTarget, VersionedContentNotice,
    };
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
    use url::Url;

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
        inserted: Vec<(ContentNotice, ContentNoticeSubmissionFingerprint)>,
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
            state.inserted.push((notice.clone(), fingerprint));
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

    fn handler(state: &Shared) -> impl SubmitContentNoticeUseCase {
        SubmitContentNoticeHandler::new(
            FakeUnitOfWork(Arc::clone(state)),
            FakeTargets(Arc::clone(state)),
            FakeNotices(Arc::clone(state)),
        )
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
        assert_eq!(result.content_notice_id, notice.id());
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
        let cases: [(Configure, Expect); 4] = [
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
}
