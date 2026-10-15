use crate::*;
use product_listing_core::delivery::ListingDelivery;

#[async_trait::async_trait]
pub trait SyncStore: Send {
    /// Source/grant authorization is locked by the handler first. Validate enabled provider
    /// and exact configured installation domain; never derive authority from an input epoch.
    async fn binding(&mut self, source: ListingSourceId, epoch: Uuid)
    -> Result<Binding, SyncError>;
    async fn rotate(
        &mut self,
        source: ListingSourceId,
        integration: &DeliveryIntegration,
        expected_revision: i64,
    ) -> Result<Binding, SyncError>;
    async fn current(
        &mut self,
        source: ListingSourceId,
        integration: &DeliveryIntegration,
    ) -> Result<Binding, SyncError>;
    async fn revoke(&mut self, binding: &Binding) -> Result<(), SyncError>;
    /// Erase at most `limit` retained raw sessions after 30 days, preserving immutable
    /// digests, retry identities, canonical ownership and deletion fences.
    async fn cleanup(&mut self, source: ListingSourceId, limit: u32) -> Result<u32, SyncError>;
    async fn head(
        &mut self,
        binding: &Binding,
        scope: &ExternalKey,
    ) -> Result<ScopeHead, SyncError>;
    async fn begin(&mut self, binding: &Binding, request: &Begin) -> Result<Receipt, SyncError>;
    async fn receipt(&mut self, source: ListingSourceId, id: Uuid) -> Result<Receipt, SyncError>;
    async fn capture(
        &mut self,
        receipt: &Receipt,
        index: u32,
        chunk: &Chunk,
    ) -> Result<(), SyncError>;
    async fn chunks(&mut self, receipt: &Receipt) -> Result<Vec<Chunk>, SyncError>;
    async fn seal(
        &mut self,
        receipt: &Receipt,
        manifest: Manifest,
        complete: bool,
    ) -> Result<(), SyncError>;
    async fn state(
        &mut self,
        receipt: &Receipt,
        state: ApplicationState,
        failure: Option<&str>,
    ) -> Result<(), SyncError>;
    /// Advance the active scope generation and receipt together; protected state and
    /// normal change signals have already been written in this same transaction.
    async fn activate(
        &mut self,
        receipt: &Receipt,
        configuration_changed: bool,
    ) -> Result<(), SyncError>;
    async fn claims(
        &mut self,
        binding: &Binding,
        scope: &ExternalKey,
    ) -> Result<Vec<Claim>, SyncError>;
    async fn claim(
        &mut self,
        binding: &Binding,
        kind: ObjectKind,
        key: &ExternalKey,
    ) -> Result<Option<Claim>, SyncError>;
    async fn save_claim(&mut self, binding: &Binding, claim: &Claim) -> Result<(), SyncError>;
    async fn mark_profile(
        &mut self,
        binding: &Binding,
        id: ShippingProfileId,
        window: ObservationWindow,
        changed: bool,
    ) -> Result<(), SyncError>;
    async fn mark_item(
        &mut self,
        binding: &Binding,
        delivery: &ListingDelivery,
        key: &SourceListingId,
        owned: [bool; 4],
        window: ObservationWindow,
        changed: [bool; 4],
    ) -> Result<(), SyncError>;
    async fn item_ownership(
        &mut self,
        binding: &Binding,
        key: &SourceListingId,
    ) -> Result<ItemOwnership, SyncError>;
    async fn default_owner(&mut self, binding: &Binding) -> Result<Option<Uuid>, SyncError>;
    async fn mark_default(
        &mut self,
        binding: &Binding,
        window: ObservationWindow,
        changed: bool,
    ) -> Result<(), SyncError>;
    /// Bounded, address-free recovery directory. The caller retries exact finalization.
    async fn pending(
        &mut self,
        binding: &Binding,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Uuid>, SyncError>;
}
pub trait SyncStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl SyncStore + 'a;
}

#[async_trait::async_trait]
pub trait SyncApplication<Tx>: Send + Sync {
    /// Returns whether shared canonical configuration changed. No-ops, missing extraction
    /// and evidence-only correction must not invalidate reliable membership assertions.
    async fn apply(
        &self,
        tx: &mut Tx,
        binding: &Binding,
        receipt: &Receipt,
        records: &[Record],
    ) -> Result<bool, SyncError>;
}
