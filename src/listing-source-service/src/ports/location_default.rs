use crate::use_cases::location_default::{
    LocationDefaultError, LocationDefaultView, SelectedLocationEligibility,
};
use application::operation_context::Principal;
use listing_source_core::{ListingSourceId, location_default::ListingSourceLocationDefault};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};

domain_primitives::version_newtype!(LocationDefaultRevision);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredLocationDefault {
    pub default: ListingSourceLocationDefault,
    pub revision: LocationDefaultRevision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLocationAccessPolicy {
    AdministratorOrOperator,
    AdministratorOnly,
}

#[async_trait::async_trait]
pub trait SourceLocationAccess: Send {
    /// Locks owner Party before source, revalidates ownership and current authority.
    /// Trusted service/system principals are allowed under either policy.
    async fn authorize_source(
        &mut self,
        principal: &Principal,
        source_id: ListingSourceId,
        policy: SourceLocationAccessPolicy,
    ) -> Result<PartyId, LocationDefaultError>;
    /// Locks the selected location against concurrent retirement; never selects an alternative.
    async fn check_location(
        &mut self,
        party_id: PartyId,
        location_id: PartyLocationId,
    ) -> Result<SelectedLocationEligibility, LocationDefaultError>;
}
pub trait SourceLocationAccessFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SourceLocationAccess + 'tx;
}

#[async_trait::async_trait]
pub trait LocationDefaultRepository: Send {
    async fn find_for_update(
        &mut self,
        source_id: ListingSourceId,
    ) -> Result<StoredLocationDefault, LocationDefaultError>;
    /// Persists reference/evidence and minimal authoritative invalidation atomically.
    async fn update(
        &mut self,
        stored: &StoredLocationDefault,
        actor: &str,
    ) -> Result<StoredLocationDefault, LocationDefaultError>;
}
pub trait LocationDefaultRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl LocationDefaultRepository + 'tx;
}

#[async_trait::async_trait]
pub trait LocationDefaultReader: Send {
    /// Protected reference/evidence view; contains no PartyLocation address or point.
    async fn get(
        &mut self,
        source_id: ListingSourceId,
    ) -> Result<LocationDefaultView, LocationDefaultError>;
}
pub trait LocationDefaultReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl LocationDefaultReader + 'tx;
}

#[async_trait::async_trait]
pub trait EffectiveSourceGeographyReader: Send + Sync {
    /// Current geography of active locations for both meanings in one authoritative read.
    async fn get(
        &self,
        source_id: ListingSourceId,
    ) -> Result<
        crate::use_cases::location_default::EffectiveSourceGeographyView,
        LocationDefaultError,
    >;
}
