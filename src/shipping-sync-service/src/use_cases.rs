use crate::{ports::*, *};
use application::{
    operation_context::{CredentialCapability, OperationContext},
    transaction::{Transaction, UnitOfWork},
};
use listing_source_service::ports::location_default::{
    SourceLocationAccess, SourceLocationAccessFactory, SourceLocationAccessPolicy,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub enum SyncCommand {
    Cleanup {
        limit: u32,
    },
    Rotate {
        integration: DeliveryIntegration,
        expected_revision: i64,
    },
    Current {
        integration: DeliveryIntegration,
    },
    Revoke {
        epoch: Uuid,
    },
    Head {
        epoch: Uuid,
        scope: ExternalKey,
    },
    Begin(Begin),
    Capture {
        id: Uuid,
        index: u32,
        chunk: Chunk,
    },
    Finalize {
        id: Uuid,
        manifest: Manifest,
    },
    Status {
        id: Uuid,
    },
    Pending {
        epoch: Uuid,
        after: Option<Uuid>,
        limit: u32,
    },
}
#[derive(Clone, Debug)]
pub enum SyncResult {
    Cleaned { sessions: u32 },
    Binding(Binding),
    Head(ScopeHead),
    Receipt(Box<Receipt>),
    Pending(Vec<Uuid>),
}
#[async_trait::async_trait]
pub trait ShippingSyncUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        command: SyncCommand,
    ) -> Result<SyncResult, SyncError>;
}
pub struct ShippingSyncHandler<U, S, A, C> {
    uow: U,
    sync: S,
    access: A,
    application: C,
}
impl<U, S, A, C> ShippingSyncHandler<U, S, A, C> {
    pub fn new(uow: U, sync: S, access: A, application: C) -> Self {
        Self {
            uow,
            sync,
            access,
            application,
        }
    }
}
impl<U, S, A, C> ShippingSyncHandler<U, S, A, C>
where
    U: UnitOfWork,
    S: SyncStoreFactory<U::Tx>,
    A: SourceLocationAccessFactory<U::Tx>,
    C: SyncApplication<U::Tx>,
{
    async fn authorize(
        &self,
        tx: &mut U::Tx,
        context: &OperationContext,
        source: ListingSourceId,
        write: bool,
    ) -> Result<(), SyncError> {
        for capability in if write {
            [
                CredentialCapability::ListingSourcesWrite,
                CredentialCapability::ProductListingsWrite,
            ]
        } else {
            [
                CredentialCapability::ListingSourcesRead,
                CredentialCapability::ProductListingsRead,
            ]
        } {
            context
                .require_credential_capability(capability)
                .map_err(|_| shipping_service::ShippingError::SourceAccessDenied)?;
        }
        self.access
            .in_transaction(tx)
            .authorize_source(
                &context.principal,
                source,
                SourceLocationAccessPolicy::AdministratorOrOperator,
            )
            .await
            .map_err(shipping_service::ShippingError::from)?;
        Ok(())
    }
    async fn finalize(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: Uuid,
        manifest: Manifest,
    ) -> Result<SyncResult, SyncError> {
        if manifest.chunks == 0 || manifest.chunks > MAX_CHUNKS || manifest.records > MAX_RECORDS {
            return Err(SyncError::InvalidInput);
        }
        // First commit raw custody and immutable completion intent. Application can be retried
        // after a dependency failure or an ambiguous canonical commit without recapturing.
        let mut tx = self.uow.begin().await?;
        self.authorize(&mut tx, context, source, true).await?;
        let mut repo = self.sync.in_transaction(&mut tx);
        let receipt = repo.receipt(source, id).await?;
        repo.binding(source, receipt.begin.epoch).await?;
        if receipt.manifest.is_some_and(|m| m != manifest) {
            return Err(SyncError::Conflict);
        }
        if receipt.state == ApplicationState::Applied {
            drop(repo);
            tx.commit().await?;
            return Ok(SyncResult::Receipt(Box::new(receipt)));
        }
        if receipt.expires_at <= OffsetDateTime::now_utc() {
            repo.state(&receipt, ApplicationState::Expired, Some("SESSION_EXPIRED"))
                .await?;
        } else {
            let chunks = match repo.chunks(&receipt).await {
                Ok(chunks) => chunks,
                Err(SyncError::Incomplete) => Vec::new(),
                Err(error) => return Err(error),
            };
            let count: usize = chunks.iter().map(|c| c.records.len()).sum();
            let digest: [u8; 32] =
                Sha256::digest(chunks.iter().flat_map(|c| c.digest).collect::<Vec<_>>()).into();
            if chunks.len() != manifest.chunks as usize
                || count != manifest.records as usize
                || digest != manifest.digest
            {
                // Completion intent is immutable even while declared chunks are missing.
                repo.seal(&receipt, manifest, false).await?;
                drop(repo);
                tx.commit().await?;
                return Err(SyncError::Incomplete);
            }
            // Completeness cannot be advertised after failed/partial capability acquisition.
            if receipt.begin.mode == Mode::Snapshot
                && chunks
                    .iter()
                    .flat_map(|c| &c.records)
                .any(|r| matches!(r.assertion, Assertion::Unavailable { .. }) || matches!(&r.assertion, Assertion::Item { assertion, .. } if assertion.selection == EvidenceSelection::Unavailable))
            {
                repo.seal(&receipt, manifest,true).await?;
                repo.state(
                    &receipt,
                    ApplicationState::ReconcileRequired,
                    Some("CAPABILITY_UNAVAILABLE"),
                )
                .await?;
            } else {
                repo.seal(&receipt, manifest,true).await?;
            }
        }
        drop(repo);
        tx.commit().await?;
        let outcome = self.apply(context, source, id).await;
        match outcome {
            Ok(()) => {}
            Err(SyncError::Dependency(code)) => {
                self.record_failure(
                    context,
                    source,
                    id,
                    ApplicationState::WaitingDependency,
                    code,
                )
                .await?
            }
            Err(SyncError::Dirty) => {
                self.record_failure(
                    context,
                    source,
                    id,
                    ApplicationState::ReconcileRequired,
                    "CONCURRENT_CHANGE",
                )
                .await?
            }
            Err(SyncError::Listing(product_listing_service::use_cases::ProductListingDeliveryError::ReferenceIneligible)) => {
                self.record_failure(context,source,id,ApplicationState::WaitingDependency,"PROFILE_OR_LOCATION_INELIGIBLE").await?
            }
            Err(SyncError::Listing(product_listing_service::use_cases::ProductListingDeliveryError::IntegrationMismatch)) => {
                self.record_failure(context,source,id,ApplicationState::WaitingDependency,"INTEGRATION_CONFLICT").await?
            }
            Err(error) => return Err(error),
        }
        // Returning a command's receipt uses the command's write authorization.
        // An independent status query still requires its read capabilities.
        let mut tx = self.uow.begin().await?;
        self.authorize(&mut tx, context, source, true).await?;
        let mut repo = self.sync.in_transaction(&mut tx);
        let receipt = repo.receipt(source, id).await?;
        repo.binding(source, receipt.begin.epoch).await?;
        drop(repo);
        tx.commit().await?;
        Ok(SyncResult::Receipt(Box::new(receipt)))
    }
    async fn record_failure(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: Uuid,
        state: ApplicationState,
        code: &'static str,
    ) -> Result<(), SyncError> {
        let mut tx = self.uow.begin().await?;
        self.authorize(&mut tx, context, source, true).await?;
        let mut repo = self.sync.in_transaction(&mut tx);
        let receipt = repo.receipt(source, id).await?;
        repo.binding(source, receipt.begin.epoch).await?;
        if receipt.state != ApplicationState::Applied {
            repo.state(&receipt, state, Some(code)).await?;
        }
        drop(repo);
        tx.commit().await?;
        Ok(())
    }
    async fn apply(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: Uuid,
    ) -> Result<(), SyncError> {
        let mut tx = self.uow.begin().await?;
        self.authorize(&mut tx, context, source, true).await?;
        let mut repo = self.sync.in_transaction(&mut tx);
        let receipt = repo.receipt(source, id).await?;
        let binding = repo.binding(source, receipt.begin.epoch).await?;
        if matches!(
            receipt.state,
            ApplicationState::Applied
                | ApplicationState::Expired
                | ApplicationState::ReconcileRequired
        ) {
            drop(repo);
            tx.commit().await?;
            return Ok(());
        }
        let head = repo.head(&binding, &receipt.begin.scope).await?;
        let expected_sequence =
            receipt.begin.expected_sequence + i64::from(receipt.begin.mode == Mode::Delta);
        if receipt.begin.expected_generation != head.generation
            || expected_sequence != head.sequence
        {
            return Err(SyncError::Dirty);
        }
        let records = repo
            .chunks(&receipt)
            .await?
            .into_iter()
            .flat_map(|c| c.records)
            .collect::<Vec<_>>();
        for (i, record) in records.iter().enumerate() {
            if let Some(kind) = record.kind() {
                let index = match kind {
                    ObjectKind::Profile => 0,
                    ObjectKind::Default => 1,
                    ObjectKind::Item => 2,
                };
                if !receipt.begin.components[index] {
                    return Err(SyncError::InvalidInput);
                }
            }
            if record.window.observed_at < receipt.started_at
                || record.window.validate(OffsetDateTime::now_utc()).is_err()
            {
                return Err(SyncError::Dependency("EVIDENCE_EXPIRED_OR_PREDATES_FETCH"));
            }
            if records[..i]
                .iter()
                .any(|r| r.key == record.key && r.kind() == record.kind())
            {
                return Err(SyncError::Conflict);
            }
        }
        drop(repo);
        let configuration_changed = self
            .application
            .apply(&mut tx, &binding, &receipt, &records)
            .await?;
        self.sync
            .in_transaction(&mut tx)
            .activate(&receipt, configuration_changed)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
#[async_trait::async_trait]
impl<U, S, A, C> ShippingSyncUseCase for ShippingSyncHandler<U, S, A, C>
where
    U: UnitOfWork,
    S: SyncStoreFactory<U::Tx>,
    A: SourceLocationAccessFactory<U::Tx>,
    C: SyncApplication<U::Tx>,
{
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        command: SyncCommand,
    ) -> Result<SyncResult, SyncError> {
        if let SyncCommand::Finalize { id, manifest } = command {
            return self.finalize(context, source, id, manifest).await;
        }
        let write = !matches!(
            command,
            SyncCommand::Current { .. }
                | SyncCommand::Status { .. }
                | SyncCommand::Head { .. }
                | SyncCommand::Pending { .. }
        );
        let mut tx = self.uow.begin().await?;
        self.authorize(&mut tx, context, source, write).await?;
        let mut repo = self.sync.in_transaction(&mut tx);
        let result = match command {
            SyncCommand::Cleanup { limit } => {
                if !(1..=32).contains(&limit) {
                    return Err(SyncError::InvalidInput);
                }
                SyncResult::Cleaned {
                    sessions: repo.cleanup(source, limit).await?,
                }
            }
            SyncCommand::Rotate {
                integration,
                expected_revision,
            } => {
                if expected_revision < 0 {
                    return Err(SyncError::InvalidInput);
                }
                SyncResult::Binding(repo.rotate(source, &integration, expected_revision).await?)
            }
            SyncCommand::Current { integration } => {
                SyncResult::Binding(repo.current(source, &integration).await?)
            }
            SyncCommand::Revoke { epoch } => {
                let mut binding = repo.binding(source, epoch).await?;
                repo.revoke(&binding).await?;
                binding.active = false;
                binding.revision += 1;
                SyncResult::Binding(binding)
            }
            SyncCommand::Head { epoch, scope } => {
                let binding = repo.binding(source, epoch).await?;
                SyncResult::Head(repo.head(&binding, &scope).await?)
            }
            SyncCommand::Begin(request) => {
                if request.expected_generation < 0
                    || request.expected_sequence < 0
                    || request.expected_sequence == i64::MAX
                    || !request.components.iter().any(|c| *c)
                    || ![&request.key, &request.scope, &request.provider_version]
                        .iter()
                        .all(|k| k.as_str().bytes().all(|b| b.is_ascii_graphic()))
                    || request.key.as_str().len() > 128
                    || request.scope.as_str().len() > 128
                    || request.provider_version.as_str().len() > 32
                {
                    return Err(SyncError::InvalidInput);
                }
                let binding = repo.binding(source, request.epoch).await?;
                SyncResult::Receipt(Box::new(repo.begin(&binding, &request).await?))
            }
            SyncCommand::Capture { id, index, chunk } => {
                if index >= MAX_CHUNKS || chunk.records.len() > MAX_CHUNK_RECORDS {
                    return Err(SyncError::InvalidInput);
                }
                let receipt = repo.receipt(source, id).await?;
                repo.binding(source, receipt.begin.epoch).await?;
                if receipt.expires_at <= OffsetDateTime::now_utc() {
                    return Err(SyncError::Conflict);
                }
                // Matching custody retries do not acquire a new assertion. The store
                // still compares immutable content before acknowledging the replay.
                if !receipt.chunk_digests.iter().any(|(i, _)| *i == index) {
                    for r in &chunk.records {
                        r.window.validate(OffsetDateTime::now_utc())?;
                    }
                }
                repo.capture(&receipt, index, &chunk).await?;
                SyncResult::Receipt(Box::new(repo.receipt(source, id).await?))
            }
            SyncCommand::Status { id } => {
                let receipt = repo.receipt(source, id).await?;
                repo.binding(source, receipt.begin.epoch).await?;
                SyncResult::Receipt(Box::new(receipt))
            }
            SyncCommand::Pending {
                epoch,
                after,
                limit,
            } => {
                if !(1..=32).contains(&limit) {
                    return Err(SyncError::InvalidInput);
                }
                let binding = repo.binding(source, epoch).await?;
                SyncResult::Pending(repo.pending(&binding, after, limit).await?)
            }
            SyncCommand::Finalize { .. } => unreachable!(),
        };
        drop(repo);
        tx.commit().await?;
        Ok(result)
    }
}
