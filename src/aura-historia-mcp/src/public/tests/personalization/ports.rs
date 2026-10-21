use super::fixtures::ListingFixture;
use application::{
    operation_context::OperationContext,
    pagination::{Cursor, CursoredResult},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use async_trait::async_trait;
use auction_core::AuctionId;
use auction_service::ports::{
    PublicAuctionDetails, PublicAuctionDetailsReadError, PublicAuctionDetailsReader,
    PublicAuctionSourceSummary,
};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use fxrate_core::{FxRateId, FxRateSnapshot, NewFxRateSnapshot};
use fxrate_service::ports::*;
use listing_source_core::ListingSourceId;
use product_listing_core::{
    content_policy::{ContentPolicyDecision, SensitiveContentCategory},
    delivery::ListingDelivery,
    product_listing_id::ProductListingId,
};
use product_listing_service::{ports::*, use_cases::*};
use serde_json::{Value, json};
use shipping_service::assessment::*;
use std::{collections::HashMap, sync::atomic::Ordering};
use time::OffsetDateTime;
use user_core::{
    user_address::{BrowsingDestination, UserAddressId},
    user_id::UserId,
};
use user_service::address_book::*;

pub(super) struct FixtureTransaction;
#[async_trait]
impl Transaction for FixtureTransaction {
    async fn commit(self) -> Result<(), TransactionError> {
        Ok(())
    }
}
#[async_trait]
impl UnitOfWork for ListingFixture {
    type Tx = FixtureTransaction;
    async fn begin(&self) -> Result<Self::Tx, TransactionError> {
        Ok(FixtureTransaction)
    }
}
impl ProductListingDetailsReaderFactory<FixtureTransaction> for ListingFixture {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut FixtureTransaction,
    ) -> impl ProductListingDetailsReader + 'tx {
        self.clone()
    }
}
#[async_trait]
impl ProductListingDetailsReader for ListingFixture {
    async fn find_details(
        &mut self,
        request: &ProductListingDetailsReadRequest,
    ) -> Result<Option<PersonalizedProductListingDetailsReadModel>, ProductListingDetailsReadError>
    {
        assert!(
            matches!(&request.lookup, ProductListingLookup::ById(id) if *id == self.id())
                || matches!(&request.lookup, ProductListingLookup::ByTitleSlug(slug) if *slug == self.slug())
        );
        Ok(Some(self.details(request.user_id)))
    }
}
impl AuctionCatalogueReaderFactory<FixtureTransaction> for ListingFixture {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut FixtureTransaction,
    ) -> impl AuctionCatalogueReader + 'tx {
        self.clone()
    }
}
#[async_trait]
impl AuctionCatalogueReader for ListingFixture {
    async fn list(
        &mut self,
        request: &AuctionCatalogueReadRequest,
    ) -> Result<AuctionCataloguePage, AuctionCatalogueReadError> {
        Ok(self.page(self.details(request.user_id)))
    }
}
impl FxRateSnapshotRepositoryFactory<FixtureTransaction> for ListingFixture {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut FixtureTransaction,
    ) -> impl FxRateSnapshotRepository + 'tx {
        self.clone()
    }
}
#[async_trait]
impl FxRateSnapshotRepository for ListingFixture {
    async fn find_latest(
        &mut self,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(Some(self.fx_snapshot()))
    }
    async fn find_latest_at_or_before(
        &mut self,
        _: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(Some(self.fx_snapshot()))
    }
    async fn find_by_id(
        &mut self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        assert_eq!(id, self.fx_snapshot().id());
        Ok(Some(self.fx_snapshot()))
    }
    async fn find_by_ids(
        &mut self,
        _: &[FxRateId],
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotRepositoryError> {
        Ok(vec![])
    }
    async fn insert(
        &mut self,
        _: &NewFxRateSnapshot,
        _: &str,
    ) -> Result<FxRateSnapshotInsertOutcome, FxRateSnapshotRepositoryError> {
        panic!("public reads cannot insert snapshots")
    }
}
#[async_trait]
impl FxRateSnapshotReader for ListingFixture {
    async fn find_by_id(
        &self,
        id: FxRateId,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        assert_eq!(id, self.fx_snapshot().id());
        Ok(Some(self.fx_snapshot()))
    }
    async fn find_latest_at_or_before(
        &self,
        _: OffsetDateTime,
    ) -> Result<Option<FxRateSnapshot>, FxRateSnapshotReadError> {
        Ok(Some(self.fx_snapshot()))
    }
    async fn find_latest_at_or_before_many(
        &self,
        _: &[OffsetDateTime],
    ) -> Result<Vec<FxRateSnapshot>, FxRateSnapshotReadError> {
        Ok(vec![self.fx_snapshot()])
    }
}
#[async_trait]
impl ProductListingSearchReader for ListingFixture {
    fn continuation_mode(
        &self,
        _: &Value,
        _: product_listing_core::sort_product_listing_field::SortProductListingField,
    ) -> Result<ProductListingSearchMode, ProductListingSearchReadError> {
        Ok(ProductListingSearchMode::Lexical)
    }
    async fn search(
        &self,
        _: &ProductListingSearchReadRequest,
    ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
        Ok(CursoredResult {
            items: vec![self.search_item()],
            total: Some(1),
            cursor: Cursor {
                size: 1,
                search_after: Some(json!([42, self.id()])),
            },
        })
    }
    async fn search_hybrid(
        &self,
        _: &ProductListingSearchReadRequest,
        _: &[f32],
    ) -> Result<ProductListingSearchReadResult, ProductListingSearchReadError> {
        panic!("fixture has no text query")
    }
}
#[async_trait]
impl EmbeddingGenerator for ListingFixture {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("public reads cannot generate product embeddings")
    }
    async fn embed_search_query(
        &self,
        _: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        panic!("fixture has no text query")
    }
}
#[async_trait]
impl ListingSourceSummaryReader for ListingFixture {
    async fn find_summaries(
        &self,
        ids: &[ListingSourceId],
    ) -> Result<
        HashMap<ListingSourceId, ListingSourceSummaryWithReferral>,
        ListingSourceSummaryReadError,
    > {
        assert_eq!(&[self.source().listing_source_id], ids);
        Ok(HashMap::from([(
            self.source().listing_source_id,
            ListingSourceSummaryWithReferral {
                summary: self.source(),
                referral_configuration: None,
            },
        )]))
    }
}
#[async_trait]
impl ProductListingUserStateReader for ListingFixture {
    async fn find_for_user(
        &self,
        lookup: &ProductListingUserStateLookup,
    ) -> Result<
        HashMap<ProductListingId, product_listing_service::user_state::ProductListingUserState>,
        ProductListingUserStateReadError,
    > {
        assert_eq!(self.user_id, lookup.user_id);
        assert_eq!(vec![self.id()], lookup.product_listing_ids);
        Ok(HashMap::from([(self.id(), self.state())]))
    }
}
#[async_trait]
impl ProductListingContentAssessmentReader for ListingFixture {
    async fn find_current_assessments(
        &self,
        ids: &[ProductListingId],
    ) -> Result<
        HashMap<ProductListingId, ProductListingContentAssessment>,
        ProductListingContentAssessmentReadError,
    > {
        assert_eq!(&[self.id()], ids);
        Ok(HashMap::from([(
            self.id(),
            ProductListingContentAssessment {
                product_listing_id: self.id(),
                source_event_id: self.details(None).item.event_id,
                decision: ContentPolicyDecision::RequiresConsent(
                    SensitiveContentCategory::NaziGermany,
                ),
            },
        )]))
    }
}
impl ProductListingEmbeddingReaderFactory<FixtureTransaction> for ListingFixture {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut FixtureTransaction,
    ) -> impl ProductListingEmbeddingReader + 'tx {
        self.clone()
    }
}
#[async_trait]
impl ProductListingEmbeddingReader for ListingFixture {
    async fn find_embedding(
        &mut self,
        _: &ProductListingEmbeddingLookup,
    ) -> Result<Option<ProductListingEmbedding>, ProductListingEmbeddingReadError> {
        Ok(Some(ProductListingEmbedding {
            product_listing_id: self.id(),
            embedding: Some(vec![1.0]),
        }))
    }
}
#[async_trait]
impl ProductListingSimilarProductListingsReader for ListingFixture {
    async fn find_similar_product_listings(
        &self,
        _: &ProductListingSimilarProductListingsRequest,
    ) -> Result<Vec<ProductListingSearchItem>, ProductListingSimilarProductListingsReadError> {
        Ok(vec![self.search_item()])
    }
}
#[async_trait]
impl PublicAuctionDetailsReader for ListingFixture {
    async fn find_by_id(
        &self,
        auction_id: AuctionId,
    ) -> Result<Option<PublicAuctionDetails>, PublicAuctionDetailsReadError> {
        Ok(Some(PublicAuctionDetails {
            auction_id,
            source: PublicAuctionSourceSummary {
                listing_source_id: self.source().listing_source_id,
                slug_id: self.source().slug_id,
                name: self.source().name,
                referral_configuration: None,
            },
            name: None,
            catalogue_url: None,
            format: None,
            schedule: Default::default(),
            reported_status: None,
            reported_lot_count: None,
            visible_active_assigned_listing_count: 1,
        }))
    }
}
impl AssessmentReaderFactory<FixtureTransaction> for ListingFixture {
    fn in_transaction<'tx>(
        &'tx self,
        _: &'tx mut FixtureTransaction,
    ) -> impl AssessmentReader + 'tx {
        self.clone()
    }
}
#[async_trait]
impl AssessmentReader for ListingFixture {
    async fn snapshot(
        &mut self,
        ids: &[ProductListingId],
        user: Option<UserId>,
    ) -> Result<AssessmentDependencies, AssessmentError> {
        if let Some(user) = user {
            assert_eq!(self.user_id, user);
        }
        Ok(AssessmentDependencies {
            evaluated_at: OffsetDateTime::UNIX_EPOCH,
            listings: ids
                .iter()
                .map(|id| ListingDependency {
                    id: *id,
                    source_id: self.source().listing_source_id,
                    revision: 1,
                    source_revision: 1,
                    operator_id: "pty_01k6k0w000e008000000000003".parse().unwrap(),
                    delivery: ListingDelivery::inherited(self.source().listing_source_id),
                    evidence: None,
                    integration_current: true,
                })
                .collect(),
            profiles: vec![],
            defaults: vec![],
            locations: vec![],
        })
    }
}
#[async_trait]
impl AddressBookQueries for ListingFixture {
    async fn resolve_destination_facts(
        &self,
        context: &OperationContext,
        explicit: Option<BrowsingDestination>,
    ) -> Result<DestinationFacts, AddressBookError> {
        assert!(explicit.is_none());
        assert!(
            context
                .principal
                .require_credential_capability(
                    application::operation_context::CredentialCapability::UsersRead
                )
                .is_ok()
        );
        assert!(
            matches!(context.principal, application::operation_context::Principal::DelegatedUser { user_id, .. } if user_id == self.user_id)
        );
        self.destination_reads.fetch_add(1, Ordering::SeqCst);
        Ok(DestinationFacts {
            submitted: geo::GeographicDescription::new(
                None,
                Some(geo::CountryCode::DEU),
                None,
                None,
            )
            .unwrap(),
            derived: None,
            attribution: None,
        })
    }
    async fn get(
        &self,
        _: &OperationContext,
        _: UserAddressId,
    ) -> Result<UserAddressView, AddressBookError> {
        panic!("no address-detail reads")
    }
    async fn list(
        &self,
        _: &OperationContext,
        _: Option<UserAddressId>,
        _: u32,
    ) -> Result<UserAddressPage, AddressBookError> {
        panic!("no address-list reads")
    }
    async fn destination(&self, _: &OperationContext) -> Result<DestinationView, AddressBookError> {
        panic!("no destination-detail reads")
    }
    async fn resolve_destination(
        &self,
        _: &OperationContext,
        _: Option<BrowsingDestination>,
    ) -> Result<ResolvedDestination, AddressBookError> {
        panic!("only destination facts are requested")
    }
}
