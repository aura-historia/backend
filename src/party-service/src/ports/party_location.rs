use crate::use_cases::party_locations::{
    CreatePartyLocationCommand, LocationEvidence, PartyLocationError, PartyLocationView,
    PartyLocationsPage,
};
use party_core::{
    party_id::PartyId, party_location::PartyLocation, party_location_id::PartyLocationId,
};
use time::OffsetDateTime;
use user_core::user_id::UserId;

domain_primitives::version_newtype!(PartyLocationRevision);

#[derive(Debug, Clone, PartialEq)]
pub struct StoredPartyLocation {
    pub location: PartyLocation,
    pub revision: PartyLocationRevision,
    pub evidence: Option<LocationEvidence>,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

/// All management operations lock the owner first, including grant changes.
#[async_trait::async_trait]
pub trait PartyLocationAccess: Send {
    async fn lock_party(&mut self, party_id: PartyId) -> Result<bool, PartyLocationError>;
    /// Unsuspended users may manage through an explicit grant or active Party partnership membership.
    async fn can_manage_locations(
        &mut self,
        party_id: PartyId,
        user_id: UserId,
    ) -> Result<bool, PartyLocationError>;
    async fn set_management_grant(
        &mut self,
        party_id: PartyId,
        user_id: UserId,
        granted: bool,
        actor: &str,
    ) -> Result<(), PartyLocationError>;
}
pub trait PartyLocationAccessFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyLocationAccess + 'tx;
}

#[async_trait::async_trait]
pub trait PartyLocationRepository: Send {
    /// Returns the original result, or fails on conflicting command reuse. Owner is locked.
    async fn replay_create(
        &mut self,
        actor: &str,
        command: &CreatePartyLocationCommand,
    ) -> Result<Option<StoredPartyLocation>, PartyLocationError>;
    async fn record_create(
        &mut self,
        actor: &str,
        command: &CreatePartyLocationCommand,
        result: &StoredPartyLocation,
    ) -> Result<(), PartyLocationError>;
    async fn find_for_update(
        &mut self,
        party_id: PartyId,
        id: PartyLocationId,
    ) -> Result<Option<StoredPartyLocation>, PartyLocationError>;
    /// Aggregate and privacy-safe revision signal persist in the same transaction.
    async fn insert(
        &mut self,
        location: &PartyLocation,
        evidence: Option<&LocationEvidence>,
        actor: &str,
    ) -> Result<StoredPartyLocation, PartyLocationError>;
    async fn update(
        &mut self,
        stored: &StoredPartyLocation,
        actor: &str,
    ) -> Result<StoredPartyLocation, PartyLocationError>;
}
pub trait PartyLocationRepositoryFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyLocationRepository + 'tx;
}

#[async_trait::async_trait]
pub trait PartyLocationReader: Send {
    async fn get(
        &mut self,
        party_id: PartyId,
        id: PartyLocationId,
    ) -> Result<Option<PartyLocationView>, PartyLocationError>;
    async fn list(
        &mut self,
        party_id: PartyId,
        after: Option<PartyLocationId>,
        limit: u32,
    ) -> Result<PartyLocationsPage, PartyLocationError>;
}
pub trait PartyLocationReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyLocationReader + 'tx;
}

/// A lifecycle command retains all historical references and never selects a replacement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpectedPartyLocation {
    pub id: PartyLocationId,
    pub revision: PartyLocationRevision,
}
