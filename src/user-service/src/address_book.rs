//! Own private address commands and reads. No provider calls occur on this path.
use application::{
    error::BoxError,
    operation_context::{
        CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
    },
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use geo::{DerivedGeography, GeographicDescription, SubmittedGeography};
use geo_service::{geocoding::GeocodingRequest, resolution::*};
use user_core::{user_address::*, user_id::UserId};

#[derive(Debug, thiserror::Error)]
pub enum AddressBookError {
    #[error("authentication required")]
    AuthenticationRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("address or user not found")]
    NotFound,
    #[error("address or destination changed")]
    Conflict,
    #[error("invalid address book input")]
    InvalidInput,
    #[error("address book storage failed")]
    Storage(#[source] BoxError),
    #[error("invalid persisted address book")]
    InvalidState(#[source] BoxError),
    #[error("address book transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
    #[error("address resolution failed")]
    Resolution(
        #[from]
        #[source]
        ResolutionError,
    ),
}
impl From<OperationAuthorizationError> for AddressBookError {
    fn from(e: OperationAuthorizationError) -> Self {
        match e {
            OperationAuthorizationError::AuthenticationRequired(_) => Self::AuthenticationRequired,
            _ => Self::Forbidden,
        }
    }
}
impl From<InvalidUserAddress> for AddressBookError {
    fn from(_: InvalidUserAddress) -> Self {
        Self::InvalidInput
    }
}
fn owner(ctx: &OperationContext, write: bool) -> Result<UserId, AddressBookError> {
    ctx.require()
        .any_user()
        .credential_capability(if write {
            CredentialCapability::UsersWrite
        } else {
            CredentialCapability::UsersRead
        })
        .authorize::<AddressBookError>()?;
    match ctx.principal {
        Principal::User(id) | Principal::DelegatedUser { user_id: id, .. } => Ok(id),
        _ => Err(AddressBookError::Forbidden),
    }
}
fn valid_version(version: u64) -> Result<(), AddressBookError> {
    if !(1..=i64::MAX as u64).contains(&version) {
        return Err(AddressBookError::InvalidInput);
    }
    Ok(())
}

pub struct StoredUserAddress {
    pub address: UserAddress,
    pub version: u64,
}
#[derive(Clone)]
pub struct UserAddressView {
    pub id: UserAddressId,
    pub label: UserAddressLabel,
    pub geography: GeographicDescription,
    pub input_revision: u64,
    pub version: u64,
    pub preferred: bool,
    pub resolution: UserAddressResolution,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAddressResolutionStatus {
    Pending,
    Accepted,
    Ambiguous,
    NoMatch,
    Failed,
    Blocked,
    Unavailable,
}
impl UserAddressResolutionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Accepted => "ACCEPTED",
            Self::Ambiguous => "AMBIGUOUS",
            Self::NoMatch => "NO_MATCH",
            Self::Failed => "FAILED",
            Self::Blocked => "BLOCKED",
            Self::Unavailable => "UNAVAILABLE",
        }
    }
}
#[derive(Clone)]
pub struct UserAddressAttribution {
    pub provider: String,
    pub attribution_url: String,
}
#[derive(Clone)]
pub struct UserAddressResolution {
    pub status: UserAddressResolutionStatus,
    pub derived: Option<DerivedGeography>,
    pub attribution: Option<UserAddressAttribution>,
}
impl UserAddressResolution {
    pub fn unavailable() -> Self {
        Self {
            status: UserAddressResolutionStatus::Unavailable,
            derived: None,
            attribution: None,
        }
    }
    fn from_view(view: &ResolutionView) -> Self {
        let status = match view.status {
            ResolutionStatus::Pending => UserAddressResolutionStatus::Pending,
            ResolutionStatus::Accepted if view.evidence.is_some() => {
                UserAddressResolutionStatus::Accepted
            }
            ResolutionStatus::Ambiguous if view.evidence.is_some() => {
                UserAddressResolutionStatus::Ambiguous
            }
            ResolutionStatus::NoMatch if view.evidence.is_some() => {
                UserAddressResolutionStatus::NoMatch
            }
            ResolutionStatus::Failed => UserAddressResolutionStatus::Failed,
            ResolutionStatus::Blocked => UserAddressResolutionStatus::Blocked,
            _ => UserAddressResolutionStatus::Unavailable,
        };
        let accepted = view
            .evidence
            .as_ref()
            .and_then(|e| e.accepted_index.and_then(|index| e.candidates.get(index)));
        Self {
            status,
            derived: accepted.map(|c| c.geography.clone()),
            attribution: accepted.map(|c| UserAddressAttribution {
                provider: c.provider.clone(),
                attribution_url: c.attribution_url.clone(),
            }),
        }
    }
}
impl std::fmt::Debug for UserAddressView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserAddressView")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}
pub struct UserAddressPage {
    pub items: Vec<UserAddressView>,
    pub next: Option<UserAddressId>,
}
pub struct DestinationView {
    pub destination: BrowsingDestination,
    pub version: u64,
}
/// Authorized snapshot for this request. Persisted filters must copy values, never IDs.
pub struct ResolvedDestination {
    pub destination: BrowsingDestination,
    pub submitted: Option<GeographicDescription>,
    pub derived: Option<DerivedGeography>,
    pub resolution_status: Option<UserAddressResolutionStatus>,
    pub attribution: Option<UserAddressAttribution>,
}

