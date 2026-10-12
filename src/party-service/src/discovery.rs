//! Public location-oriented discovery. Protected location state is not a read model.
use application::{
    error::BoxError,
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use geo::{CountryCode, SpatialPosition, SubdivisionCode, core::discovery::GeographicQuery};
use geo_service::discovery::{
    GeographicEligibilityError, GeographicQueryGate, with_geographic_eligibility,
};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverPartyLocationsRequest {
    pub geography: GeographicQuery,
    pub cursor: Cursor<PartyLocationDiscoveryCursor>,
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PartyLocationDiscoveryCursor {
    pub location_id: PartyLocationId,
    pub distance_km: Option<f64>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PublicPartyLocation {
    pub party_id: PartyId,
    pub party_name: String,
    pub location_id: PartyLocationId,
    pub label: String,
    pub country: Option<CountryCode>,
    pub subdivision: Option<SubdivisionCode>,
    pub position: Option<SpatialPosition>,
    pub basis: &'static str,
    pub distance_km: Option<f64>,
}
pub type DiscoverPartyLocationsResult =
    CursoredResult<PublicPartyLocation, PartyLocationDiscoveryCursor>;
#[derive(Debug, thiserror::Error)]
pub enum DiscoverPartyLocationsError {
    #[error("invalid location discovery query or cursor")]
    InvalidInput,
    #[error("location discovery failed")]
    Read(#[source] BoxError),
    #[error("invalid location discovery state")]
    InvalidState(#[source] BoxError),
    #[error("location discovery transaction failed")]
    Transaction(
        #[from]
        #[source]
        application::transaction::TransactionError,
    ),
    #[error("location discovery eligibility failed")]
    Eligibility(#[source] GeographicEligibilityError),
}
#[async_trait::async_trait]
pub trait PartyLocationDiscoveryReader: Send {
    async fn discover(
        &mut self,
        request: &DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError>;
}
pub trait PartyLocationDiscoveryReaderFactory<Tx>: Send + Sync {
    fn in_transaction<'tx>(&'tx self, tx: &'tx mut Tx) -> impl PartyLocationDiscoveryReader + 'tx;
}
#[async_trait::async_trait]
pub trait DiscoverPartyLocationsUseCase: Send + Sync {
    async fn execute(
        &self,
        request: DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError>;
}
pub struct DiscoverPartyLocationsHandler<U, R, G> {
    uow: U,
    reader: R,
    gate: G,
}
impl<U, R, G> DiscoverPartyLocationsHandler<U, R, G> {
    pub fn new(uow: U, reader: R, gate: G) -> Self {
        Self { uow, reader, gate }
    }
}
#[async_trait::async_trait]
impl<U, R, G> DiscoverPartyLocationsUseCase for DiscoverPartyLocationsHandler<U, R, G>
where
    U: UnitOfWork,
    R: PartyLocationDiscoveryReaderFactory<U::Tx>,
    G: GeographicQueryGate,
{
    async fn execute(
        &self,
        request: DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
        let ordered = request.geography.distance_order().is_some();
        if !(1..=100).contains(&request.cursor.size)
            || request.cursor.search_after.is_some_and(|c| {
                c.distance_km.is_some() != ordered
                    || c.distance_km.is_some_and(|d| !d.is_finite() || d < 0.0)
            })
        {
            return Err(DiscoverPartyLocationsError::InvalidInput);
        }
        with_geographic_eligibility(&self.gate, || async {
            let result = async {
                let mut tx = self.uow.begin().await?;
                let result = self
                    .reader
                    .in_transaction(&mut tx)
                    .discover(&request)
                    .await?;
                tx.commit().await?;
                Ok(result)
            }
            .await;
            Ok(result)
        })
        .await
        .map_err(DiscoverPartyLocationsError::Eligibility)?
        .unwrap_or_else(|| Ok(Default::default()))
    }
}
