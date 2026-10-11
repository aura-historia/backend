use crate::ports::location_default::*;
use application::{
    error::BoxError,
    operation_context::{CredentialAuthorizationError, CredentialCapability, OperationContext},
    patch_field::PatchField,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use listing_source_core::{ListingSourceId, location_default::*};
use party_core::party_location_id::PartyLocationId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedLocationEligibility {
    Active,
    Retired,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocationDefaultView {
    pub source_id: ListingSourceId,
    pub revision: LocationDefaultRevision,
    pub assignment: Option<SourceLocationAssignment>,
    pub eligibility: Option<SelectedLocationEligibility>,
}
impl StoredLocationDefault {
    fn into_view(self, eligibility: Option<SelectedLocationEligibility>) -> LocationDefaultView {
        LocationDefaultView {
            source_id: self.default.source_id(),
            revision: self.revision,
            assignment: self.default.assignment().cloned(),
            eligibility,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssignLocationDefaultCommand {
    pub source_id: ListingSourceId,
    pub expected_revision: LocationDefaultRevision,
    pub assignment: SourceLocationAssignment,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearLocationDefaultCommand {
    pub source_id: ListingSourceId,
    pub expected_revision: LocationDefaultRevision,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeLocationDefaultApplicabilityCommand {
    pub source_id: ListingSourceId,
    pub expected_revision: LocationDefaultRevision,
    pub business_association: PatchField<SourceLocationEvidence>,
    pub advertised_inventory: PatchField<SourceLocationEvidence>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GetLocationDefaultRequest {
    pub source_id: ListingSourceId,
}

/// Protected dependency basis. Never expose evidence or revision tokens in public geography.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceDefaultGeographyBasis {
    pub source_id: ListingSourceId,
    pub assignment_revision: LocationDefaultRevision,
    pub location_id: PartyLocationId,
    pub location_revision: u64,
    pub location_input_revision: u64,
    pub meaning: SourceLocationMeaning,
}
impl SourceDefaultGeographyBasis {
    pub fn kind(&self) -> &'static str {
        "SOURCE_DEFAULT_DERIVED"
    }
}
/// Describes the selected location even when no description/point has been resolved yet.
#[derive(Clone, PartialEq)]
pub struct SourceDefaultGeographyView {
    pub basis: SourceDefaultGeographyBasis,
    pub geography: Option<geo::DerivedGeography>,
}
impl std::fmt::Debug for SourceDefaultGeographyView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceDefaultGeographyView(<redacted>)")
    }
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EffectiveSourceGeographyView {
    pub business_association: Option<SourceDefaultGeographyView>,
    pub advertised_inventory: Option<SourceDefaultGeographyView>,
}

#[derive(Debug, thiserror::Error)]
pub enum LocationDefaultError {
    #[error("authenticated actor required")]
    AuthenticationRequired,
    #[error("source location operation not permitted")]
    Forbidden,
    #[error("listing source not found")]
    SourceNotFound,
    #[error("location does not belong to source operator or does not exist")]
    LocationNotFound,
    #[error("retired location cannot be assigned or have applicability changed")]
    LocationRetired,
    #[error("no source location default to change")]
    DefaultNotAssigned,
    #[error("stale source location default revision")]
    ConcurrencyConflict,
    #[error("source location default revision exhausted")]
    RevisionExhausted,
    #[error("invalid expected revision")]
    InvalidRevision,
    #[error("invalid source location assignment")]
    InvalidAssignment(
        #[from]
        #[source]
        InvalidSourceLocationAssignment,
    ),
    #[error("temporary source location persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted source location state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal source location failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("source location transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
}
fn capability(context: &OperationContext, write: bool) -> Result<(), LocationDefaultError> {
    context
        .require_credential_capability(if write {
            CredentialCapability::ListingSourcesWrite
        } else {
            CredentialCapability::ListingSourcesRead
        })
        .map(|_| ())
        .map_err(|error| match error {
            CredentialAuthorizationError::AuthenticationRequired(_) => {
                LocationDefaultError::AuthenticationRequired
            }
            CredentialAuthorizationError::InsufficientCapability { .. } => {
                LocationDefaultError::Forbidden
            }
        })
}
fn validate_revision(revision: LocationDefaultRevision) -> Result<(), LocationDefaultError> {
    if revision.into_inner() > i64::MAX as u64 {
        return Err(LocationDefaultError::InvalidRevision);
    }
    Ok(())
}
fn expected(
    stored: &StoredLocationDefault,
    revision: LocationDefaultRevision,
) -> Result<(), LocationDefaultError> {
    if stored.revision != revision {
        return Err(LocationDefaultError::ConcurrencyConflict);
    }
    Ok(())
}
fn patch<T: Clone>(current: Option<&T>, value: PatchField<T>) -> Option<T> {
    match value {
        PatchField::Unchanged => current.cloned(),
        PatchField::Set(v) => Some(v),
        PatchField::Clear => None,
    }
}
async fn persist<Tx: Transaction, R: LocationDefaultRepositoryFactory<Tx>>(
    locations: &R,
    tx: &mut Tx,
    stored: StoredLocationDefault,
    replacement: Option<SourceLocationAssignment>,
    context: &OperationContext,
) -> Result<StoredLocationDefault, LocationDefaultError> {
    let mut stored = stored;
    if stored.default.replace(replacement).changed() {
        let actor = format!("{}:{}", context.principal.kind(), context.principal.label());
        stored = locations.in_transaction(tx).update(&stored, &actor).await?;
    }
    Ok(stored)
}

#[async_trait::async_trait]
pub trait AssignLocationDefaultUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AssignLocationDefaultCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError>;
}
pub struct AssignLocationDefaultHandler<U, R, A> {
    uow: U,
    locations: R,
    access: A,
    policy: SourceLocationAccessPolicy,
}
impl<U, R, A> AssignLocationDefaultHandler<U, R, A> {
    pub fn new(uow: U, locations: R, access: A) -> Self {
        Self {
            uow,
            locations,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOrOperator,
        }
    }

    pub fn new_admin_only(uow: U, locations: R, access: A) -> Self {
        Self {
            uow,
            locations,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOnly,
        }
    }
}
#[async_trait::async_trait]
impl<
    U: UnitOfWork,
    R: LocationDefaultRepositoryFactory<U::Tx>,
    A: SourceLocationAccessFactory<U::Tx>,
> AssignLocationDefaultUseCase for AssignLocationDefaultHandler<U, R, A>
{
    #[tracing::instrument(skip_all, fields(source_id = %command.source_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: AssignLocationDefaultCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        capability(context, true)?;
        validate_revision(command.expected_revision)?;
        let mut tx = self.uow.begin().await?;
        let party_id = self
            .access
            .in_transaction(&mut tx)
            .authorize_source(&context.principal, command.source_id, self.policy)
            .await?;
        let stored = self
            .locations
            .in_transaction(&mut tx)
            .find_for_update(command.source_id)
            .await?;
        expected(&stored, command.expected_revision)?;
        let eligibility = self
            .access
            .in_transaction(&mut tx)
            .check_location(party_id, command.assignment.location_id())
            .await?;
        if eligibility != SelectedLocationEligibility::Active {
            return Err(LocationDefaultError::LocationRetired);
        }
        let result = persist(
            &self.locations,
            &mut tx,
            stored,
            Some(command.assignment),
            context,
        )
        .await?;
        tx.commit().await?;
        tracing::info!(event = "listing_source.location_default_assigned", actor_type = context.principal.kind(), actor_id = %context.principal.label(), revision = result.revision.into_inner(), changed = result.revision != command.expected_revision, outcome = "success");
        Ok(result.into_view(Some(eligibility)))
    }
}

#[async_trait::async_trait]
pub trait ClearLocationDefaultUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ClearLocationDefaultCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError>;
}
pub struct ClearLocationDefaultHandler<U, R, A> {
    uow: U,
    locations: R,
    access: A,
    policy: SourceLocationAccessPolicy,
}
impl<U, R, A> ClearLocationDefaultHandler<U, R, A> {
    pub fn new(uow: U, locations: R, access: A) -> Self {
        Self {
            uow,
            locations,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOrOperator,
        }
    }

    pub fn new_admin_only(uow: U, locations: R, access: A) -> Self {
        Self {
            uow,
            locations,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOnly,
        }
    }
}
#[async_trait::async_trait]
impl<
    U: UnitOfWork,
    R: LocationDefaultRepositoryFactory<U::Tx>,
    A: SourceLocationAccessFactory<U::Tx>,
> ClearLocationDefaultUseCase for ClearLocationDefaultHandler<U, R, A>
{
    #[tracing::instrument(skip_all, fields(source_id = %command.source_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ClearLocationDefaultCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        capability(context, true)?;
        validate_revision(command.expected_revision)?;
        let mut tx = self.uow.begin().await?;
        self.access
            .in_transaction(&mut tx)
            .authorize_source(&context.principal, command.source_id, self.policy)
            .await?;
        let stored = self
            .locations
            .in_transaction(&mut tx)
            .find_for_update(command.source_id)
            .await?;
        expected(&stored, command.expected_revision)?;
        let result = persist(&self.locations, &mut tx, stored, None, context).await?;
        tx.commit().await?;
        tracing::info!(event = "listing_source.location_default_cleared", actor_type = context.principal.kind(), actor_id = %context.principal.label(), revision = result.revision.into_inner(), changed = result.revision != command.expected_revision, outcome = "success");
        Ok(result.into_view(None))
    }
}

#[async_trait::async_trait]
pub trait ChangeLocationDefaultApplicabilityUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeLocationDefaultApplicabilityCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError>;
}
pub struct ChangeLocationDefaultApplicabilityHandler<U, R, A> {
    uow: U,
    locations: R,
    access: A,
    policy: SourceLocationAccessPolicy,
}
impl<U, R, A> ChangeLocationDefaultApplicabilityHandler<U, R, A> {
    pub fn new(uow: U, locations: R, access: A) -> Self {
        Self {
            uow,
            locations,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOrOperator,
        }
    }