#[async_trait::async_trait]
pub trait AddressBookAccess: Send {
    /// Lock the current owner against account erasure and concurrent preferences/edits.
    async fn lock_owner(&mut self, owner: UserId) -> Result<(), AddressBookError>;
}
#[async_trait::async_trait]
pub trait UserAddressRepository: Send {
    async fn find(
        &mut self,
        owner: UserId,
        id: UserAddressId,
    ) -> Result<Option<StoredUserAddress>, AddressBookError>;
    async fn insert(
        &mut self,
        address: &UserAddress,
    ) -> Result<StoredUserAddress, AddressBookError>;
    async fn update(
        &mut self,
        address: &UserAddress,
        expected: u64,
    ) -> Result<StoredUserAddress, AddressBookError>;
    /// Remove private content; retain only an ID tombstone until account erasure.
    async fn delete(
        &mut self,
        owner: UserId,
        id: UserAddressId,
        expected: u64,
    ) -> Result<(), AddressBookError>;
    async fn set_destination(
        &mut self,
        owner: UserId,
        destination: &BrowsingDestination,
        expected: u64,
    ) -> Result<(), AddressBookError>;
}
#[async_trait::async_trait]
pub trait UserAddressReader: Send {
    async fn get(
        &mut self,
        owner: UserId,
        id: UserAddressId,
    ) -> Result<Option<UserAddressView>, AddressBookError>;
    async fn list(
        &mut self,
        owner: UserId,
        after: Option<UserAddressId>,
        limit: u32,
    ) -> Result<UserAddressPage, AddressBookError>;
    async fn destination(&mut self, owner: UserId) -> Result<DestinationView, AddressBookError>;
}
pub trait AddressBookFactory<Tx>: Send + Sync {
    fn access<'a>(&'a self, tx: &'a mut Tx) -> impl AddressBookAccess + 'a;
    fn repository<'a>(&'a self, tx: &'a mut Tx) -> impl UserAddressRepository + 'a;
    fn reader<'a>(&'a self, tx: &'a mut Tx) -> impl UserAddressReader + 'a;
}
pub struct CreateUserAddress {
    pub label: UserAddressLabel,
    pub geography: GeographicDescription,
}
pub struct UpdateUserAddress {
    pub id: UserAddressId,
    pub expected_version: u64,
    pub label: UserAddressLabel,
    pub geography: GeographicDescription,
}

