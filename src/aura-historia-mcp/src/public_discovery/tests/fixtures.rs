use super::*;
use application::{
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use listing_source_service::{
    ports::{PublicListingSourceSearchReader, PublicListingSourceSearchReaderFactory},
    use_cases::queries::{
        get_public_listing_source_by_slug::{
            GetPublicListingSourceBySlugError, GetPublicListingSourceBySlugRequest,
            GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugUseCase,
        },
        public_listing_source::{PublicListingSourceOperatorSummary, PublicListingSourceSummary},
        search_public_listing_sources::{
            PublicListingSourceSearchPage, PublicListingSourceSearchPosition,
            SearchPublicListingSourcesError, SearchPublicListingSourcesHandler,
            SearchPublicListingSourcesRequest, SearchPublicListingSourcesResult,
            SearchPublicListingSourcesUseCase,
        },
    },
};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId, party_name::PartyName};
use party_service::discovery::*;
use product_listing_service::use_cases::{
    GetPublicCatalogueOverviewError, GetPublicCatalogueOverviewUseCase, PublicCatalogueOverview,
};
use std::sync::Mutex;

#[derive(Clone, Copy, Default)]
pub enum Outcome {
    #[default]
    Success,
    Pending,
    Unavailable,
    Internal,
    Missing,
}

#[derive(Debug)]
pub enum Request {
    Search(OperationContext, SearchPublicListingSourcesRequest),
    Detail(OperationContext, GetPublicListingSourceBySlugRequest),
    Location(DiscoverPartyLocationsRequest),
    Overview(OperationContext),
}

#[derive(Default)]
pub struct Fixture {
    pub requests: Mutex<Vec<Request>>,
    pub outcome: Outcome,
}

impl Fixture {
    pub fn registry(self: &Arc<Self>, budget: PublicListingSourceReadBudget) -> Registry {
        Registry::new(
            operations(
                PublicListingSources {
                    search: self.clone(),
                    get_by_slug: self.clone(),
                },
                budget,
                PublicPartyLocations {
                    discover: self.clone(),
                },
                PublicOverview { get: self.clone() },
            )
            .unwrap(),
        )
        .unwrap()
    }

    fn record(&self, request: Request) {
        self.requests.lock().unwrap().push(request);
    }
}

#[async_trait::async_trait]
impl SearchPublicListingSourcesUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: SearchPublicListingSourcesRequest,
    ) -> Result<SearchPublicListingSourcesResult, SearchPublicListingSourcesError> {
        self.record(Request::Search(context.clone(), request.clone()));
        match self.outcome {
            Outcome::Pending => std::future::pending().await,
            Outcome::Unavailable => Err(SearchPublicListingSourcesError::TemporarilyUnavailable {
                source: static_error("private provider detail"),
            }),
            Outcome::Internal => Err(SearchPublicListingSourcesError::InvalidReadModel {
                source: static_error("private provider detail"),
            }),
            _ => {
                SearchPublicListingSourcesHandler::new(NoopUnitOfWork, SourceReader)
                    .execute(context, request)
                    .await
            }
        }
    }
}

#[async_trait::async_trait]
impl GetPublicListingSourceBySlugUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetPublicListingSourceBySlugRequest,
    ) -> Result<GetPublicListingSourceBySlugResult, GetPublicListingSourceBySlugError> {
        self.record(Request::Detail(context.clone(), request.clone()));
        match self.outcome {
            Outcome::Pending => std::future::pending().await,
            Outcome::Unavailable => {
                Err(GetPublicListingSourceBySlugError::TemporarilyUnavailable {
                    source: static_error("private provider detail"),
                })
            }
            Outcome::Internal => Err(GetPublicListingSourceBySlugError::InvalidReadModel {
                source: static_error("private provider detail"),
            }),
            Outcome::Missing => Err(GetPublicListingSourceBySlugError::NotFound),
            Outcome::Success => Ok(PublicListingSourceSummary {
                listing_source_slug_id: request.slug_id,
                ..source()
            }),
        }
    }
}

#[async_trait::async_trait]
impl DiscoverPartyLocationsUseCase for Fixture {
    async fn execute(
        &self,
        request: DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
        self.record(Request::Location(request.clone()));
        match self.outcome {
            Outcome::Pending => std::future::pending().await,
            Outcome::Unavailable => Err(DiscoverPartyLocationsError::Read(static_error(
                "private provider detail",
            ))),
            Outcome::Internal => Err(DiscoverPartyLocationsError::InvalidState(static_error(
                "private provider detail",
            ))),
            _ => {
                DiscoverPartyLocationsHandler::new(NoopUnitOfWork, LocationReader)
                    .execute(request)
                    .await
            }
        }
    }
}

