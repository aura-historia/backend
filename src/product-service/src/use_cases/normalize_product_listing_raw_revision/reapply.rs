//! Reapplication: normalizes a stream's already processed latest revision again with the current
//! normalizer, through the same completion as the drain, without moving the stream head
//! ([production reapplication](../../../../../docs/normalization-replay.md#production-reapplication)).

use super::{
    EvaluatedRevision, NORMALIZER_VERSION, NormalizeProductListingRawRevisionError,
    NormalizeProductListingRawRevisionHandler, map_port_error,
};
use crate::ports::{
    NormalizationDiagnosticCaseWriterFactory, ProductListingRawNormalizationOutcome,
    ProductListingRawNormalizationWriter, ProductListingRawNormalizationWriterFactory,
    ProductListingRawReapplicationWork, ReapplicableProductListingRawStreamPageRequest,
    ReapplicableProductListingRawStreamReader,
};
use application::transaction::{Transaction, UnitOfWork};
use listing_source_core::ListingSourceId;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle,
    product_listing::ProductListing,
    product_listing_id::{ProductListingId, ProductListingKey},
};
use product_listing_normalization::{
    NormalizationIssueReason, ProductListingRawValuesNormalizationOutcome,
};
use product_listing_service::ports::{
    ProductListingEventAppenderFactory, ProductListingRawStreamId, ProductListingRepository,
    ProductListingRepositoryFactory,
};

/// Whether a reapplication commits its effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawRevisionReapplicationMode {
    /// Runs every step in a transaction that is rolled back: nothing is written, no event is
    /// logged and no shipping is applied.
    DryRun,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReapplyProductListingRawRevisionsCommand {
    pub mode: RawRevisionReapplicationMode,
    /// Continue after this stream; `None` starts at the first.
    pub after: Option<ProductListingRawStreamId>,
    pub page_size: u32,
    pub listing_source_id: Option<ListingSourceId>,
}

#[derive(Debug, Default)]
pub struct ReapplyProductListingRawRevisionsPage {
    /// The scanned streams that qualified, in ID order.
    pub streams: Vec<ReappliedProductListingRawStream>,
    /// The last scanned stream, to continue after; `None` when the traversal ended.
    pub last: Option<ProductListingRawStreamId>,
}