#[async_trait::async_trait]
pub trait AddressBookCommands: Send + Sync {
    async fn create(
        &self,
        ctx: &OperationContext,
        command: CreateUserAddress,
    ) -> Result<UserAddressView, AddressBookError>;
    async fn update(
        &self,
        ctx: &OperationContext,
        command: UpdateUserAddress,
    ) -> Result<UserAddressView, AddressBookError>;
    async fn delete(
        &self,
        ctx: &OperationContext,
        id: UserAddressId,
        expected: u64,
    ) -> Result<(), AddressBookError>;
    async fn set_destination(
        &self,
        ctx: &OperationContext,
        destination: BrowsingDestination,
        expected: u64,
    ) -> Result<DestinationView, AddressBookError>;
}
#[async_trait::async_trait]
pub trait AddressBookQueries: Send + Sync {
    async fn get(
        &self,
        ctx: &OperationContext,
        id: UserAddressId,
    ) -> Result<UserAddressView, AddressBookError>;
    async fn list(
        &self,
        ctx: &OperationContext,
        after: Option<UserAddressId>,
        limit: u32,
    ) -> Result<UserAddressPage, AddressBookError>;
    async fn destination(
        &self,
        ctx: &OperationContext,
    ) -> Result<DestinationView, AddressBookError>;
    async fn resolve_destination(
        &self,
        ctx: &OperationContext,
        explicit: Option<BrowsingDestination>,
    ) -> Result<ResolvedDestination, AddressBookError>;
}
pub struct AddressBookHandler<U, A, R> {
    uow: U,
    addresses: A,
    resolutions: R,
}
impl<U, A, R> AddressBookHandler<U, A, R> {
    pub fn new(uow: U, addresses: A, resolutions: R) -> Self {
        Self {
            uow,
            addresses,
            resolutions,
        }
    }
}
fn subject(id: UserAddressId) -> Result<ResolutionSubject, AddressBookError> {
    ResolutionSubject::new(RetentionPurpose::UserLocation, id.into_uuid()).map_err(Into::into)
}
fn binding(address: &UserAddress, version: u64) -> Result<ResolutionBinding, AddressBookError> {
    Ok(ResolutionBinding {
        subject: subject(address.id())?,
        input_revision: address.input_revision(),
        guard: version,
    })
}
fn input(address: &UserAddress) -> Result<ResolutionInput, AddressBookError> {
    let query = address
        .geography()
        .address_text()
        .ok_or(AddressBookError::InvalidInput)?
        .clone();
    Ok(ResolutionInput::new(
        GeocodingRequest::new(query)
            .with_submitted(SubmittedGeography::new(address.geography().clone())),
        ResolutionScope::new(ScopeKind::User, Some(address.owner().into_uuid()))?,
        RetentionPurpose::UserLocation,
    )?)
}
impl<U, A, R> AddressBookHandler<U, A, R>
where
    U: UnitOfWork,
    A: AddressBookFactory<U::Tx>,
    R: ResolutionRepositoryFactory<U::Tx>,
{
    async fn begin(
        &self,
        ctx: &OperationContext,
        write: bool,
    ) -> Result<(UserId, U::Tx), AddressBookError> {
        let owner = owner(ctx, write)?;
        let mut tx = self.uow.begin().await?;
        self.addresses.access(&mut tx).lock_owner(owner).await?;
        Ok((owner, tx))
    }
    async fn view(
        &self,
        tx: &mut U::Tx,
        owner: UserId,
        id: UserAddressId,
    ) -> Result<UserAddressView, AddressBookError> {
        let mut view = self
            .addresses
            .reader(tx)
            .get(owner, id)
            .await?
            .ok_or(AddressBookError::NotFound)?;
        let resolution = self
            .resolutions
            .resolution_reader(tx)
            .inspect(subject(id)?)
            .await?;
        Self::enrich(&mut view, owner, resolution.as_ref())?;
        Ok(view)
    }
    fn enrich(
        view: &mut UserAddressView,
        owner: UserId,
        resolution: Option<&ResolutionView>,
    ) -> Result<(), AddressBookError> {
        if let Some(resolution) = resolution {
            if resolution.scope.kind() != ScopeKind::User
                || resolution.scope.owner() != Some(owner.into_uuid())
            {
                return Err(AddressBookError::InvalidState(Box::new(
                    std::io::Error::other("invalid private resolution ownership"),
                )));
            }
            if resolution.input_revision != view.input_revision || resolution.guard != view.version
            {
                return Err(AddressBookError::Conflict);
            }
            view.resolution = UserAddressResolution::from_view(resolution);
        }
        Ok(())
    }
    async fn current(
        &self,
        tx: &mut U::Tx,
        owner: UserId,
        id: UserAddressId,
        expected: u64,
    ) -> Result<StoredUserAddress, AddressBookError> {
        valid_version(expected)?;
        let stored = self
            .addresses
            .repository(tx)
            .find(owner, id)
            .await?
            .ok_or(AddressBookError::NotFound)?;
        if stored.version != expected {
            return Err(AddressBookError::Conflict);
        }
        Ok(stored)
    }
}
#[async_trait::async_trait]
impl<U, A, R> AddressBookCommands for AddressBookHandler<U, A, R>
where
    U: UnitOfWork,
    A: AddressBookFactory<U::Tx>,
    R: ResolutionRepositoryFactory<U::Tx>,
{
    async fn create(
        &self,
        ctx: &OperationContext,
        c: CreateUserAddress,
    ) -> Result<UserAddressView, AddressBookError> {
        let (owner, mut tx) = self.begin(ctx, true).await?;
        let address = UserAddress::create(UserAddressId::new(), owner, c.label, c.geography)?;
        let stored = self.addresses.repository(&mut tx).insert(&address).await?;
        self.resolutions
            .resolutions(&mut tx)
            .bind(
                &input(&stored.address)?,
                binding(&stored.address, stored.version)?,
            )
            .await?;
        let id = stored.address.id();
        let view = self.view(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(view)
    }
    async fn update(
        &self,
        ctx: &OperationContext,
        c: UpdateUserAddress,
    ) -> Result<UserAddressView, AddressBookError> {
        let (owner, mut tx) = self.begin(ctx, true).await?;
        let mut stored = self
            .current(&mut tx, owner, c.id, c.expected_version)
            .await?;
        if stored.address.replace(c.label, c.geography)? {
            stored = self
                .addresses
                .repository(&mut tx)
                .update(&stored.address, c.expected_version)
                .await?;
            self.resolutions
                .resolutions(&mut tx)
                .bind(
                    &input(&stored.address)?,
                    binding(&stored.address, stored.version)?,
                )
                .await?;
        }
        let view = self.view(&mut tx, owner, c.id).await?;
        tx.commit().await?;
        Ok(view)
    }
    async fn delete(
        &self,
        ctx: &OperationContext,
        id: UserAddressId,
        expected: u64,
    ) -> Result<(), AddressBookError> {
        let (owner, mut tx) = self.begin(ctx, true).await?;
        self.current(&mut tx, owner, id, expected).await?;
        self.addresses
            .repository(&mut tx)
            .delete(owner, id, expected)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn set_destination(
        &self,
        ctx: &OperationContext,
        destination: BrowsingDestination,
        expected: u64,
    ) -> Result<DestinationView, AddressBookError> {
        valid_version(expected)?;
        let (owner, mut tx) = self.begin(ctx, true).await?;
        if let BrowsingDestination::Address(id) = &destination {
            self.view(&mut tx, owner, *id).await?;
        }
        self.addresses
            .repository(&mut tx)
            .set_destination(owner, &destination, expected)
            .await?;
        let view = self.addresses.reader(&mut tx).destination(owner).await?;
        tx.commit().await?;
        Ok(view)
    }
}
#[async_trait::async_trait]
impl<U, A, R> AddressBookQueries for AddressBookHandler<U, A, R>
where
    U: UnitOfWork,
    A: AddressBookFactory<U::Tx>,
    R: ResolutionRepositoryFactory<U::Tx>,
{
    async fn get(
        &self,
        ctx: &OperationContext,
        id: UserAddressId,
    ) -> Result<UserAddressView, AddressBookError> {
        let (owner, mut tx) = self.begin(ctx, false).await?;
        let view = self.view(&mut tx, owner, id).await?;
        tx.commit().await?;
        Ok(view)
    }
    async fn list(
        &self,
        ctx: &OperationContext,
        after: Option<UserAddressId>,
        limit: u32,
    ) -> Result<UserAddressPage, AddressBookError> {
        if !(1..=100).contains(&limit) {
            return Err(AddressBookError::InvalidInput);
        }
        let (owner, mut tx) = self.begin(ctx, false).await?;
        let mut page = self
            .addresses
            .reader(&mut tx)
            .list(owner, after, limit)
            .await?;
        let subjects = page
            .items
            .iter()
            .map(|item| subject(item.id))
            .collect::<Result<Vec<_>, _>>()?;
        let resolutions = self
            .resolutions
            .resolution_reader(&mut tx)
            .inspect_many(&subjects)
            .await?;
        if resolutions.len() != page.items.len() {
            return Err(AddressBookError::InvalidState(Box::new(
                std::io::Error::other("invalid resolution read cardinality"),
            )));
        }
        for (item, resolution) in page.items.iter_mut().zip(resolutions) {
            Self::enrich(item, owner, resolution.as_ref())?;
        }
        tx.commit().await?;
        Ok(page)
    }
    async fn destination(
        &self,
        ctx: &OperationContext,
    ) -> Result<DestinationView, AddressBookError> {
        let (owner, mut tx) = self.begin(ctx, false).await?;
        let view = self.addresses.reader(&mut tx).destination(owner).await?;
        tx.commit().await?;
        Ok(view)
    }
    async fn resolve_destination(
        &self,
        ctx: &OperationContext,
        explicit: Option<BrowsingDestination>,
    ) -> Result<ResolvedDestination, AddressBookError> {
        let (owner, mut tx) = self.begin(ctx, false).await?;
        let destination = match explicit {
            Some(d) => d,
            None => {
                self.addresses
                    .reader(&mut tx)
                    .destination(owner)
                    .await?
                    .destination
            }
        };
        let (submitted, derived, resolution_status, attribution) = match &destination {
            BrowsingDestination::None => (None, None, None, None),
            BrowsingDestination::CountryPostal {
                country,
                postal_code,
            } => (
                GeographicDescription::new(None, Some(*country), None, postal_code.clone())
                    .map_err(|_| AddressBookError::InvalidInput)?,
                None,
                None,
                None,
            ),
            BrowsingDestination::Address(id) => {
                let current = self.view(&mut tx, owner, *id).await?;
                (
                    Some(current.geography),
                    current.resolution.derived,
                    Some(current.resolution.status),
                    current.resolution.attribution,
                )
            }
        };
        tx.commit().await?;
        Ok(ResolvedDestination {
            destination,
            submitted,
            derived,
            resolution_status,
            attribution,
        })
    }
}
