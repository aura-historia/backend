use crate::{
    ports::{
        AuctionCatalogueCursor, AuctionCatalogueReadError, AuctionCatalogueReadRequest,
        AuctionCatalogueReader, AuctionCatalogueReaderFactory,
    },
    use_cases::{
        PersonalizedProductListingDetailsView, ProductListingDetailsWithUserState,
        present_product_details, redact_hidden_product,
    },
    user_state::ProductListingUserStateDisclosure,
};
use application::{
    error::{BoxError, box_error},
    operation_context::{OperationContext, Principal},
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, UnitOfWork},
};
use auction_core::AuctionId;
use auction_service::ports::{PublicAuctionDetailsReadError, PublicAuctionDetailsReader};
use fxrate_core::FxRateSnapshot;

use fxrate_service::ports::{
    FxRateSnapshotRepository, FxRateSnapshotRepositoryError, FxRateSnapshotRepositoryFactory,
};
use localization::Language;
use money::Currency;
use product_listing_core::{
    listing_availability::ListingAvailability, product_listing::ListingSaleObservation,
};
use std::collections::HashMap;
use time::OffsetDateTime;

const MAX_CATALOGUE_SIZE: u64 = 100;

#[derive(Debug, Clone, PartialEq)]
pub struct GetAuctionCatalogueRequest {
    pub auction_id: AuctionId,
    pub language: Language,
    pub currency: Currency,
    pub cursor: Option<Cursor<AuctionCatalogueCursor>>,
}

#[derive(Debug, thiserror::Error)]
pub enum GetAuctionCatalogueError {
    #[error("shipping enrichment failed")]
    Shipping(
        #[from]
        #[source]
        crate::shipping::ShippingReadError,
    ),
    #[error("Auction not found")]
    AuctionNotFound,
    #[error("Auction catalogue cursor belongs to another Auction")]
    CursorScopeMismatch,
    #[error("Auction catalogue unavailable")]
    TemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("Auction catalogue invalid")]
    InvalidReadModel {
        #[source]
        source: BoxError,
    },
    #[error("no persisted FX snapshot is available for catalogue pricing")]
    PricingFxSnapshotMissing,
    #[error("catalogue price presentation failed")]
    PricingPresentationFailed {
        #[source]
        source: BoxError,
    },
}

#[async_trait::async_trait]
pub trait GetAuctionCatalogueUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAuctionCatalogueRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductListingDetailsView, AuctionCatalogueCursor>,
        GetAuctionCatalogueError,
    >;
}

pub struct GetAuctionCatalogueHandler<U, C, F, A> {
    shipping: crate::shipping::ShippingEnrichment,
    unit_of_work: U,
    catalogue: C,
    fx_rates: F,
    auctions: A,
}

impl<U, C, F, A> GetAuctionCatalogueHandler<U, C, F, A> {
    pub fn new(
        unit_of_work: U,
        catalogue: C,
        fx_rates: F,
        auctions: A,
        shipping: crate::shipping::ShippingEnrichment,
    ) -> Self {
        Self {
            shipping,
            unit_of_work,
            catalogue,
            fx_rates,
            auctions,
        }
    }
}

