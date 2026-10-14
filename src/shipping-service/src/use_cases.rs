use crate::{
    ShippingError,
    ports::{
        ShippingDefaultRevision, ShippingEvidence, ShippingManagementRevision, ShippingReader,
        ShippingReaderFactory, ShippingRepository, ShippingRepositoryFactory, StoredDefault,
        StoredProfile,
    },
};
use application::{
    operation_context::{
        CredentialAuthorizationError, CredentialCapability, OperationContext, Principal,
    },
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use listing_source_service::{
    ports::location_default::{
        SourceLocationAccess, SourceLocationAccessFactory, SourceLocationAccessPolicy,
    },
    use_cases::location_default::SelectedLocationEligibility,
};
use party_core::party_id::PartyId;
use shipping_core::{
    profile::{
        ShippingProfile, ShippingProfileContent, ShippingProfileError, ShippingProfileId,
        ShippingProfileLifecycle, ShippingProfileName, ShippingProfileRevision,
    },
    terms::{DeliverySite, ShippingPolicy},
};

#[derive(Debug, Clone, PartialEq)]
pub struct ProfileView {
    pub id: ShippingProfileId,
    pub source_id: ListingSourceId,
    pub management_revision: ShippingManagementRevision,
    pub revision: ShippingProfileRevision,
    pub lifecycle: ShippingProfileLifecycle,
    pub content: ShippingProfileContent,
}
impl From<StoredProfile> for ProfileView {
    fn from(value: StoredProfile) -> Self {
        Self {
            id: value.profile.id(),
            source_id: value.profile.source_id(),
            management_revision: value.management_revision,
            revision: value.profile.revision(),
            lifecycle: value.profile.lifecycle(),
            content: value.profile.content().clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct ProfileSummary {
    pub id: ShippingProfileId,
    pub source_id: ListingSourceId,
    pub name: ShippingProfileName,
    pub management_revision: ShippingManagementRevision,
    pub revision: ShippingProfileRevision,
    pub lifecycle: ShippingProfileLifecycle,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultEligibility {
    Active,
    Retired,
    Missing,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultView {
    pub source_id: ListingSourceId,
    pub revision: ShippingDefaultRevision,
    pub profile_id: Option<ShippingProfileId>,
    pub eligibility: Option<DefaultEligibility>,
}
#[derive(Debug, Clone)]
pub struct CreateProfileCommand {
    pub source_id: ListingSourceId,
    pub creation_key: String,
    pub content: ShippingProfileContent,
    pub evidence: Vec<ShippingEvidence>,
}
#[derive(Debug, Clone)]
pub struct ReplaceProfileCommand {
    pub source_id: ListingSourceId,
    pub profile_id: ShippingProfileId,
    pub expected_revision: ShippingManagementRevision,
    pub content: ShippingProfileContent,
    pub evidence: Vec<ShippingEvidence>,
}
#[derive(Debug, Clone, Copy)]
pub struct ProfileLifecycleCommand {
    pub source_id: ListingSourceId,
    pub profile_id: ShippingProfileId,
    pub expected_revision: ShippingManagementRevision,
}
#[derive(Debug, Clone, Copy)]
pub struct AssignDefaultCommand {
    pub source_id: ListingSourceId,
    pub profile_id: ShippingProfileId,
    pub expected_revision: ShippingDefaultRevision,
}
#[derive(Debug, Clone, Copy)]
pub struct ClearDefaultCommand {
    pub source_id: ListingSourceId,
    pub expected_revision: ShippingDefaultRevision,
}

fn capability(context: &OperationContext, write: bool) -> Result<(), ShippingError> {
    context
        .require_credential_capability(if write {
            CredentialCapability::ListingSourcesWrite
        } else {
            CredentialCapability::ListingSourcesRead
        })
        .map(|_| ())
        .map_err(|e| match e {
            CredentialAuthorizationError::AuthenticationRequired(_) => {
                ShippingError::AuthenticationRequired
            }
            CredentialAuthorizationError::InsufficientCapability { .. } => {
                ShippingError::SourceAccessDenied
            }
        })
}
fn validate_revision(value: u64) -> Result<(), ShippingError> {
    if value > i64::MAX as u64 {
        Err(ShippingError::InvalidRevision)
    } else {
        Ok(())
    }
}
fn validate_evidence(evidence: &[ShippingEvidence]) -> Result<(), ShippingError> {
    if evidence.len() > 32
        || evidence
            .iter()
            .enumerate()
            .any(|(i, e)| evidence[..i].contains(e))
    {
        return Err(ShippingError::InvalidInput);
    }
    Ok(())
}
fn caller(context: &OperationContext) -> String {
    let kind = match context.principal {
        Principal::User(_) | Principal::DelegatedUser { .. } => "user",
        _ => context.principal.kind(),
    };
    format!("{kind}:{}", context.principal.label())
}
fn profile_expected(
    stored: &StoredProfile,
    revision: ShippingManagementRevision,
) -> Result<(), ShippingError> {
    if stored.management_revision != revision {
        Err(ShippingError::StaleRevision)
    } else {
        Ok(())
    }
}
fn default_expected(
    stored: &StoredDefault,
    revision: ShippingDefaultRevision,
) -> Result<(), ShippingError> {
    if stored.revision != revision {
        Err(ShippingError::StaleRevision)
    } else {
        Ok(())
    }
}
async fn validate_sites(
    access: &mut impl SourceLocationAccess,
    owner: PartyId,
    profile: &ShippingProfile,
) -> Result<(), ShippingError> {
    let terms = &profile.content().terms;
    let origin = match terms.shipping() {
        Some(ShippingPolicy::Coverage(c)) => c.dispatch_origin(),
        _ => None,
    };
    let collection = terms.collection().and_then(|c| c.site());
    // These explicit terms assert dispatch/collection applicability. Party roles confer none.
    for site in origin.into_iter().chain(collection) {
        if let DeliverySite::PartyLocation(id) = site
            && access.check_location(owner, *id).await? != SelectedLocationEligibility::Active
        {
            return Err(ShippingError::LocationRetired);
        }
    }
    Ok(())
}
async fn authorize<Tx, A: SourceLocationAccessFactory<Tx>>(
    access: &A,
    tx: &mut Tx,
    context: &OperationContext,
    source: ListingSourceId,
) -> Result<PartyId, ShippingError> {
    access
        .in_transaction(tx)
        .authorize_source(
            &context.principal,
            source,
            SourceLocationAccessPolicy::AdministratorOrOperator,
        )
        .await
        .map_err(Into::into)
}
struct ShippingWriteHandler<U, R, A> {
    uow: U,
    repositories: R,
    access: A,
}
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    ShippingWriteHandler<U, R, A>
{
    async fn save_changed(
        &self,
        tx: &mut U::Tx,
        mut stored: StoredProfile,
        expected: ShippingManagementRevision,
        context: &OperationContext,
        changed: bool,
    ) -> Result<ProfileView, ShippingError> {
        if changed {
            if expected.into_inner() >= i64::MAX as u64
                || stored.profile.revision().into_inner() > i64::MAX as u64
            {
                return Err(ShippingError::RevisionExhausted);
            }
            stored.management_revision = expected.next();
            self.repositories
                .in_transaction(tx)
                .save(&stored, expected, &caller(context))
                .await?;
        }
        Ok(stored.into())
    }
    async fn lifecycle(
        &self,
        context: &OperationContext,
        command: ProfileLifecycleCommand,
        restore: bool,
    ) -> Result<ProfileView, ShippingError> {
        capability(context, true)?;
        validate_revision(command.expected_revision.into_inner())?;
        let mut tx = self.uow.begin().await?;
        let owner = authorize(&self.access, &mut tx, context, command.source_id).await?;
        let mut stored = self
            .repositories
            .in_transaction(&mut tx)
            .find_for_update(command.source_id, command.profile_id)
            .await?;
        profile_expected(&stored, command.expected_revision)?;
        if restore {
            validate_sites(
                &mut self.access.in_transaction(&mut tx),
                owner,
                &stored.profile,
            )
            .await?;
        }
        let changed = if restore {
            stored.profile.restore()?
        } else {
            stored.profile.retire()?
        }
        .changed();
        let result = self
            .save_changed(&mut tx, stored, command.expected_revision, context, changed)
            .await?;
        tx.commit().await?;
        audit(
            context,
            command.source_id,
            Some(command.profile_id),
            changed,
            if restore {
                "restore_profile"
            } else {
                "retire_profile"
            },
            result.management_revision.into_inner(),
        );
        Ok(result)
    }
}
fn audit(
    context: &OperationContext,
    source: ListingSourceId,
    profile: Option<ShippingProfileId>,
    changed: bool,
    action: &'static str,
    revision: u64,
) {
    tracing::info!(event="shipping.management_committed", action, source_id=%source, profile_id=?profile, actor_type=context.principal.kind(), actor_id=%context.principal.label(), request_id=%context.request_id, correlation_id=%context.correlation_id, changed, revision, outcome="success");
}

#[async_trait::async_trait]
pub trait CreateShippingProfileUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProfileCommand,
    ) -> Result<ProfileView, ShippingError>;
}
pub struct CreateShippingProfileHandler<U, R, A> {
    handler: ShippingWriteHandler<U, R, A>,
}
impl<U, R, A> CreateShippingProfileHandler<U, R, A> {
    pub fn new(uow: U, repositories: R, access: A) -> Self {
        Self {
            handler: ShippingWriteHandler {
                uow,
                repositories,
                access,
            },
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    CreateShippingProfileUseCase for CreateShippingProfileHandler<U, R, A>
{
    #[tracing::instrument(name = "shipping.create_profile", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProfileCommand,
    ) -> Result<ProfileView, ShippingError> {
        let handler = &self.handler;
        capability(context, true)?;
        if command.creation_key.is_empty()
            || command.creation_key.len() > 128
            || !command.creation_key.bytes().all(|b| b.is_ascii_graphic())
        {
            return Err(ShippingError::InvalidInput);
        }
        validate_evidence(&command.evidence)?;
        let profile =
            ShippingProfile::create(ShippingProfileId::new(), command.source_id, command.content)?;
        let mut tx = handler.uow.begin().await?;
        let owner = authorize(&handler.access, &mut tx, context, command.source_id).await?;
        let caller = caller(context);
        let retry = handler
            .repositories
            .in_transaction(&mut tx)
            .creation_retry(
                command.source_id,
                &caller,
                &command.creation_key,
                profile.content(),
                &command.evidence,
            )
            .await?;
        if let Some(stored) = retry {
            tx.commit().await?;
            audit(
                context,
                command.source_id,
                Some(stored.profile.id()),
                false,
                "create_profile",
                stored.management_revision.into_inner(),
            );
            return Ok(stored.into());
        }
        validate_sites(&mut handler.access.in_transaction(&mut tx), owner, &profile).await?;
        let stored = StoredProfile {
            profile,
            management_revision: ShippingManagementRevision::INITIAL,
            evidence: command.evidence,
        };
        handler
            .repositories
            .in_transaction(&mut tx)
            .insert(&stored, &caller, &command.creation_key)
            .await?;
        tx.commit().await?;
        audit(
            context,
            command.source_id,
            Some(stored.profile.id()),
            true,
            "create_profile",
            stored.management_revision.into_inner(),
        );
        Ok(stored.into())
    }
}

#[async_trait::async_trait]
pub trait ReplaceShippingProfileUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ReplaceProfileCommand,
    ) -> Result<ProfileView, ShippingError>;
}
pub struct ReplaceShippingProfileHandler<U, R, A> {
    handler: ShippingWriteHandler<U, R, A>,
}
impl<U, R, A> ReplaceShippingProfileHandler<U, R, A> {
    pub fn new(uow: U, repositories: R, access: A) -> Self {
        Self {
            handler: ShippingWriteHandler {
                uow,
                repositories,
                access,
            },
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    ReplaceShippingProfileUseCase for ReplaceShippingProfileHandler<U, R, A>
{
    #[tracing::instrument(name = "shipping.replace_profile", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ReplaceProfileCommand,
    ) -> Result<ProfileView, ShippingError> {
        let handler = &self.handler;
        capability(context, true)?;
        validate_revision(command.expected_revision.into_inner())?;
        validate_evidence(&command.evidence)?;
        let mut tx = handler.uow.begin().await?;
        let owner = authorize(&handler.access, &mut tx, context, command.source_id).await?;
        let mut stored = handler
            .repositories
            .in_transaction(&mut tx)
            .find_for_update(command.source_id, command.profile_id)
            .await?;
        profile_expected(&stored, command.expected_revision)?;
        let terms_changed = stored.profile.replace_content(command.content)?.changed();
        validate_sites(
            &mut handler.access.in_transaction(&mut tx),
            owner,
            &stored.profile,
        )
        .await?;
        let evidence_changed = stored.evidence != command.evidence;
        if evidence_changed && stored.profile.lifecycle() == ShippingProfileLifecycle::Retired {
            return Err(ShippingProfileError::Retired.into());
        }
        stored.evidence = command.evidence;
        let changed = terms_changed || evidence_changed;
        let result = handler
            .save_changed(&mut tx, stored, command.expected_revision, context, changed)
            .await?;
        tx.commit().await?;
        audit(
            context,
            command.source_id,
            Some(command.profile_id),
            changed,
            "replace_profile",
            result.management_revision.into_inner(),
        );
        Ok(result)
    }
}

#[async_trait::async_trait]
pub trait RetireShippingProfileUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ProfileLifecycleCommand,
    ) -> Result<ProfileView, ShippingError>;
}
pub struct RetireShippingProfileHandler<U, R, A> {
    handler: ShippingWriteHandler<U, R, A>,
}
impl<U, R, A> RetireShippingProfileHandler<U, R, A> {
    pub fn new(uow: U, repositories: R, access: A) -> Self {
        Self {
            handler: ShippingWriteHandler {
                uow,
                repositories,
                access,
            },
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    RetireShippingProfileUseCase for RetireShippingProfileHandler<U, R, A>
{
    #[tracing::instrument(name = "shipping.retire_profile", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ProfileLifecycleCommand,
    ) -> Result<ProfileView, ShippingError> {
        let handler = &self.handler;
        handler.lifecycle(context, command, false).await
    }
}

#[async_trait::async_trait]
pub trait RestoreShippingProfileUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ProfileLifecycleCommand,
    ) -> Result<ProfileView, ShippingError>;
}
pub struct RestoreShippingProfileHandler<U, R, A> {
    handler: ShippingWriteHandler<U, R, A>,
}
impl<U, R, A> RestoreShippingProfileHandler<U, R, A> {
    pub fn new(uow: U, repositories: R, access: A) -> Self {
        Self {
            handler: ShippingWriteHandler {
                uow,
                repositories,
                access,
            },
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    RestoreShippingProfileUseCase for RestoreShippingProfileHandler<U, R, A>
{
    #[tracing::instrument(name = "shipping.restore_profile", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ProfileLifecycleCommand,
    ) -> Result<ProfileView, ShippingError> {
        let handler = &self.handler;
        handler.lifecycle(context, command, true).await
    }
}

#[async_trait::async_trait]
pub trait AssignShippingDefaultUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: AssignDefaultCommand,
    ) -> Result<DefaultView, ShippingError>;
}
pub struct AssignShippingDefaultHandler<U, R, A> {
    handler: ShippingWriteHandler<U, R, A>,
}
impl<U, R, A> AssignShippingDefaultHandler<U, R, A> {
    pub fn new(uow: U, repositories: R, access: A) -> Self {
        Self {
            handler: ShippingWriteHandler {
                uow,
                repositories,
                access,
            },
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    AssignShippingDefaultUseCase for AssignShippingDefaultHandler<U, R, A>
{
    #[tracing::instrument(name = "shipping.assign_default", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: AssignDefaultCommand,
    ) -> Result<DefaultView, ShippingError> {
        let handler = &self.handler;
        capability(context, true)?;
        validate_revision(command.expected_revision.into_inner())?;
        let mut tx = handler.uow.begin().await?;
        let owner = authorize(&handler.access, &mut tx, context, command.source_id).await?;
        let mut stored = handler
            .repositories
            .in_transaction(&mut tx)
            .default_for_update(command.source_id)
            .await?;
        default_expected(&stored, command.expected_revision)?;
        let profile = handler
            .repositories
            .in_transaction(&mut tx)
            .find_for_update(command.source_id, command.profile_id)
            .await?;
        validate_sites(
            &mut handler.access.in_transaction(&mut tx),
            owner,
            &profile.profile,
        )
        .await?;
        let changed = stored.selection.select(Some(&profile.profile))?.changed();
        if changed {
            if stored.revision.into_inner() >= i64::MAX as u64 {
                return Err(ShippingError::RevisionExhausted);
            }
            stored.revision = stored.revision.next();
            handler
                .repositories
                .in_transaction(&mut tx)
                .save_default(&stored, command.expected_revision, &caller(context))
                .await?;
        }
        let view = DefaultView {
            source_id: command.source_id,
            revision: stored.revision,
            profile_id: stored.selection.profile_id(),
            eligibility: Some(DefaultEligibility::Active),
        };
        tx.commit().await?;
        audit(
            context,
            command.source_id,
            Some(command.profile_id),
            changed,
            "assign_default",
            view.revision.into_inner(),
        );
        Ok(view)
    }
}

#[async_trait::async_trait]
pub trait ClearShippingDefaultUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        command: ClearDefaultCommand,
    ) -> Result<DefaultView, ShippingError>;
}
pub struct ClearShippingDefaultHandler<U, R, A> {
    handler: ShippingWriteHandler<U, R, A>,
}
impl<U, R, A> ClearShippingDefaultHandler<U, R, A> {
    pub fn new(uow: U, repositories: R, access: A) -> Self {
        Self {
            handler: ShippingWriteHandler {
                uow,
                repositories,
                access,
            },
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ShippingRepositoryFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    ClearShippingDefaultUseCase for ClearShippingDefaultHandler<U, R, A>
{
    #[tracing::instrument(name = "shipping.clear_default", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        command: ClearDefaultCommand,
    ) -> Result<DefaultView, ShippingError> {
        let handler = &self.handler;
        capability(context, true)?;
        validate_revision(command.expected_revision.into_inner())?;
        let mut tx = handler.uow.begin().await?;
        authorize(&handler.access, &mut tx, context, command.source_id).await?;
        let mut stored = handler
            .repositories
            .in_transaction(&mut tx)
            .default_for_update(command.source_id)
            .await?;
        default_expected(&stored, command.expected_revision)?;
        let changed = stored.selection.select(None)?.changed();
        if changed {
            if stored.revision.into_inner() >= i64::MAX as u64 {
                return Err(ShippingError::RevisionExhausted);
            }
            stored.revision = stored.revision.next();
            handler
                .repositories
                .in_transaction(&mut tx)
                .save_default(&stored, command.expected_revision, &caller(context))
                .await?;
        }
        let view = DefaultView {
            source_id: command.source_id,
            revision: stored.revision,
            profile_id: None,
            eligibility: None,
        };
        tx.commit().await?;
        audit(
            context,
            command.source_id,
            None,
            changed,
            "clear_default",
            view.revision.into_inner(),
        );
        Ok(view)
    }
}

#[async_trait::async_trait]
pub trait GetShippingProfileUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: ShippingProfileId,
    ) -> Result<ProfileView, ShippingError>;
}
pub struct GetShippingProfileHandler<U, Q, A> {
    uow: U,
    readers: Q,
    access: A,
}
impl<U, Q, A> GetShippingProfileHandler<U, Q, A> {
    pub fn new(uow: U, readers: Q, access: A) -> Self {
        Self {
            uow,
            readers,
            access,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, Q: ShippingReaderFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    GetShippingProfileUseCase for GetShippingProfileHandler<U, Q, A>
{
    #[tracing::instrument(name = "shipping.get_profile", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        id: ShippingProfileId,
    ) -> Result<ProfileView, ShippingError> {
        capability(context, false)?;
        let mut tx = self.uow.begin().await?;
        authorize(&self.access, &mut tx, context, source).await?;
        let view = self.readers.in_transaction(&mut tx).get(source, id).await?;
        tx.commit().await?;
        Ok(view)
    }
}

#[async_trait::async_trait]
pub trait ListShippingProfilesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        after: Option<ShippingProfileId>,
        limit: u32,
    ) -> Result<Vec<ProfileSummary>, ShippingError>;
}
pub struct ListShippingProfilesHandler<U, Q, A> {
    uow: U,
    readers: Q,
    access: A,
}
impl<U, Q, A> ListShippingProfilesHandler<U, Q, A> {
    pub fn new(uow: U, readers: Q, access: A) -> Self {
        Self {
            uow,
            readers,
            access,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, Q: ShippingReaderFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    ListShippingProfilesUseCase for ListShippingProfilesHandler<U, Q, A>
{
    #[tracing::instrument(name = "shipping.list_profiles", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
        after: Option<ShippingProfileId>,
        limit: u32,
    ) -> Result<Vec<ProfileSummary>, ShippingError> {
        capability(context, false)?;
        if !(1..=100).contains(&limit) {
            return Err(ShippingError::InvalidInput);
        }
        let mut tx = self.uow.begin().await?;
        authorize(&self.access, &mut tx, context, source).await?;
        let views = self
            .readers
            .in_transaction(&mut tx)
            .list(source, after, limit)
            .await?;
        tx.commit().await?;
        Ok(views)
    }
}

#[async_trait::async_trait]
pub trait GetShippingDefaultUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
    ) -> Result<DefaultView, ShippingError>;
}
pub struct GetShippingDefaultHandler<U, Q, A> {
    uow: U,
    readers: Q,
    access: A,
}
impl<U, Q, A> GetShippingDefaultHandler<U, Q, A> {
    pub fn new(uow: U, readers: Q, access: A) -> Self {
        Self {
            uow,
            readers,
            access,
        }
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, Q: ShippingReaderFactory<U::Tx>, A: SourceLocationAccessFactory<U::Tx>>
    GetShippingDefaultUseCase for GetShippingDefaultHandler<U, Q, A>
{
    #[tracing::instrument(name = "shipping.get_default", skip_all, fields(principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        source: ListingSourceId,
    ) -> Result<DefaultView, ShippingError> {
        capability(context, false)?;
        let mut tx = self.uow.begin().await?;
        authorize(&self.access, &mut tx, context, source).await?;
        let view = self
            .readers
            .in_transaction(&mut tx)
            .get_default(source)
            .await?;
        tx.commit().await?;
        Ok(view)
    }
}
