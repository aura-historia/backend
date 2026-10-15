//! Durable, bounded effective shipping projection. All authority is reread per page.
use crate::assessment::{
    AssertionEvidence, AssessmentDependencies, AssessmentError, ProfileAssessmentBasis,
    assess_listing,
};
use application::{
    error::BoxError,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use product_listing_core::product_listing_id::ProductListingId;
use shipping_core::{
    discovery::{EffectiveShipping, derive},
    selection::ProfileSelection,
};
use time::OffsetDateTime;
use uuid::Uuid;

domain_primitives::object_id_newtype!(ShippingWorkId, "swork");
#[derive(Debug, Clone)]
pub struct ShippingClaim {
    pub work_id: ShippingWorkId,
    pub generation: i64,
    pub token: Uuid,
    pub kind: String,
    pub dependency_id: String,
    pub after_listing: Option<Uuid>,
}
pub struct ShippingPage {
    pub dependencies: AssessmentDependencies,
    pub after_listing: Option<Uuid>,
    pub complete: bool,
}
pub struct ShippingWrite {
    pub listing_id: ProductListingId,
    pub listing_revision: u64,
    pub source_revision: u64,
    pub source_id: listing_source_core::ListingSourceId,
    pub integration_current: bool,
    pub default_revision: Option<u64>,
    pub profile_reference: Option<shipping_core::profile::ShippingProfileId>,
    pub profile: Option<ProfileAssessmentBasis>,
    pub evidence: Option<AssertionEvidence>,
    pub locations: Vec<ShippingLocationBasis>,
    pub location_references: Vec<party_core::party_location_id::PartyLocationId>,
    pub summary: EffectiveShipping,
    pub valid_until: Option<OffsetDateTime>,
}
pub struct ShippingLocationBasis {
    pub id: party_core::party_location_id::PartyLocationId,
    pub revision: u64,
    pub input_revision: u64,
    pub active: bool,
    pub resolution_id: Option<Uuid>,
    pub resolution_generation: Option<u64>,
    pub valid_until: Option<OffsetDateTime>,
}
#[derive(Debug, thiserror::Error)]
pub enum ShippingProjectionError {
    #[error("effective shipping persistence failed")]
    Storage(#[source] BoxError),
    #[error("effective shipping transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
    #[error("effective shipping dependency read failed")]
    Assessment(
        #[from]
        #[source]
        AssessmentError,
    ),
    #[error("shipping projection claim was superseded or expired")]
    Superseded,
    #[error("shipping projection work is claimed")]
    Deferred,
    #[error("invalid shipping projection input")]
    InvalidInput,
}
#[async_trait::async_trait]
pub trait ShippingProjectionStore: Send {
    async fn maintain(&mut self, limit: u16) -> Result<(), ShippingProjectionError>;
    async fn claim(
        &mut self,
        work: Option<ShippingWorkId>,
    ) -> Result<Option<ShippingClaim>, ShippingProjectionError>;
    async fn page(
        &mut self,
        claim: &ShippingClaim,
        limit: u16,
    ) -> Result<ShippingPage, ShippingProjectionError>;
    async fn apply(&mut self, writes: &[ShippingWrite]) -> Result<u32, ShippingProjectionError>;
    async fn checkpoint(
        &mut self,
        claim: &ShippingClaim,
        page: &ShippingPage,
    ) -> Result<(), ShippingProjectionError>;
}
pub trait ShippingProjectionStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl ShippingProjectionStore + 'a;
}
#[async_trait::async_trait]
pub trait ProjectListingShippingUseCase: Send + Sync {
    /// None also recovers expired evidence and lost wake-ups, without any provider calls.
    async fn execute(
        &self,
        work: Option<ShippingWorkId>,
        limit: u16,
    ) -> Result<u32, ShippingProjectionError>;
}
pub struct ProjectListingShippingHandler<U, S> {
    uow: U,
    store: S,
}
impl<U, S> ProjectListingShippingHandler<U, S> {
    pub fn new(uow: U, store: S) -> Self {
        Self { uow, store }
    }
}
#[async_trait::async_trait]
impl<U, S> ProjectListingShippingUseCase for ProjectListingShippingHandler<U, S>
where
    U: UnitOfWork,
    S: ShippingProjectionStoreFactory<U::Tx>,
{
    #[tracing::instrument(name = "shipping.project", skip_all)]
    async fn execute(
        &self,
        work: Option<ShippingWorkId>,
        limit: u16,
    ) -> Result<u32, ShippingProjectionError> {
        if !(1..=100).contains(&limit) {
            return Err(ShippingProjectionError::InvalidInput);
        }
        let mut tx = self.uow.begin().await?;
        if work.is_none() {
            self.store.in_transaction(&mut tx).maintain(limit).await?;
        }
        let claim = self.store.in_transaction(&mut tx).claim(work).await?;
        tx.commit().await?;
        let Some(claim) = claim else {
            return Ok(0);
        };
        let mut tx = self.uow.begin().await?;
        let page = self
            .store
            .in_transaction(&mut tx)
            .page(&claim, limit)
            .await?;
        let writes = page
            .dependencies
            .listings
            .iter()
            .map(|listing| {
                let view = assess_listing(&page.dependencies, listing, Default::default());
                let locations: Vec<_> = page
                    .dependencies
                    .locations
                    .iter()
                    .filter(|l| l.location.party_id() == listing.operator_id)
                    .map(|l| l.location.clone())
                    .collect();
                let raw = shipping_core::assessment::EffectiveDeliveryTerms::resolve(
                    &listing.delivery.configuration(),
                    page.dependencies
                        .defaults
                        .iter()
                        .find(|d| d.selection.source_id() == listing.source_id)
                        .map(|d| &d.selection),
                    &page
                        .dependencies
                        .profiles
                        .iter()
                        .map(|p| p.profile.clone())
                        .collect::<Vec<_>>(),
                );
                let referenced = crate::assessment::referenced_locations(raw.terms());
                let mut basis = page
                    .dependencies
                    .locations
                    .iter()
                    .filter(|l| {
                        l.location.party_id() == listing.operator_id
                            && referenced.contains(&l.location.id())
                    })
                    .map(|l| ShippingLocationBasis {
                        id: l.location.id(),
                        revision: l.revision,
                        input_revision: l.location.input_revision().into_inner(),
                        resolution_id: l.resolution_id,
                        active: l.location.eligible_for_assignment(),
                        resolution_generation: l.resolution_generation,
                        valid_until: l.resolution_valid_until,
                    })
                    .collect::<Vec<_>>();
                basis.sort_unstable_by_key(|l| l.id);
                let default = page
                    .dependencies
                    .defaults
                    .iter()
                    .find(|d| d.selection.source_id() == listing.source_id);
                let profile_reference = match listing.delivery.selection() {
                    Some(ProfileSelection::Explicit(id)) => Some(*id),
                    Some(ProfileSelection::InheritSourceDefault) => {
                        default.and_then(|d| d.selection.profile_id())
                    }
                    _ => None,
                };
                let valid_until = listing
                    .evidence
                    .and_then(|e| e.expires_at)
                    .into_iter()
                    .chain(view.profile.as_ref().and_then(|p| p.evidence.expires_at))
                    .chain(basis.iter().filter_map(|l| l.valid_until))
                    .filter(|t| *t > page.dependencies.evaluated_at)
                    .min();
                ShippingWrite {
                    listing_id: listing.id,
                    listing_revision: listing.revision,
                    source_id: listing.source_id,
                    source_revision: listing.source_revision,
                    integration_current: listing.integration_current,
                    default_revision: default.map(|d| d.revision),
                    profile_reference,
                    profile: view.profile,
                    evidence: listing.evidence,
                    locations: basis,
                    location_references: referenced,
                    summary: derive(&view.effective, &locations),
                    valid_until,
                }
            })
            .collect::<Vec<_>>();
        let changed = self.store.in_transaction(&mut tx).apply(&writes).await?;
        self.store
            .in_transaction(&mut tx)
            .checkpoint(&claim, &page)
            .await?;
        tx.commit().await?;
        tracing::info!(
            event = "shipping.projection.completed",
            changed,
            complete = page.complete
        );
        Ok(changed)
    }
}