#[async_trait::async_trait]
impl<U, C, F, A> GetAuctionCatalogueUseCase for GetAuctionCatalogueHandler<U, C, F, A>
where
    U: UnitOfWork,
    C: AuctionCatalogueReaderFactory<U::Tx>,
    F: FxRateSnapshotRepositoryFactory<U::Tx>,
    A: PublicAuctionDetailsReader,
{
    #[tracing::instrument(name = "get_auction_catalogue", skip_all, fields(auction_id = %request.auction_id, principal_type = context.principal.kind(), request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetAuctionCatalogueRequest,
    ) -> Result<
        CursoredResult<PersonalizedProductListingDetailsView, AuctionCatalogueCursor>,
        GetAuctionCatalogueError,
    > {
        self.auctions
            .find_by_id(request.auction_id)
            .await?
            .ok_or(GetAuctionCatalogueError::AuctionNotFound)?;
        let cursor = validate_cursor(request.auction_id, request.cursor)?;

        let mut transaction = self.unit_of_work.begin().await.map_err(|source| {
            GetAuctionCatalogueError::TemporarilyUnavailable {
                source: box_error(source),
            }
        })?;
        let page = self
            .catalogue
            .in_transaction(&mut transaction)
            .list(&AuctionCatalogueReadRequest {
                auction_id: request.auction_id,
                language: request.language,
                user_id: personalization_user_id(&context.principal),
                cursor,
            })
            .await?;
        let valuation_at = OffsetDateTime::now_utc();
        let (mut items, current_snapshot) = present_catalogue_items(
            page.items,
            &self.fx_rates,
            &mut transaction,
            request.currency,
            valuation_at,
        )
        .await?;
        transaction.commit().await.map_err(|source| {
            GetAuctionCatalogueError::TemporarilyUnavailable {
                source: box_error(source),
            }
        })?;

        self.shipping
            .details(
                context,
                &mut items,
                crate::shipping::ShippingReadPurpose::Collection,
                request.currency,
                current_snapshot.as_ref(),
                valuation_at,
            )
            .await?;
        let disclosure = ProductListingUserStateDisclosure::for_principal(&context.principal);
        Ok(CursoredResult {
            items: items
                .into_iter()
                .map(|item| disclosure.disclose(item))
                .collect(),
            cursor: page.cursor,
            total: page.total,
        })
    }
}

async fn present_catalogue_items<Tx, F>(
    factual_items: Vec<crate::ports::PersonalizedProductListingDetailsReadModel>,
    fx_rates: &F,
    transaction: &mut Tx,
    currency: Currency,
    valuation_at: OffsetDateTime,
) -> Result<
    (
        Vec<ProductListingDetailsWithUserState>,
        Option<FxRateSnapshot>,
    ),
    GetAuctionCatalogueError,
>
where
    F: FxRateSnapshotRepositoryFactory<Tx>,
{
    if factual_items.is_empty() {
        return Ok((Vec::new(), None));
    }
    let sale_snapshot_ids = factual_items
        .iter()
        .filter_map(|item| applicable_sale_observation(&item.item).map(|sale| sale.fx_rate_id()))
        .collect::<Vec<_>>();
    let requires_current_snapshot = factual_items
        .iter()
        .any(|item| applicable_sale_observation(&item.item).is_none());
    let mut repository = fx_rates.in_transaction(transaction);
    let current_snapshot = if requires_current_snapshot {
        repository
            .find_latest_at_or_before(valuation_at)
            .await?
            .ok_or(GetAuctionCatalogueError::PricingFxSnapshotMissing)
            .map(Some)?
    } else {
        None
    };
    let sale_snapshots = repository.find_by_ids(&sale_snapshot_ids).await?;
    let sale_snapshots = sale_snapshots
        .into_iter()
        .map(|snapshot| (snapshot.id(), snapshot))
        .collect::<HashMap<_, _>>();

    let items = factual_items
        .into_iter()
        .map(|factual| {
            let snapshot = applicable_sale_observation(&factual.item)
                .map(|sale| {
                    sale_snapshots
                        .get(&sale.fx_rate_id())
                        .ok_or(GetAuctionCatalogueError::PricingFxSnapshotMissing)
                })
                .transpose()?
                .or(current_snapshot.as_ref())
                .ok_or(GetAuctionCatalogueError::PricingFxSnapshotMissing)?;
            let mut view =
                present_product_details(factual, snapshot, currency).map_err(|source| {
                    GetAuctionCatalogueError::PricingPresentationFailed {
                        source: box_error(source),
                    }
                })?;
            if view
                .user_state
                .as_ref()
                .is_some_and(|state| state.search_filter.hidden)
            {
                redact_hidden_product(&mut view.item).map_err(|error| {
                    GetAuctionCatalogueError::PricingPresentationFailed {
                        source: box_error(error),
                    }
                })?;
            }
            Ok(view)
        })
        .collect::<Result<Vec<_>, GetAuctionCatalogueError>>()?;
    Ok((items, current_snapshot))
}

