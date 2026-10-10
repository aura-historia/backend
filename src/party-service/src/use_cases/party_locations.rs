use crate::ports::party_location::*;
use application::{
    error::BoxError,
    operation_context::{
        CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
    },
    patch_field::PatchField,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use geo::{GeographicDescription, SpatialPosition};
use party_core::{
    party_id::PartyId,
    party_location::{
        PartyLocation, PartyLocationContent, PartyLocationDisclosure, PartyLocationLabel,
        PartyLocationLifecycle, PartyLocationRole,
    },
    party_location_id::PartyLocationId,
};
use std::collections::BTreeSet;
use time::OffsetDateTime;
use user_core::user_id::UserId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminUseCase,
};

/// Opaque caller evidence; never fetched. Caller identity and trust are server-owned.
#[derive(Clone, PartialEq, Eq)]
pub struct LocationEvidence {
    reference: String,
    observed_at: Option<OffsetDateTime>,
    scope: LocationAssertionScope,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationAssertionScope {
    SiteDescription,
    RegisteredAddress,
    CorrespondenceAddress,
}
impl LocationAssertionScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SiteDescription => "SITE_DESCRIPTION",
            Self::RegisteredAddress => "REGISTERED_ADDRESS",
            Self::CorrespondenceAddress => "CORRESPONDENCE_ADDRESS",
        }
    }
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "SITE_DESCRIPTION" => Some(Self::SiteDescription),
            "REGISTERED_ADDRESS" => Some(Self::RegisteredAddress),
            "CORRESPONDENCE_ADDRESS" => Some(Self::CorrespondenceAddress),
            _ => None,
        }
    }
}
impl LocationEvidence {
    pub fn new(
        reference: String,
        observed_at: Option<OffsetDateTime>,
        scope: LocationAssertionScope,
    ) -> Result<Self, PartyLocationError> {
        if reference.trim().is_empty()
            || reference.len() > 2048
            || reference
                .chars()
                .any(|ch| ch.is_control() || matches!(ch, '\u{2028}' | '\u{2029}'))
        {
            return Err(PartyLocationError::InvalidInput);
        }
        Ok(Self {
            reference,
            observed_at: observed_at.map(|value| value.to_offset(time::UtcOffset::UTC)),
            scope,
        })
    }
    pub fn reference(&self) -> &str {
        &self.reference
    }
    pub fn observed_at(&self) -> Option<OffsetDateTime> {
        self.observed_at
    }
    pub fn scope(&self) -> LocationAssertionScope {
        self.scope
    }
}
impl std::fmt::Debug for LocationEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LocationEvidence(<redacted>)")
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct PartyLocationIdempotencyKey(String);
impl PartyLocationIdempotencyKey {
    pub fn new(value: String) -> Result<Self, PartyLocationError> {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_.:".contains(&c))
        {
            return Err(PartyLocationError::InvalidInput);
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl std::fmt::Debug for PartyLocationIdempotencyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PartyLocationIdempotencyKey(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CreatePartyLocationCommand {
    pub party_id: PartyId,
    pub idempotency_key: PartyLocationIdempotencyKey,
    pub content: PartyLocationContent,
    pub evidence: Option<LocationEvidence>,
    /// Explicit relocation, never inferred from address equality or a label edit.
    pub relocates: Option<ExpectedPartyLocation>,
}
#[derive(Clone, PartialEq)]
pub struct UpdatePartyLocationCommand {
    pub party_id: PartyId,
    pub expected: ExpectedPartyLocation,
    pub label: PatchField<PartyLocationLabel>,
    pub roles: PatchField<BTreeSet<PartyLocationRole>>,
    pub geography: PatchField<GeographicDescription>,
    pub position: PatchField<SpatialPosition>,
    pub disclosure: PatchField<PartyLocationDisclosure>,
    pub evidence: PatchField<LocationEvidence>,
}
impl std::fmt::Debug for UpdatePartyLocationCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdatePartyLocationCommand")
            .field("party_id", &self.party_id)
            .field("expected", &self.expected)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct SetPartyLocationLifecycleCommand {
    pub party_id: PartyId,
    pub expected: ExpectedPartyLocation,
    pub lifecycle: PartyLocationLifecycle,
}
#[derive(Debug, Clone, PartialEq)]
pub struct GrantPartyLocationManagementCommand {
    pub party_id: PartyId,
    pub user_id: UserId,
    pub granted: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct GetPartyLocationRequest {
    pub party_id: PartyId,
    pub id: PartyLocationId,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ListPartyLocationsRequest {
    pub party_id: PartyId,
    pub after: Option<PartyLocationId>,
    pub limit: u32,
}

/// Protected management presentation, never an aggregate.
#[derive(Clone, PartialEq)]
pub struct PartyLocationView {
    pub id: PartyLocationId,
    pub party_id: PartyId,
    pub label: PartyLocationLabel,
    pub roles: BTreeSet<PartyLocationRole>,
    pub geography: Option<GeographicDescription>,
    pub position: Option<SpatialPosition>,
    pub disclosure: PartyLocationDisclosure,
    pub lifecycle: PartyLocationLifecycle,
    pub revision: PartyLocationRevision,
    pub input_revision: u64,
    pub evidence: Option<LocationEvidence>,
}
impl std::fmt::Debug for PartyLocationView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PartyLocationView")
            .field("id", &self.id)
            .field("party_id", &self.party_id)
            .field("disclosure", &self.disclosure)
            .field("lifecycle", &self.lifecycle)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}
impl From<StoredPartyLocation> for PartyLocationView {
    fn from(value: StoredPartyLocation) -> Self {
        let c = value.location.content();
        Self {
            id: value.location.id(),
            party_id: value.location.party_id(),
            label: c.label.clone(),
            roles: c.roles.clone(),
            geography: c.geography.clone(),
            position: c.position,
            disclosure: c.disclosure,
            lifecycle: value.location.lifecycle(),
            revision: value.revision,
            input_revision: value.location.input_revision().into_inner(),
            evidence: value.evidence,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct PartyLocationsPage {
    pub items: Vec<PartyLocationView>,
    pub next: Option<PartyLocationId>,
}

#[derive(Debug, thiserror::Error)]
pub enum PartyLocationError {
    #[error("authenticated actor required")]
    AuthenticationRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("party or location not found")]
    NotFound,
    #[error("concurrent location change")]
    ConcurrencyConflict,
    #[error("location revision exhausted")]
    RevisionExhausted,
    #[error("idempotency key reused with different command semantics")]
    IdempotencyConflict,
    #[error("invalid location input")]
    InvalidInput,
    #[error("invalid location transition")]
    Domain(
        #[from]
        #[source]
        party_core::party_location::PartyLocationError,
    ),
    #[error("temporary location persistence failure")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("invalid persisted location state")]
    InvalidPersistedState {
        #[source]
        source: BoxError,
    },
    #[error("internal location failure")]
    Internal {
        #[source]
        source: BoxError,
    },
    #[error("location transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
}

async fn privileged<A: CheckUserAdminUseCase>(
    context: &OperationContext,
    admin: &A,
    write: bool,
) -> Result<bool, PartyLocationError> {
    context
        .require_credential_capability(if write {
            CredentialCapability::PartiesWrite
        } else {
            CredentialCapability::PartiesRead
        })
        .map_err(|e| match e {
            CredentialAuthorizationError::AuthenticationRequired(_) => {
                PartyLocationError::AuthenticationRequired
            }
            CredentialAuthorizationError::InsufficientCapability { .. } => {
                PartyLocationError::Forbidden
            }
        })?;
    match context.principal {
        Principal::Service(_) | Principal::System => Ok(true),
        Principal::User(_) | Principal::DelegatedUser { .. } => {
            match admin.execute(context, CheckUserAdminRequest).await {
                Ok(_) => Ok(true),
                Err(CheckUserAdminError::Forbidden) => Ok(false),
                Err(CheckUserAdminError::AuthenticatedActorRequired) => {
                    Err(PartyLocationError::AuthenticationRequired)
                }
                Err(
                    CheckUserAdminError::TemporarilyUnavailable { source }
                    | CheckUserAdminError::BeginTransactionFailed(source)
                    | CheckUserAdminError::CommitTransactionFailed(source),
                ) => Err(PartyLocationError::TemporarilyUnavailable { source }),
                Err(
                    CheckUserAdminError::Internal { source }
                    | CheckUserAdminError::InvalidReadModel { source },
                ) => Err(PartyLocationError::Internal { source }),
            }
        }
        Principal::Anonymous => Err(PartyLocationError::AuthenticationRequired),
    }
}
async fn authorize<Tx, F: PartyLocationAccessFactory<Tx>>(
    context: &OperationContext,
    privileged: bool,
    access: &F,
    tx: &mut Tx,
    party_id: PartyId,
) -> Result<(), PartyLocationError> {
    if !access.in_transaction(tx).lock_party(party_id).await? {
        return Err(PartyLocationError::NotFound);
    }
    if privileged {
        return Ok(());
    }
    let user = match context.principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => id,
        _ => return Err(PartyLocationError::Forbidden),
    };
    if access
        .in_transaction(tx)
        .can_manage_locations(party_id, user)
        .await?
    {
        Ok(())
    } else {
        Err(PartyLocationError::Forbidden)
    }
}
fn actor(context: &OperationContext) -> String {
    format!("{}:{}", context.principal.kind(), context.principal.label())
}
fn required<T>(current: T, patch: PatchField<T>) -> Result<T, PartyLocationError> {
    match patch {
        PatchField::Unchanged => Ok(current),
        PatchField::Set(v) => Ok(v),
        PatchField::Clear => Err(PartyLocationError::InvalidInput),
    }
}
fn optional<T>(current: Option<T>, patch: PatchField<T>) -> Option<T> {
    match patch {
        PatchField::Unchanged => current,
        PatchField::Set(v) => Some(v),
        PatchField::Clear => None,
    }
}
async fn load<Tx, R: PartyLocationRepositoryFactory<Tx>>(
    r: &R,
    tx: &mut Tx,
    party_id: PartyId,
    expected: ExpectedPartyLocation,
) -> Result<StoredPartyLocation, PartyLocationError> {
    if expected.revision.into_inner() > i64::MAX as u64 {
        return Err(PartyLocationError::InvalidInput);
    }
    let stored = r
        .in_transaction(tx)
        .find_for_update(party_id, expected.id)
        .await?
        .ok_or(PartyLocationError::NotFound)?;
    if stored.revision != expected.revision {
        return Err(PartyLocationError::ConcurrencyConflict);
    }
    Ok(stored)
}

#[async_trait::async_trait]
pub trait CreatePartyLocationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreatePartyLocationCommand,
    ) -> Result<PartyLocationView, PartyLocationError>;
}
pub struct CreatePartyLocationHandler<U, R, F, A> {
    uow: U,
    locations: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> CreatePartyLocationHandler<U, R, F, A> {
    pub fn new(uow: U, locations: R, access: F, admin: A) -> Self {
        Self {
            uow,
            locations,
            access,
            admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, F, A> CreatePartyLocationUseCase for CreatePartyLocationHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationRepositoryFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id, party_id = %command.party_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreatePartyLocationCommand,
    ) -> Result<PartyLocationView, PartyLocationError> {
        let privileged = privileged(context, &self.admin, true).await?;
        let mut tx = self.uow.begin().await?;
        authorize(context, privileged, &self.access, &mut tx, command.party_id).await?;
        let actor = actor(context);
        let replay = self
            .locations
            .in_transaction(&mut tx)
            .replay_create(&actor, &command)
            .await?;
        if let Some(result) = replay {
            tx.commit().await?;
            return Ok(result.into());
        }
        if let Some(expected) = command.relocates {
            let mut old = load(&self.locations, &mut tx, command.party_id, expected).await?;
            if !old.location.eligible_for_assignment() {
                return Err(PartyLocationError::InvalidInput);
            }
            old.location.set_lifecycle(PartyLocationLifecycle::Retired);
            self.locations
                .in_transaction(&mut tx)
                .update(&old, &actor)
                .await?;
        }
        let location = PartyLocation::create(
            PartyLocationId::new(),
            command.party_id,
            command.content.clone(),
        );
        let result = self
            .locations
            .in_transaction(&mut tx)
            .insert(&location, command.evidence.as_ref(), &actor)
            .await?;
        self.locations
            .in_transaction(&mut tx)
            .record_create(&actor, &command, &result)
            .await?;
        tx.commit().await?;
        tracing::info!(event = "party_location.created", actor = %actor, location_id = %location.id(), outcome = "success");
        Ok(result.into())
    }
}

#[async_trait::async_trait]
pub trait UpdatePartyLocationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartyLocationCommand,
    ) -> Result<PartyLocationView, PartyLocationError>;
}
pub struct UpdatePartyLocationHandler<U, R, F, A> {
    uow: U,
    locations: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> UpdatePartyLocationHandler<U, R, F, A> {
    pub fn new(uow: U, locations: R, access: F, admin: A) -> Self {
        Self {
            uow,
            locations,
            access,
            admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, F, A> UpdatePartyLocationUseCase for UpdatePartyLocationHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationRepositoryFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id, party_id = %command.party_id, location_id = %command.expected.id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: UpdatePartyLocationCommand,
    ) -> Result<PartyLocationView, PartyLocationError> {
        let privileged = privileged(context, &self.admin, true).await?;
        let mut tx = self.uow.begin().await?;
        authorize(context, privileged, &self.access, &mut tx, command.party_id).await?;
        let mut stored = load(&self.locations, &mut tx, command.party_id, command.expected).await?;
        let c = stored.location.content().clone();
        let content = PartyLocationContent {
            label: required(c.label, command.label)?,
            roles: required(c.roles, command.roles)?,
            geography: optional(c.geography, command.geography),
            position: optional(c.position, command.position),
            disclosure: required(c.disclosure, command.disclosure)?,
        };
        let evidence = optional(stored.evidence.clone(), command.evidence);
        if stored
            .location
            .correct_same_site(content, evidence != stored.evidence)?
            .changed()
        {
            stored.evidence = evidence;
            stored = self
                .locations
                .in_transaction(&mut tx)
                .update(&stored, &actor(context))
                .await?;
        }
        tx.commit().await?;
        tracing::info!(event = "party_location.corrected", actor = %actor(context), location_id = %stored.location.id(), outcome = "success");
        Ok(stored.into())
    }
}

#[async_trait::async_trait]
pub trait SetPartyLocationLifecycleUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: SetPartyLocationLifecycleCommand,
    ) -> Result<PartyLocationView, PartyLocationError>;
}
pub struct SetPartyLocationLifecycleHandler<U, R, F, A> {
    uow: U,
    locations: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> SetPartyLocationLifecycleHandler<U, R, F, A> {
    pub fn new(uow: U, locations: R, access: F, admin: A) -> Self {
        Self {
            uow,
            locations,
            access,
            admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, F, A> SetPartyLocationLifecycleUseCase for SetPartyLocationLifecycleHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationRepositoryFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id, party_id = %command.party_id, location_id = %command.expected.id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: SetPartyLocationLifecycleCommand,
    ) -> Result<PartyLocationView, PartyLocationError> {
        let privileged = privileged(context, &self.admin, true).await?;
        let mut tx = self.uow.begin().await?;
        authorize(context, privileged, &self.access, &mut tx, command.party_id).await?;
        let mut stored = load(&self.locations, &mut tx, command.party_id, command.expected).await?;
        if stored.location.set_lifecycle(command.lifecycle).changed() {
            stored = self
                .locations
                .in_transaction(&mut tx)
                .update(&stored, &actor(context))
                .await?;
        }
        tx.commit().await?;
        tracing::info!(event = "party_location.lifecycle_changed", actor = %actor(context), location_id = %stored.location.id(), lifecycle = command.lifecycle.as_str(), outcome = "success");
        Ok(stored.into())
    }
}

#[async_trait::async_trait]
pub trait GrantPartyLocationManagementUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartyLocationManagementCommand,
    ) -> Result<(), PartyLocationError>;
}
pub struct GrantPartyLocationManagementHandler<U, F, A> {
    uow: U,
    access: F,
    admin: A,
}
impl<U, F, A> GrantPartyLocationManagementHandler<U, F, A> {
    pub fn new(uow: U, access: F, admin: A) -> Self {
        Self { uow, access, admin }
    }
}
#[async_trait::async_trait]
impl<U, F, A> GrantPartyLocationManagementUseCase for GrantPartyLocationManagementHandler<U, F, A>
where
    U: UnitOfWork,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: GrantPartyLocationManagementCommand,
    ) -> Result<(), PartyLocationError> {
        if !privileged(context, &self.admin, true).await? {
            return Err(PartyLocationError::Forbidden);
        }
        let mut tx = self.uow.begin().await?;
        authorize(context, true, &self.access, &mut tx, command.party_id).await?;
        self.access
            .in_transaction(&mut tx)
            .set_management_grant(
                command.party_id,
                command.user_id,
                command.granted,
                &actor(context),
            )
            .await?;
        tx.commit().await?;
        tracing::info!(event = "party_location.management_grant_changed", actor = %actor(context), party_id = %command.party_id, user_id = %command.user_id, granted = command.granted, outcome = "success");
        Ok(())
    }
}

#[async_trait::async_trait]
pub trait GetPartyLocationUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartyLocationRequest,
    ) -> Result<PartyLocationView, PartyLocationError>;
}
pub struct GetPartyLocationHandler<U, R, F, A> {
    uow: U,
    reader: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> GetPartyLocationHandler<U, R, F, A> {
    pub fn new(uow: U, reader: R, access: F, admin: A) -> Self {
        Self {
            uow,
            reader,
            access,
            admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, F, A> GetPartyLocationUseCase for GetPartyLocationHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationReaderFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPartyLocationRequest,
    ) -> Result<PartyLocationView, PartyLocationError> {
        let privileged = privileged(context, &self.admin, false).await?;
        let mut tx = self.uow.begin().await?;
        authorize(context, privileged, &self.access, &mut tx, request.party_id).await?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .get(request.party_id, request.id)
            .await?
            .ok_or(PartyLocationError::NotFound)?;
        tx.commit().await?;
        Ok(result)
    }
}
#[async_trait::async_trait]
pub trait ListPartyLocationsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartyLocationsRequest,
    ) -> Result<PartyLocationsPage, PartyLocationError>;
}
pub struct ListPartyLocationsHandler<U, R, F, A> {
    uow: U,
    reader: R,
    access: F,
    admin: A,
}
impl<U, R, F, A> ListPartyLocationsHandler<U, R, F, A> {
    pub fn new(uow: U, reader: R, access: F, admin: A) -> Self {
        Self {
            uow,
            reader,
            access,
            admin,
        }
    }
}
#[async_trait::async_trait]
impl<U, R, F, A> ListPartyLocationsUseCase for ListPartyLocationsHandler<U, R, F, A>
where
    U: UnitOfWork,
    R: PartyLocationReaderFactory<U::Tx>,
    F: PartyLocationAccessFactory<U::Tx>,
    A: CheckUserAdminUseCase,
{
    #[tracing::instrument(skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: ListPartyLocationsRequest,
    ) -> Result<PartyLocationsPage, PartyLocationError> {
        if !(1..=100).contains(&request.limit) {
            return Err(PartyLocationError::InvalidInput);
        }
        let privileged = privileged(context, &self.admin, false).await?;
        let mut tx = self.uow.begin().await?;
        authorize(context, privileged, &self.access, &mut tx, request.party_id).await?;
        let result = self
            .reader
            .in_transaction(&mut tx)
            .list(request.party_id, request.after, request.limit)
            .await?;
        tx.commit().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_rejects_multiline_references_and_normalizes_observation_offsets() {
        for reference in [" ", "x\nx", "x\u{2028}x", "x\u{2029}x", &"é".repeat(1025)] {
            assert!(
                LocationEvidence::new(
                    reference.to_owned(),
                    None,
                    LocationAssertionScope::SiteDescription,
                )
                .is_err()
            );
        }
        let observed =
            OffsetDateTime::UNIX_EPOCH.to_offset(time::UtcOffset::from_hms(2, 0, 0).unwrap());
        let evidence = LocationEvidence::new(
            "reference-secret".to_owned(),
            Some(observed),
            LocationAssertionScope::SiteDescription,
        )
        .unwrap();
        assert_eq!(Some(OffsetDateTime::UNIX_EPOCH), evidence.observed_at());
        assert_eq!(
            time::UtcOffset::UTC,
            evidence.observed_at().unwrap().offset()
        );
        assert!(!format!("{evidence:?}").contains("reference-secret"));
    }

    #[test]
    fn debug_output_redacts_private_geography_and_coordinates() {
        let geography = GeographicDescription::new(
            Some(geo::AddressText::new("Secret Road".to_owned()).unwrap()),
            None,
            None,
            None,
        )
        .unwrap()
        .unwrap();
        let position = SpatialPosition::new(
            geo::GeoPoint::new(52.5, 13.4).unwrap(),
            geo::SpatialPrecision::Premises,
            None,
        )
        .unwrap();
        let view = PartyLocationView {
            id: PartyLocationId::new(),
            party_id: PartyId::new(),
            label: PartyLocationLabel::new("Secret showroom".to_owned()).unwrap(),
            roles: BTreeSet::new(),
            geography: Some(geography.clone()),
            position: Some(position),
            disclosure: PartyLocationDisclosure::Private,
            lifecycle: PartyLocationLifecycle::Active,
            revision: PartyLocationRevision::INITIAL,
            input_revision: 1,
            evidence: None,
        };
        let command = UpdatePartyLocationCommand {
            party_id: view.party_id,
            expected: ExpectedPartyLocation {
                id: view.id,
                revision: PartyLocationRevision::INITIAL,
            },
            label: PatchField::Unchanged,
            roles: PatchField::Unchanged,
            geography: PatchField::Set(geography),
            position: PatchField::Set(position),
            disclosure: PatchField::Unchanged,
            evidence: PatchField::Unchanged,
        };
        for debug in [format!("{view:?}"), format!("{command:?}")] {
            assert!(debug.contains(&view.id.to_string()));
            for private in ["Secret Road", "Secret showroom", "52.5", "13.4"] {
                assert!(!debug.contains(private));
            }
        }
    }
}
