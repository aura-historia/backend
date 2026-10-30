//! Rebuildable effective geography. No listing is geocoded and no address is copied.
use application::{
    error::{BoxError, FailureKind},
    transaction::{Transaction, UnitOfWork},
};
use geo::{CountryCode, GeographicDescription, SpatialPosition, SubdivisionCode};
use geo_service::resolution::{
    ResolutionReader, ResolutionRepositoryFactory, ResolutionSubject, ResolutionView,
};
use listing_source_core::ListingSourceId;
use party_core::party_location_id::PartyLocationId;
use product_listing_core::product_listing_id::ProductListingId;
use uuid::Uuid;

pub const POLICY: &str = "EFFECTIVE_GEOGRAPHY_V1";
domain_primitives::object_id_newtype!(GeographyWorkId, "gwork");

#[derive(Debug, Clone, PartialEq)]
pub struct GeographicDimension {
    pub location_id: PartyLocationId,
    pub reference_release: geo::reference::ReferenceRelease,
    pub country: Option<CountryCode>,
    pub subdivision: Option<SubdivisionCode>,
    pub position: Option<SpatialPosition>,
}
impl GeographicDimension {
    pub const BASIS: &'static str = "SOURCE_DEFAULT_DERIVED";
}
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectiveListingGeography {
    pub business: Option<GeographicDimension>,
    pub inventory: Option<GeographicDimension>,
}
/// Current authority, locked by the reader for the short projection transaction.
#[derive(Debug, Clone)]
pub struct GeographyInput {
    pub listing_id: ProductListingId,
    pub source_id: ListingSourceId,
    pub listing_version: i64,
    pub active: bool,
    pub assignment_revision: i64,
    pub location_id: Option<PartyLocationId>,
    pub location_revision: Option<i64>,
    pub input_revision: Option<i64>,
    pub location_active: bool,
    pub business: bool,
    pub inventory: bool,
    pub description: Option<GeographicDescription>,
    pub position: Option<SpatialPosition>,
}
#[derive(Debug, Clone)]
pub struct GeographyClaim {
    pub work_id: GeographyWorkId,
    pub generation: i64,
    pub token: Uuid,
    pub kind: String,
    pub dependency_id: String,
    pub after_source: Option<Uuid>,
    pub after_listing: Option<Uuid>,
}
#[derive(Debug, Clone, Default)]
pub struct GeographyPage {
    pub inputs: Vec<GeographyInput>,
    pub after_source: Option<Uuid>,
    pub after_listing: Option<Uuid>,
    pub complete: bool,
}
#[derive(Debug, Clone)]
pub struct GeographyWrite {
    pub input: GeographyInput,
    pub resolution: Option<ResolutionView>,
    pub geography: EffectiveListingGeography,
}
#[derive(Debug, thiserror::Error)]
pub enum GeographyError {
    #[error("effective geography persistence failed")]
    Storage(#[source] BoxError),
    #[error("effective geography transaction failed")]
    Transaction(
        #[from]
        #[source]
        application::transaction::TransactionError,
    ),
    #[error("effective geography resolution read failed")]
    Resolution(
        #[from]
        #[source]
        geo_service::resolution::ResolutionError,
    ),
    #[error("effective geography claim was superseded or expired")]
    Superseded,
    #[error("effective geography work is claimed by another worker")]
    Deferred,
    #[error("invalid effective geography input")]
    InvalidInput,
}
impl GeographyError {
    /// The bounded label of the step that failed, shared by every worker boundary.
    pub fn failure_stage(&self) -> &'static str {
        match self {
            Self::Storage(_) | Self::Superseded | Self::Deferred | Self::InvalidInput => {
                "geography_projection"
            }
            Self::Transaction(_) => "geography_transaction",
            Self::Resolution(_) => "geography_resolution",
        }
    }