    pub fn new_admin_only(uow: U, locations: R, access: A) -> Self {
        Self {
            uow,
            locations,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOnly,
        }
    }
}
#[async_trait::async_trait]
impl<
    U: UnitOfWork,
    R: LocationDefaultRepositoryFactory<U::Tx>,
    A: SourceLocationAccessFactory<U::Tx>,
> ChangeLocationDefaultApplicabilityUseCase for ChangeLocationDefaultApplicabilityHandler<U, R, A>
{
    #[tracing::instrument(skip_all, fields(source_id = %command.source_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ChangeLocationDefaultApplicabilityCommand,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        capability(context, true)?;
        validate_revision(command.expected_revision)?;
        let mut tx = self.uow.begin().await?;
        let party_id = self
            .access
            .in_transaction(&mut tx)
            .authorize_source(&context.principal, command.source_id, self.policy)
            .await?;
        let stored = self
            .locations
            .in_transaction(&mut tx)
            .find_for_update(command.source_id)
            .await?;
        expected(&stored, command.expected_revision)?;
        let old = stored
            .default
            .assignment()
            .ok_or(LocationDefaultError::DefaultNotAssigned)?;
        let eligibility = self
            .access
            .in_transaction(&mut tx)
            .check_location(party_id, old.location_id())
            .await?;
        if eligibility != SelectedLocationEligibility::Active {
            return Err(LocationDefaultError::LocationRetired);
        }
        let replacement = SourceLocationAssignment::new(
            old.location_id(),
            patch(old.business_association(), command.business_association),
            patch(old.advertised_inventory(), command.advertised_inventory),
        )?;
        let result = persist(&self.locations, &mut tx, stored, Some(replacement), context).await?;
        tx.commit().await?;
        tracing::info!(event = "listing_source.location_default_applicability_changed", actor_type = context.principal.kind(), actor_id = %context.principal.label(), revision = result.revision.into_inner(), changed = result.revision != command.expected_revision, outcome = "success");
        Ok(result.into_view(Some(eligibility)))
    }
}