#[async_trait::async_trait]
impl GetPublicCatalogueOverviewUseCase for Fixture {
    async fn execute(
        &self,
        context: &OperationContext,
    ) -> Result<PublicCatalogueOverview, GetPublicCatalogueOverviewError> {
        self.record(Request::Overview(context.clone()));
        match self.outcome {
            Outcome::Pending => std::future::pending().await,
            Outcome::Unavailable => Err(
                GetPublicCatalogueOverviewError::ReaderTemporarilyUnavailable {
                    source: static_error("private provider detail"),
                },
            ),
            Outcome::Internal => Err(GetPublicCatalogueOverviewError::ReaderInvalidReadModel {
                source: static_error("private provider detail"),
            }),
            Outcome::Missing => Err(GetPublicCatalogueOverviewError::NotYetCalculated),
            Outcome::Success => Ok(PublicCatalogueOverview {
                as_of: time::macros::datetime!(2026-10-19 12:34:56.789 UTC),
                sources_indexed: 12,
                live_objects: 3456,
                countries: 7,
                languages: 5,
                new_objects_last_7_days: 89,
            }),
        }
    }
}

pub fn source_id() -> ListingSourceId {
    "ls_01h455vb4pex5vy7enb1p677vn".parse().unwrap()
}
pub fn party_id() -> PartyId {
    "pty_01h455vb4pex5vy7enb1p677vn".parse().unwrap()
}
pub fn location_id() -> PartyLocationId {
    "ploc_01h455vb4pex5vy7enb1p677vn".parse().unwrap()
}

fn source() -> PublicListingSourceSummary {
    PublicListingSourceSummary {
        listing_source_id: source_id(),
        listing_source_slug_id: ListingSourceSlugId::raw("museum-source").unwrap(),
        name: ListingSourceName::try_from("Museum Source").unwrap(),
        operator: PublicListingSourceOperatorSummary {
            name: PartyName::try_from("Museum Operator").unwrap(),
        },
        url: Some("https://museum.example.test/".parse().unwrap()),
        image: Some("https://museum.example.test/card.jpg".parse().unwrap()),
    }
}

struct NoopUnitOfWork;
struct NoopTransaction;
#[async_trait::async_trait]
impl UnitOfWork for NoopUnitOfWork {
    type Tx = NoopTransaction;
    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        Ok(NoopTransaction)
    }
}
#[async_trait::async_trait]
impl Transaction for NoopTransaction {
    async fn commit(self) -> Result<(), TransactionError> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct SourceReader;
impl PublicListingSourceSearchReaderFactory<NoopTransaction> for SourceReader {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut NoopTransaction,
    ) -> impl PublicListingSourceSearchReader + 'tx {
        *self
    }
}
#[async_trait::async_trait]
impl PublicListingSourceSearchReader for SourceReader {
    async fn search(
        &mut self,
        request: &SearchPublicListingSourcesRequest,
    ) -> Result<
        PublicListingSourceSearchPage,
        listing_source_service::ports::PublicListingSourceSearchReadError,
    > {
        Ok(PublicListingSourceSearchPage {
            items: vec![source()],
            next_position: request.continuation().is_none().then(|| {
                PublicListingSourceSearchPosition::new(
                    if request.query().is_browse() { 0 } else { 1 },
                    "museum source".into(),
                    source_id(),
                )
                .unwrap()
            }),
        })
    }
}

#[derive(Clone, Copy)]
struct LocationReader;
impl PartyLocationDiscoveryReaderFactory<NoopTransaction> for LocationReader {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut NoopTransaction,
    ) -> impl PartyLocationDiscoveryReader + 'tx {
        *self
    }
}
#[async_trait::async_trait]
impl PartyLocationDiscoveryReader for LocationReader {
    async fn discover(
        &mut self,
        request: &DiscoverPartyLocationsRequest,
    ) -> Result<DiscoverPartyLocationsResult, DiscoverPartyLocationsError> {
        let distance_km = request.geography.distance_order().map(|_| 23.5);
        Ok(CursoredResult {
            items: vec![PublicPartyLocation {
                party_id: party_id(),
                party_name: "Museum Operator".into(),
                location_id: location_id(),
                label: "Gallery".into(),
                country: Some(geo::CountryCode::DEU),
                subdivision: Some(geo::SubdivisionCode::new("DE-BY").unwrap()),
                position: Some(
                    geo::SpatialPosition::new(
                        geo::GeoPoint::new(48.137, 11.575).unwrap(),
                        geo::SpatialPrecision::Locality,
                        None,
                    )
                    .unwrap(),
                ),
                basis: "ACCEPTED_DERIVED",
                distance_km,
            }],
            total: None,
            cursor: Cursor {
                size: request.cursor.size,
                search_after: Some(PartyLocationDiscoveryCursor {
                    location_id: location_id(),
                    distance_km,
                }),
            },
        })
    }
}