    /// The bounded kind every worker boundary records for this failure.
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Storage(_) | Self::Transaction(_) => FailureKind::Storage,
            Self::Resolution(error) => error.failure_kind(),
            Self::Superseded => FailureKind::Rejected("claim_superseded"),
            Self::Deferred => FailureKind::Rejected("claimed_elsewhere"),
            Self::InvalidInput => FailureKind::Rejected("invalid_input"),
        }
    }
}
#[async_trait::async_trait]
pub trait GeographyProjectionStore: Send {
    async fn claim(
        &mut self,
        work: Option<GeographyWorkId>,
    ) -> Result<Option<GeographyClaim>, GeographyError>;
    async fn page(
        &mut self,
        claim: &GeographyClaim,
        limit: u16,
    ) -> Result<GeographyPage, GeographyError>;
    async fn apply(&mut self, writes: &[GeographyWrite]) -> Result<u32, GeographyError>;
    async fn checkpoint(
        &mut self,
        claim: &GeographyClaim,
        page: &GeographyPage,
    ) -> Result<(), GeographyError>;
}
pub trait GeographyProjectionStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl GeographyProjectionStore + 'tx;
}
#[async_trait::async_trait]
pub trait ProjectListingGeographyUseCase: Send + Sync {
    /// One bounded page; None recovers durable work independently of wake-up delivery.
    async fn execute(
        &self,
        work: Option<GeographyWorkId>,
        limit: u16,
    ) -> Result<u32, GeographyError>;
}
pub struct ProjectListingGeographyHandler<U, S, R> {
    uow: U,
    store: S,
    resolutions: R,
}
impl<U, S, R> ProjectListingGeographyHandler<U, S, R> {
    pub fn new(uow: U, store: S, resolutions: R) -> Self {
        Self {
            uow,
            store,
            resolutions,
        }
    }
}
#[async_trait::async_trait]
impl<U, S, R> ProjectListingGeographyUseCase for ProjectListingGeographyHandler<U, S, R>
where
    U: UnitOfWork,
    S: GeographyProjectionStoreFactory<U::Tx>,
    R: ResolutionRepositoryFactory<U::Tx>,
{
    async fn execute(
        &self,
        work: Option<GeographyWorkId>,
        limit: u16,
    ) -> Result<u32, GeographyError> {
        if !(1..=100).contains(&limit) {
            return Err(GeographyError::InvalidInput);
        }
        let mut tx = self.uow.begin().await?;
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
        let mut location_ids = page
            .inputs
            .iter()
            .filter_map(|i| i.location_id)
            .collect::<Vec<_>>();
        location_ids.sort_unstable();
        location_ids.dedup();
        let subjects = location_ids
            .into_iter()
            .map(|id| {
                ResolutionSubject::new(
                    geo_service::resolution::RetentionPurpose::PartyLocation,
                    id.into_uuid(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        let resolutions = self
            .resolutions
            .resolution_reader(&mut tx)
            .inspect_many(&subjects)
            .await?;
        if resolutions.len() != subjects.len() {
            return Err(
                geo_service::resolution::ResolutionError::InvalidState(Box::new(
                    std::io::Error::other("invalid resolution read cardinality"),
                ))
                .into(),
            );
        }
        let resolutions = subjects
            .iter()
            .zip(resolutions)
            .filter_map(|(subject, resolution)| resolution.map(|view| (subject.id(), view)))
            .collect::<std::collections::HashMap<_, _>>();
        let writes = page
            .inputs
            .iter()
            .map(|input| {
                let resolution = input
                    .location_id
                    .and_then(|id| resolutions.get(&id.into_uuid()))
                    .cloned();
                let geography = derive(input, resolution.as_ref());
                GeographyWrite {
                    input: input.clone(),
                    resolution,
                    geography,
                }
            })
            .collect::<Vec<_>>();
        let changed = self.store.in_transaction(&mut tx).apply(&writes).await?;
        self.store
            .in_transaction(&mut tx)
            .checkpoint(&claim, &page)
            .await?;
        tx.commit().await?;
        Ok(changed)
    }
}
/// Future listing overrides/suppression must replace this selection as a whole.
/// Assertions are preserved; only accepted enrichment fills missing administrative facts.
pub fn derive(
    input: &GeographyInput,
    resolution: Option<&ResolutionView>,
) -> EffectiveListingGeography {
    if !input.active || !input.location_active {
        return EffectiveListingGeography::default();
    }
    let Some(location_id) = input.location_id else {
        return EffectiveListingGeography::default();
    };
    let resolved = resolution
        .filter(|r| {
            Some(r.guard as i64) == input.location_revision
                && Some(r.input_revision as i64) == input.input_revision
        })
        .and_then(ResolutionView::accepted_geography);
    let submitted = input.description.as_ref();
    let description = resolved.and_then(|r| r.description());
    let dimension = GeographicDimension {
        location_id,
        reference_release: submitted
            .or(description)
            .map(|d| d.reference_release())
            .unwrap_or(geo::reference::ReferenceRelease::CURRENT),
        country: submitted
            .and_then(|d| d.country())
            .or_else(|| description.and_then(|d| d.country())),
        subdivision: submitted
            .and_then(|d| d.subdivision())
            .or_else(|| description.and_then(|d| d.subdivision())),
        position: input
            .position
            .or_else(|| resolved.and_then(|r| r.position())),
    };
    if dimension.country.is_none()
        && dimension.subdivision.is_none()
        && dimension.position.is_none()
    {
        return EffectiveListingGeography::default();
    }
    EffectiveListingGeography {
        business: input.business.then(|| dimension.clone()),
        inventory: input.inventory.then_some(dimension),
    }
}

#[async_trait::async_trait]
pub trait EffectiveListingGeographyBatchReader: Send + Sync {
    /// Public-safe dimensions from the current derived read projection.
    async fn read_many(
        &self,
        ids: &[ProductListingId],
    ) -> Result<
        std::collections::HashMap<ProductListingId, EffectiveListingGeography>,
        GeographyError,
    >;
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> GeographyInput {
        GeographyInput {
            listing_id: ProductListingId::new(),
            source_id: ListingSourceId::new(),
            listing_version: 1,
            active: true,
            assignment_revision: 2,
            location_id: Some(PartyLocationId::new()),
            location_revision: Some(1),
            input_revision: Some(1),
            location_active: true,
            business: true,
            inventory: false,
            description: GeographicDescription::new_in_release(
                geo::reference::ReferenceRelease::CURRENT,
                None,
                Some(geo::CountryCode::DEU),
                Some(SubdivisionCode::new("DE-BY").expect("valid subdivision")),
                None,
            )
            .expect("compatible assertions"),
            position: Some(
                SpatialPosition::new(
                    geo::GeoPoint::new(48.1, 11.5).expect("valid point"),
                    geo::SpatialPrecision::Premises,
                    None,
                )
                .expect("valid precision"),
            ),
        }
    }
    #[test]
    fn applicability_and_lifecycle_are_independent_of_resolution() {
        let mut i = input();
        let g = derive(&i, None);
        assert!(g.inventory.is_none());
        assert_eq!(
            Some(geo::CountryCode::DEU),
            g.business.as_ref().and_then(|g| g.country)
        );
        i.inventory = true;
        let g = derive(&i, None);
        assert_eq!(
            i.position,
            g.inventory.as_ref().expect("applicable").position
        );
        assert_eq!(
            Some("DE-BY"),
            g.inventory
                .as_ref()
                .and_then(|g| g.subdivision.as_ref().map(|s| s.as_str()))
        );
        i.location_active = false;
        assert_eq!(EffectiveListingGeography::default(), derive(&i, None));
        i.location_active = true;
        i.description = None;
        i.position = None;
        assert_eq!(EffectiveListingGeography::default(), derive(&i, None));
        i.active = false;
        assert_eq!(EffectiveListingGeography::default(), derive(&i, None));
    }
}