#[derive(Debug)]
pub struct ReappliedProductListingRawStream {
    pub product_listing_raw_stream_id: ProductListingRawStreamId,
    pub result: Result<RawRevisionReapplication, NormalizeProductListingRawRevisionError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawRevisionReapplication {
    Skipped(RawRevisionReapplicationSkip),
    Completed(CompletedRawRevisionReapplication),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawRevisionReapplicationSkip {
    /// The stream received a newer revision, or the revision was reapplied, since the page was
    /// read. The drain owns newer revisions.
    NoLongerReapplicable,
    /// Another path withdrew the listing; reapplying an upsert would restore it.
    ListingWithdrawn,
    /// Another stream bound to the same listing processed a revision captured at the same time or
    /// later, such as a URL variant that replaced this one; reapplying would restore older state.
    SupersededByAnotherStream,
}

impl RawRevisionReapplicationSkip {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoLongerReapplicable => "NO_LONGER_REAPPLICABLE",
            Self::ListingWithdrawn => "LISTING_WITHDRAWN",
            Self::SupersededByAnotherStream => "SUPERSEDED_BY_ANOTHER_STREAM",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedRawRevisionReapplication {
    pub recorded_outcome: ProductListingRawNormalizationOutcome,
    pub outcome: ProductListingRawNormalizationOutcome,
    pub error_code: Option<&'static str>,
    pub changes: ProductListingChanges,
    pub issue_reasons: Vec<NormalizationIssueReason>,
}

/// The canonical facts a reapplication changed on its listing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProductListingChanges {
    pub created: bool,
    pub title: bool,
    pub description: bool,
    pub price: bool,
    pub price_estimate: bool,
    pub availability: bool,
    pub url: bool,
    pub images: bool,
}

impl ProductListingChanges {
    fn between(before: Option<&ProductListing>, after: Option<&ProductListing>) -> Self {
        let (Some(before), Some(after)) = (before, after) else {
            return Self {
                created: before.is_none() && after.is_some(),
                ..Self::default()
            };
        };
        let (old, new) = (before.pricing(), after.pricing());
        Self {
            created: false,
            title: before.title() != after.title(),
            description: before.description() != after.description(),
            price: old.price != new.price,
            price_estimate: old.price_estimate_min != new.price_estimate_min
                || old.price_estimate_max != new.price_estimate_max,
            availability: before.availability() != after.availability(),
            url: before.url() != after.url(),
            images: before.images() != after.images(),
        }
    }

    /// The changed facts' names, in a stable order.
    pub fn names(self) -> Vec<&'static str> {
        [
            (self.created, "created"),
            (self.title, "title"),
            (self.description, "description"),
            (self.price, "price"),
            (self.price_estimate, "priceEstimate"),
            (self.availability, "availability"),
            (self.url, "url"),
            (self.images, "images"),
        ]
        .into_iter()
        .filter_map(|(changed, name)| changed.then_some(name))
        .collect()
    }
}

#[async_trait::async_trait]
pub trait ReapplyProductListingRawRevisionsUseCase: Send + Sync {
    /// Scans one page of streams and reapplies those that qualify, each in its own transaction.
    /// A failed stream is reported and the page continues; only a failed page read fails the
    /// call.
    async fn reapply_page(
        &self,
        command: ReapplyProductListingRawRevisionsCommand,
    ) -> Result<ReapplyProductListingRawRevisionsPage, NormalizeProductListingRawRevisionError>;
}

#[async_trait::async_trait]
impl<U, W, R, E, P, D> ReapplyProductListingRawRevisionsUseCase
    for NormalizeProductListingRawRevisionHandler<U, W, R, E, P, D>
where
    U: UnitOfWork + Send + Sync,
    W: ProductListingRawNormalizationWriterFactory<U::Tx> + Send + Sync,
    R: ProductListingRepositoryFactory<U::Tx> + Send + Sync,
    E: ProductListingEventAppenderFactory<U::Tx> + Send + Sync,
    P: ReapplicableProductListingRawStreamReader + Send + Sync,
    D: NormalizationDiagnosticCaseWriterFactory<U::Tx> + Send + Sync,
{
    async fn reapply_page(
        &self,
        command: ReapplyProductListingRawRevisionsCommand,
    ) -> Result<ReapplyProductListingRawRevisionsPage, NormalizeProductListingRawRevisionError>
    {
        if command.page_size == 0 {
            return Err(NormalizeProductListingRawRevisionError::InvalidLimit);
        }
        let scanned =
            self.pending_streams
                .list_reapplicable_streams(ReapplicableProductListingRawStreamPageRequest {
                    after: command.after,
                    limit: command.page_size,
                    listing_source_id: command.listing_source_id,
                    normalizer_version: NORMALIZER_VERSION,
                })
                .await
                .map_err(|source| {
                    NormalizeProductListingRawRevisionError::PendingStreamReadFailed { source }
                })?;
        let mut page = ReapplyProductListingRawRevisionsPage {
            streams: Vec::with_capacity(scanned.reapplicable.len() + scanned.superseded.len()),
            last: scanned.last_scanned,
        };
        page.streams.extend(
            scanned
                .superseded
                .into_iter()
                .map(
                    |product_listing_raw_stream_id| ReappliedProductListingRawStream {
                        product_listing_raw_stream_id,
                        result: Ok(RawRevisionReapplication::Skipped(
                            RawRevisionReapplicationSkip::SupersededByAnotherStream,
                        )),
                    },
                ),
        );
        for product_listing_raw_stream_id in scanned.reapplicable {
            let result = self
                .reapply_stream(product_listing_raw_stream_id, command.mode)
                .await;
            page.streams.push(ReappliedProductListingRawStream {
                product_listing_raw_stream_id,
                result,
            });
        }
        Ok(page)
    }
}

impl<U, W, R, E, P, D> NormalizeProductListingRawRevisionHandler<U, W, R, E, P, D>
where
    U: UnitOfWork,
    W: ProductListingRawNormalizationWriterFactory<U::Tx>,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventAppenderFactory<U::Tx>,
    D: NormalizationDiagnosticCaseWriterFactory<U::Tx>,
{
    async fn reapply_stream(
        &self,
        product_listing_raw_stream_id: ProductListingRawStreamId,
        mode: RawRevisionReapplicationMode,
    ) -> Result<RawRevisionReapplication, NormalizeProductListingRawRevisionError> {
        let mut tx = self.unit_of_work.begin().await.map_err(|source| {
            NormalizeProductListingRawRevisionError::BeginTransactionFailed { source }
        })?;
        let Some(work) = self
            .raw_normalizations
            .in_transaction(&mut tx)
            .lock_reapplication(product_listing_raw_stream_id, NORMALIZER_VERSION)
            .await
            .map_err(map_port_error)?
        else {
            return Ok(RawRevisionReapplication::Skipped(
                RawRevisionReapplicationSkip::NoLongerReapplicable,
            ));
        };
        let evaluated = match self.evaluate(&work.revision) {
            Ok(evaluated) => evaluated,
            Err(failure) => {
                // Recording takes the head again, so this transaction must end first.
                drop(tx);
                return Err(match mode {
                    RawRevisionReapplicationMode::Apply => {
                        self.record_evaluation_failure(&work.revision, failure)
                            .await
                    }
                    RawRevisionReapplicationMode::DryRun => failure.error,
                });
            }
        };
        let before = self.current_listing(&mut tx, &work, &evaluated).await?;
        if let Some(listing) = &before {
            if listing.lifecycle() == ListingLifecycle::Withdrawn {
                return Ok(RawRevisionReapplication::Skipped(
                    RawRevisionReapplicationSkip::ListingWithdrawn,
                ));
            }
            // The page checked this too, but a stream may have bound the listing since, and an
            // unbound head finds its listing only now.
            if self
                .raw_normalizations
                .in_transaction(&mut tx)
                .is_superseded(product_listing_raw_stream_id, listing.id())
                .await
                .map_err(map_port_error)?
            {
                return Ok(RawRevisionReapplication::Skipped(
                    RawRevisionReapplicationSkip::SupersededByAnotherStream,
                ));
            }
        }
        let ProductListingRawReapplicationWork {
            head,
            revision,
            recorded_outcome,
        } = work;
        let completed = self
            .complete_revision(&mut tx, head, &revision, evaluated)
            .await?;
        let completion = &completed.completion;
        let changes = match (completion.outcome, completion.product_listing_id) {
            (ProductListingRawNormalizationOutcome::Applied, Some(product_listing_id)) => {
                let after = self.listing(&mut tx, product_listing_id).await?;
                ProductListingChanges::between(before.as_ref(), after.as_ref())
            }
            _ => ProductListingChanges::default(),
        };
        let reapplication = CompletedRawRevisionReapplication {
            recorded_outcome,
            outcome: completion.outcome,
            error_code: completion.error_code,
            changes,
            issue_reasons: completed
                .after_commit
                .issues
                .iter()
                .map(|issue| issue.reason)
                .collect(),
        };
        self.raw_normalizations
            .in_transaction(&mut tx)
            .complete_reapplication(completed.completion)
            .await
            .map_err(map_port_error)?;
        match mode {
            RawRevisionReapplicationMode::DryRun => drop(tx),
            RawRevisionReapplicationMode::Apply => {
                tx.commit().await.map_err(|source| {
                    NormalizeProductListingRawRevisionError::CommitTransactionFailed { source }
                })?;
                self.finish_committed(completed.after_commit.without_crawler_shipping())
                    .await;
            }
        }
        Ok(RawRevisionReapplication::Completed(reapplication))
    }

    /// The listing the completion will write: the head's bound listing, or for an unbound head
    /// the one the resolved source identity names, as the canonical writer finds it.
    async fn current_listing(
        &self,
        tx: &mut U::Tx,
        work: &ProductListingRawReapplicationWork,
        evaluated: &EvaluatedRevision,
    ) -> Result<Option<ProductListing>, NormalizeProductListingRawRevisionError> {
        if let Some(product_listing_id) = work.head.product_listing_id {
            return self.listing(tx, product_listing_id).await;
        }
        let ProductListingRawValuesNormalizationOutcome::Resolved(resolved) = &evaluated.normalized
        else {
            return Ok(None);
        };
        let key = ProductListingKey::new(
            work.head.listing_source_id,
            resolved.source_listing_id.clone(),
        );
        self.products
            .in_transaction(tx)
            .find_by_key(&key)
            .await
            .map(|listing| listing.map(|listing| listing.value))
            .map_err(|source| NormalizeProductListingRawRevisionError::ListingReadFailed { source })
    }

    async fn listing(
        &self,
        tx: &mut U::Tx,
        product_listing_id: ProductListingId,
    ) -> Result<Option<ProductListing>, NormalizeProductListingRawRevisionError> {
        self.products
            .in_transaction(tx)
            .find_by_id(product_listing_id)
            .await
            .map(|listing| listing.map(|listing| listing.value))
            .map_err(|source| NormalizeProductListingRawRevisionError::ListingReadFailed { source })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{
        DiagnosticCasesFake, SharedState, TestHandler, TestState, existing_listing, handler, lock,
        raw_head, revision, upsert_input,
    };
    use super::*;
    use domain_primitives::versioned::Versioned;
    use product_listing_core::source_listing_id::SourceListingId;
    use product_listing_service::ports::ProductListingStorageVersion;
    use std::sync::{Arc, Mutex};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// A stream whose head processed revision 3, bound to an existing listing without a price.
    struct Fixture {
        state: SharedState,
        stream_id: ProductListingRawStreamId,
        product_listing_id: ProductListingId,
    }

    impl Fixture {
        fn new(price: &str, withdrawn: bool) -> Result<Self, Box<dyn std::error::Error>> {
            let stream_id = ProductListingRawStreamId::new();
            let listing_source_id = ListingSourceId::new();
            let product_listing_id = ProductListingId::new();
            let source_listing_id = SourceListingId::try_from("source-123")?;
            let (mut listing, _) = existing_listing(
                product_listing_id,
                listing_source_id,
                source_listing_id.clone(),
            )?;
            if withdrawn {
                listing.withdraw()?;
                listing.take_pending_event_payload();
            }
            let mut head = raw_head(
                stream_id,
                listing_source_id,
                Some(product_listing_id),
                Some(source_listing_id),
            );
            head.last_processed_revision = 3;
            let state = Arc::new(Mutex::new(TestState::default()));
            {
                let mut state = lock(&state);
                state.existing_product = Some(Versioned::new(
                    listing,
                    ProductListingStorageVersion::INITIAL,
                ));
                state
                    .reapplications
                    .push(ProductListingRawReapplicationWork {
                        head,
                        revision: revision(stream_id, 3, upsert_input(price)?),
                        recorded_outcome: ProductListingRawNormalizationOutcome::Rejected,
                    });
                state.reapplicable_page.reapplicable = vec![stream_id];
                state.reapplicable_page.last_scanned = Some(stream_id);
            }
            Ok(Self {
                state,
                stream_id,
                product_listing_id,
            })
        }

        fn handler(&self) -> TestHandler {
            handler(&self.state)
        }

        async fn reapply(
            &self,
            mode: RawRevisionReapplicationMode,
        ) -> Result<RawRevisionReapplication, Box<dyn std::error::Error>> {
            let page = self
                .handler()
                .with_diagnostic_cases(DiagnosticCasesFake(Arc::clone(&self.state)))
                .reapply_page(command(mode))
                .await?;
            assert_eq!(Some(self.stream_id), page.last);
            let [stream] = <[_; 1]>::try_from(page.streams)
                .map_err(|_| std::io::Error::other("one stream qualifies"))?;
            assert_eq!(self.stream_id, stream.product_listing_raw_stream_id);
            Ok(stream.result?)
        }
    }

    fn command(mode: RawRevisionReapplicationMode) -> ReapplyProductListingRawRevisionsCommand {
        ReapplyProductListingRawRevisionsCommand {
            mode,
            after: None,
            page_size: 10,
            listing_source_id: None,
        }
    }

    mod applying_a_revision_that_now_resolves {
        use super::*;

        #[tokio::test]
        async fn should_apply_it_and_record_the_completion_without_moving_the_head() -> TestResult {
            let fixture = Fixture::new("EUR 1.000 - 1.500", false)?;

            let result = fixture.reapply(RawRevisionReapplicationMode::Apply).await?;

            let RawRevisionReapplication::Completed(reapplication) = result else {
                panic!("the revision is reapplied");
            };
            assert_eq!(
                ProductListingRawNormalizationOutcome::Rejected,
                reapplication.recorded_outcome
            );
            assert_eq!(
                ProductListingRawNormalizationOutcome::Applied,
                reapplication.outcome
            );
            assert!(reapplication.changes.price);
            assert!(!reapplication.changes.created);
            let state = lock(&fixture.state);
            let [completion] = state.reapplied.as_slice() else {
                panic!("one reapplication is recorded");
            };
            assert_eq!(3, completion.revision);
            assert_eq!(NORMALIZER_VERSION, completion.normalizer_version);
            assert_eq!(
                Some(fixture.product_listing_id),
                completion.product_listing_id
            );
            assert!(state.completions.is_empty(), "the head never advances");
            assert_eq!(1, state.commits);
            Ok(())
        }
    }

    mod a_dry_run {
        use super::*;

        #[tokio::test]
        async fn should_report_the_change_and_roll_everything_back() -> TestResult {
            let fixture = Fixture::new("EUR 1.000 - 1.500", false)?;

            let result = fixture
                .reapply(RawRevisionReapplicationMode::DryRun)
                .await?;

            let RawRevisionReapplication::Completed(reapplication) = result else {
                panic!("the revision is evaluated");
            };
            // The existing listing has no title or description yet; the revision sets them too.
            assert_eq!(
                vec!["title", "description", "price"],
                reapplication.changes.names()
            );
            let state = lock(&fixture.state);
            assert_eq!(0, state.commits);
            assert_eq!(1, state.rollbacks);
            Ok(())
        }
    }

    mod a_rejected_reevaluation {
        use super::*;

        #[tokio::test]
        async fn should_record_the_rejection_and_leave_the_listing_alone() -> TestResult {
            let fixture = Fixture::new("-100 EUR", false)?;

            let result = fixture.reapply(RawRevisionReapplicationMode::Apply).await?;

            let RawRevisionReapplication::Completed(reapplication) = result else {
                panic!("the revision is reapplied");
            };
            assert_eq!(
                ProductListingRawNormalizationOutcome::Rejected,
                reapplication.outcome
            );
            assert_eq!(ProductListingChanges::default(), reapplication.changes);
            assert_eq!(
                vec![NormalizationIssueReason::PriceNegative],
                reapplication.issue_reasons
            );
            let state = lock(&fixture.state);
            assert!(state.updated_products.is_empty());
            assert_eq!(1, state.reapplied.len());
            assert_eq!(1, state.diagnostic_cases.len());
            Ok(())
        }
    }

    mod a_withdrawn_listing {
        use super::*;

        #[tokio::test]
        async fn should_be_skipped_so_the_upsert_never_restores_it() -> TestResult {
            let fixture = Fixture::new("EUR 250", true)?;

            let result = fixture.reapply(RawRevisionReapplicationMode::Apply).await?;

            assert_eq!(
                RawRevisionReapplication::Skipped(RawRevisionReapplicationSkip::ListingWithdrawn),
                result
            );
            let state = lock(&fixture.state);
            assert!(state.updated_products.is_empty());
            assert!(state.reapplied.is_empty());
            assert_eq!(0, state.commits);
            Ok(())
        }
    }

    mod a_stream_another_stream_superseded_since_the_page {
        use super::*;

        #[tokio::test]
        async fn should_be_skipped_under_the_lock() -> TestResult {
            let fixture = Fixture::new("EUR 1.000 - 1.500", false)?;
            lock(&fixture.state)
                .superseded_streams
                .insert(fixture.stream_id);

            let result = fixture.reapply(RawRevisionReapplicationMode::Apply).await?;

            assert_eq!(
                RawRevisionReapplication::Skipped(
                    RawRevisionReapplicationSkip::SupersededByAnotherStream
                ),
                result
            );
            let state = lock(&fixture.state);
            assert!(state.updated_products.is_empty());
            assert!(state.reapplied.is_empty());
            assert_eq!(0, state.commits);
            Ok(())
        }
    }

    mod a_page_with_a_superseded_stream {
        use super::*;

        #[tokio::test]
        async fn should_report_it_without_locking_it() -> TestResult {
            let fixture = Fixture::new("EUR 250", false)?;
            {
                let mut state = lock(&fixture.state);
                state.reapplicable_page.reapplicable.clear();
                state.reapplicable_page.superseded = vec![fixture.stream_id];
            }

            let result = fixture.reapply(RawRevisionReapplicationMode::Apply).await?;

            assert_eq!(
                RawRevisionReapplication::Skipped(
                    RawRevisionReapplicationSkip::SupersededByAnotherStream
                ),
                result
            );
            assert_eq!(0, lock(&fixture.state).begins);
            Ok(())
        }
    }

    mod a_stream_that_moved_on {
        use super::*;

        #[tokio::test]
        async fn should_be_skipped_for_the_drain() -> TestResult {
            let fixture = Fixture::new("EUR 250", false)?;
            lock(&fixture.state).reapplications.clear();

            let result = fixture.reapply(RawRevisionReapplicationMode::Apply).await?;

            assert_eq!(
                RawRevisionReapplication::Skipped(
                    RawRevisionReapplicationSkip::NoLongerReapplicable
                ),
                result
            );
            assert!(lock(&fixture.state).reapplied.is_empty());
            Ok(())
        }
    }

    mod a_page_without_qualifying_streams {
        use super::*;

        #[tokio::test]
        async fn should_continue_after_the_last_scanned_stream() -> TestResult {
            let fixture = Fixture::new("EUR 250", false)?;
            lock(&fixture.state).reapplicable_page.reapplicable.clear();

            let page = fixture
                .handler()
                .reapply_page(command(RawRevisionReapplicationMode::Apply))
                .await?;

            assert!(page.streams.is_empty());
            assert_eq!(Some(fixture.stream_id), page.last);
            let state = lock(&fixture.state);
            let [request] = state.reapplicable_requests.as_slice() else {
                panic!("one page is read");
            };
            assert_eq!(NORMALIZER_VERSION, request.normalizer_version);
            assert_eq!(10, request.limit);
            Ok(())
        }
    }
}