#[async_trait::async_trait]
pub trait GetLocationDefaultUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetLocationDefaultRequest,
    ) -> Result<LocationDefaultView, LocationDefaultError>;
}
pub struct GetLocationDefaultHandler<U, R, A> {
    uow: U,
    reader: R,
    access: A,
    policy: SourceLocationAccessPolicy,
}
impl<U, R, A> GetLocationDefaultHandler<U, R, A> {
    pub fn new(uow: U, reader: R, access: A) -> Self {
        Self {
            uow,
            reader,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOrOperator,
        }
    }

    pub fn new_admin_only(uow: U, reader: R, access: A) -> Self {
        Self {
            uow,
            reader,
            access,
            policy: SourceLocationAccessPolicy::AdministratorOnly,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: LocationDefaultReaderFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    GetLocationDefaultUseCase for GetLocationDefaultHandler<U, R, A>
{
    #[tracing::instrument(skip_all, fields(source_id = %request.source_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetLocationDefaultRequest,
    ) -> Result<LocationDefaultView, LocationDefaultError> {
        capability(context, false)?;
        let mut tx = self.uow.begin().await?;
        self.access
            .in_transaction(&mut tx)
            .authorize_source(&context.principal, request.source_id, self.policy)
            .await?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .get(request.source_id)
            .await?;
        tx.commit().await?;
        Ok(result)
    }
}