fn validate_cursor(
    auction_id: AuctionId,
    cursor: Option<Cursor<AuctionCatalogueCursor>>,
) -> Result<Cursor<AuctionCatalogueCursor>, GetAuctionCatalogueError> {
    let mut cursor = cursor.unwrap_or_default();
    cursor.size = cursor.size.clamp(1, MAX_CATALOGUE_SIZE);
    if cursor
        .search_after
        .is_some_and(|search_after| search_after.auction_id != auction_id)
    {
        return Err(GetAuctionCatalogueError::CursorScopeMismatch);
    }
    Ok(cursor)
}

fn applicable_sale_observation(
    item: &crate::ports::ProductListingDetailsReadModel,
) -> Option<ListingSaleObservation> {
    if item.availability == Some(ListingAvailability::SoldOut) {
        item.sale_observation
    } else {
        None
    }
}

fn personalization_user_id(principal: &Principal) -> Option<user_core::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}

impl From<AuctionCatalogueReadError> for GetAuctionCatalogueError {
    fn from(value: AuctionCatalogueReadError) -> Self {
        match value {
            AuctionCatalogueReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            AuctionCatalogueReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

impl From<PublicAuctionDetailsReadError> for GetAuctionCatalogueError {
    fn from(value: PublicAuctionDetailsReadError) -> Self {
        match value {
            PublicAuctionDetailsReadError::QueryFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            PublicAuctionDetailsReadError::InvalidReadModel { source } => {
                Self::InvalidReadModel { source }
            }
        }
    }
}

impl From<FxRateSnapshotRepositoryError> for GetAuctionCatalogueError {
    fn from(error: FxRateSnapshotRepositoryError) -> Self {
        match error {
            FxRateSnapshotRepositoryError::ReadFailed { source }
            | FxRateSnapshotRepositoryError::InsertFailed { source } => {
                Self::TemporarilyUnavailable { source }
            }
            FxRateSnapshotRepositoryError::InvalidPersistedSnapshot { source } => {
                Self::InvalidReadModel { source }
            }
            FxRateSnapshotRepositoryError::CapturedAtNotMonotonic => Self::PricingFxSnapshotMissing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::{
        ListingSourceSummary, PersonalizedProductListingDetailsReadModel,
        ProductListingDetailsReadModel,
    };
    use crate::user_state::{ProductListingUserState, ProductListingUserStateView};
    use application::{
        operation_context::{CorrelationId, CredentialCapability, Principal, RequestId},
        personalized::Personalized,
        transaction::TransactionError,
    };
    use auction_core::AuctionSchedule;
    use auction_service::ports::{PublicAuctionDetails, PublicAuctionSourceSummary};
    use fxrate_core::{
        FX_RATE_SCALE, FxRateGeneration, FxRateId, FxRateQuote, FxRateSource, NewFxRateSnapshot,
    };
    use fxrate_service::ports::FxRateSnapshotInsertOutcome;
    use indexmap::IndexSet;
    use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
    use product_listing_core::{
        listing_lifecycle::ListingLifecycle, product_listing::ProductListingPricing,
        product_listing_id::ProductListingId, product_listing_image::ProductListingImage,
        product_listing_slug_id::ProductListingSlugId, source_listing_id::SourceListingId,
    };
    use std::sync::Mutex;
    use strum::IntoEnumIterator;
    use url::Url;

    struct FakeUnitOfWork;
    struct FakeTx;

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            Ok(FakeTx)
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            Ok(())
        }
    }

    struct FakeCatalogue {
        items: Mutex<Vec<PersonalizedProductListingDetailsReadModel>>,
        user_ids: Mutex<Vec<Option<user_core::user_id::UserId>>>,
    }

    struct FakeCatalogueReader<'a>(&'a FakeCatalogue);

    impl AuctionCatalogueReaderFactory<FakeTx> for FakeCatalogue {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl AuctionCatalogueReader + 'tx {
            FakeCatalogueReader(self)
        }
    }

    #[async_trait::async_trait]
    impl AuctionCatalogueReader for FakeCatalogueReader<'_> {
        async fn list(
            &mut self,
            request: &AuctionCatalogueReadRequest,
        ) -> Result<crate::ports::AuctionCataloguePage, AuctionCatalogueReadError> {
            let mut user_ids = self.0.user_ids.lock().unwrap_or_else(|e| e.into_inner());
            user_ids.push(request.user_id);
            let items =
                std::mem::take(&mut *self.0.items.lock().unwrap_or_else(|e| e.into_inner()));
            Ok(CursoredResult {
                items,
                cursor: Cursor::default(),
                total: Some(1),
            })
        }
    }

    struct FakeFxRates(FxRateSnapshot);
    struct FakeFxRateRepository(FxRateSnapshot);

    impl FxRateSnapshotRepositoryFactory<FakeTx> for FakeFxRates {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl FxRateSnapshotRepository + 'tx {
            FakeFxRateRepository(self.0.clone())
        }
    }

    #[async_trait::async_trait]
    impl FxRateSnapshotRepository for FakeFxRateRepository {
        async fn find_latest(
            &mut self,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(self.0.clone()))
        }

        async fn find_latest_at_or_before(
            &mut self,
            _timestamp: OffsetDateTime,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Some(self.0.clone()))
        }

        async fn find_by_id(
            &mut self,
            _id: FxRateId,
        ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(None)
        }

        async fn find_by_ids(
            &mut self,
            _ids: &[FxRateId],
        ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
            Ok(Vec::new())
        }

        async fn insert(
            &mut self,
            _snapshot: &NewFxRateSnapshot,
            _source_event_id: &str,
        ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
            Err(FxRateSnapshotRepositoryError::ReadFailed {
                source: box_error(std::io::Error::other("not used by catalogue reads")),
            })
        }
    }

    struct FakeAuctions(PublicAuctionDetails);

    #[async_trait::async_trait]
    impl PublicAuctionDetailsReader for FakeAuctions {
        async fn find_by_id(
            &self,
            _auction_id: AuctionId,
        ) -> Result<Option<PublicAuctionDetails>, PublicAuctionDetailsReadError> {
            Ok(Some(self.0.clone()))
        }
    }

    fn snapshot() -> Result<FxRateSnapshot, Box<dyn std::error::Error>> {
        let captured = NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| FxRateQuote::new(currency, FX_RATE_SCALE)),
        )?;
        Ok(captured.into_persisted(FxRateGeneration::try_from(1)?))
    }

    fn catalogue_item(
        user_state: ProductListingUserState,
        image_url: &Url,
    ) -> Result<PersonalizedProductListingDetailsReadModel, Box<dyn std::error::Error>> {
        let source = ListingSourceSummary {
            listing_source_id: ListingSourceId::new(),
            name: ListingSourceName::try_from("Source")?,
            slug_id: ListingSourceSlugId::raw("source")?,
        };
        Ok(Personalized {
            item: ProductListingDetailsReadModel {
                product_listing_id: ProductListingId::new(),
                product_listing_title_slug_id: ProductListingSlugId::raw("cabinet-a1b2c3")?,
                event_id: domain_primitives::event_id::EventId::new(),
                source,
                source_listing_id: SourceListingId::try_from("cabinet-1")?,
                product_title: None,
                product_description: None,
                title: None,
                description: None,
                pricing: ProductListingPricing::default(),
                sale_observation: None,
                availability: None,
                lifecycle: ListingLifecycle::Active,
                url: Url::parse("https://shop.example/products/1")?,
                view_url: Url::parse("https://shop.example/products/1")?,
                images: IndexSet::from([ProductListingImage::new(image_url.clone())]),
                content_policy: None,
                auction: None,
                lot: None,
                created: OffsetDateTime::UNIX_EPOCH,
                updated: OffsetDateTime::UNIX_EPOCH,
            },
            user_state: Some(user_state),
        })
    }

    fn auction(auction_id: AuctionId) -> Result<PublicAuctionDetails, Box<dyn std::error::Error>> {
        Ok(PublicAuctionDetails {
            auction_id,
            source: PublicAuctionSourceSummary {
                listing_source_id: ListingSourceId::new(),
                slug_id: ListingSourceSlugId::raw("source")?,
                name: ListingSourceName::try_from("Source")?,
                referral_configuration: None,
            },
            name: None,
            catalogue_url: None,
            format: None,
            schedule: AuctionSchedule::new(None, None, None, None)?,
            reported_status: None,
            reported_lot_count: None,
            visible_active_assigned_listing_count: 1,
        })
    }

    #[tokio::test]
    async fn should_enforce_loaded_state_and_disclose_only_granted_catalogue_sections()
    -> Result<(), Box<dyn std::error::Error>> {
        let image_url = Url::parse("https://example.test/unassessed.jpg")?;
        let mut user_state = ProductListingUserState::default();
        user_state
            .content_visibility
            .show_unassessed_or_sensitive_content = true;
        user_state.watchlist.watching = true;
        user_state.search_filter.matched = true;
        for (capabilities, hidden, expected) in [
            (vec![], false, None),
            (vec![], true, None),
            (
                vec![CredentialCapability::WatchlistRead],
                true,
                Some(ProductListingUserStateView {
                    watchlist: Some(user_state.watchlist),
                    ..Default::default()
                }),
            ),
        ] {
            let user_id = user_core::user_id::UserId::new();
            let auction_id = AuctionId::new();
            let mut state = user_state.clone();
            state.search_filter.hidden = hidden;
            let catalogue = FakeCatalogue {
                items: Mutex::new(vec![catalogue_item(state, &image_url)?]),
                user_ids: Mutex::new(Vec::new()),
            };
            let handler = GetAuctionCatalogueHandler::new(
                FakeUnitOfWork,
                catalogue,
                FakeFxRates(snapshot()?),
                FakeAuctions(auction(auction_id)?),
                crate::shipping::ShippingEnrichment::for_tests(),
            );
            let context = OperationContext {
                principal: Principal::DelegatedUser {
                    user_id,
                    capabilities: capabilities.into_iter().collect(),
                },
                request_id: RequestId::new("request"),
                correlation_id: CorrelationId::new("correlation"),
            };

            let page = handler
                .execute(
                    &context,
                    GetAuctionCatalogueRequest {
                        auction_id,
                        language: Language::En,
                        currency: Currency::Eur,
                        cursor: None,
                    },
                )
                .await?;

            assert_eq!(
                vec![Some(user_id)],
                *handler
                    .catalogue
                    .user_ids
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
            );
            let item = &page.items[0];
            assert_eq!(expected, item.user_state);
            if hidden {
                assert_eq!(None, item.item.product_listing_title_slug_id);
                assert!(item.item.images.is_empty());
            } else {
                assert_eq!(Some(image_url.clone()), item.item.images[0].url.clone());
            }
        }
        Ok(())
    }

    #[test]
    fn should_reject_a_catalogue_cursor_for_another_auction() {
        let requested_auction = AuctionId::new();
        let cursor = Cursor {
            size: 21,
            search_after: Some(AuctionCatalogueCursor {
                auction_id: AuctionId::new(),
                catalogue_position: Some(1),
                product_listing_id: ProductListingId::new(),
            }),
        };

        assert!(matches!(
            validate_cursor(requested_auction, Some(cursor)),
            Err(GetAuctionCatalogueError::CursorScopeMismatch)
        ));
    }

    #[test]
    fn should_bound_a_catalogue_cursor_for_its_auction() {
        let auction_id = AuctionId::new();
        let result = validate_cursor(
            auction_id,
            Some(Cursor {
                size: 0,
                search_after: Some(AuctionCatalogueCursor {
                    auction_id,
                    catalogue_position: None,
                    product_listing_id: ProductListingId::new(),
                }),
            }),
        );

        assert!(matches!(result, Ok(cursor) if cursor.size == 1));
    }
}
