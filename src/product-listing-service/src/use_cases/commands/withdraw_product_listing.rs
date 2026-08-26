use crate::ports::{
    PartnerProductListingAuthorizationError, PartnerProductListingAuthorizer,
    PartnerProductListingAuthorizerFactory, ProductListingEventStore,
    ProductListingEventStoreError, ProductListingEventStoreFactory, ProductListingRepository,
    ProductListingRepositoryError, ProductListingRepositoryFactory, stamp_product_listing_events,
};
use application::error::BoxError;
use application::operation_context::{
    CredentialCapability, OperationAuthorizationError, OperationContext, Principal,
};
use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use product_listing_core::product_listing_id::{ProductListingId, ProductListingKey};
use user_core::user_id::UserId;

#[derive(Debug, Clone, PartialEq)]
pub struct WithdrawProductListingResult {
    pub product_listing_id: ProductListingId,
    pub event_id: EventId,
}

#[derive(Debug, thiserror::Error)]
pub enum WithdrawProductListingError {
    #[error("authenticated actor required to withdraw product listing")]
    AuthenticatedActorRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("shop not found")]
    ShopNotFound,
    #[error("partner product listing authorization is temporarily unavailable")]
    PartnerAuthorizationTemporarilyUnavailable {
        #[source]
        source: BoxError,
    },
    #[error("partner product listing authorization failed internally")]
    PartnerAuthorizationInternal {
        #[source]
        source: BoxError,
    },
    #[error("product listing not found")]
    NotFound,
    #[error("product listing persistence failed")]
    PersistenceFailed,
    #[error("product listing event storage failed")]
    EventStoreFailed,
    #[error("failed to begin withdraw product listing transaction")]
    BeginTransactionFailed,
    #[error("failed to commit withdraw product listing transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait WithdrawProductListingUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError>;
}

pub struct WithdrawProductListingHandler<U, R, E, A> {
    unit_of_work: U,
    products: R,
    events: E,
    authorizer: A,
}
impl<U, R, E, A> WithdrawProductListingHandler<U, R, E, A> {
    pub fn new(unit_of_work: U, products: R, events: E, authorizer: A) -> Self {
        Self {
            unit_of_work,
            products,
            events,
            authorizer,
        }
    }
}

enum WithdrawTarget {
    Id(ProductListingId),
    Key(ProductListingKey),
}

impl<U, R, E, A> WithdrawProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    async fn withdraw(
        &self,
        context: &OperationContext,
        target: WithdrawTarget,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        context
            .require()
            .credential_capability(CredentialCapability::ProductListingsWrite)
            .authorize::<WithdrawProductListingError>()?;
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(context.principal.label()),
        );
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| WithdrawProductListingError::BeginTransactionFailed)?;
        let loaded = match target {
            WithdrawTarget::Id(id) => {
                let loaded = self
                    .products
                    .in_transaction(&mut tx)
                    .find_by_id(id)
                    .await?
                    .ok_or(WithdrawProductListingError::NotFound)?;
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, loaded.value.shop_id())
                        .await?;
                }
                loaded
            }
            WithdrawTarget::Key(key) => {
                if let Some(actor_id) = partner_actor(&context.principal) {
                    self.authorizer
                        .in_transaction(&mut tx)
                        .authorize(actor_id, key.shop_id)
                        .await?;
                }
                self.products
                    .in_transaction(&mut tx)
                    .find_by_key(&key)
                    .await?
                    .ok_or(WithdrawProductListingError::NotFound)?
            }
        };
        let expected_event_id = loaded.version;
        let mut product = loaded.value;
        product.withdraw();
        let events = stamp_product_listing_events(
            product.id(),
            time::OffsetDateTime::now_utc(),
            product.take_pending_event_payloads(),
        );
        let event_id = events
            .last()
            .map(|event| event.event_id)
            .unwrap_or(expected_event_id);
        if !events.is_empty() {
            product = self
                .products
                .in_transaction(&mut tx)
                .update(&product, expected_event_id, event_id)
                .await?
                .value;
            for event in &events {
                self.events.in_transaction(&mut tx).append(event).await?;
            }
        }
        tx.commit()
            .await
            .map_err(|_| WithdrawProductListingError::CommitTransactionFailed)?;
        tracing::info!(event = "product_listing.withdrawn", actor_type = context.principal.kind(), actor_id = %context.principal.label(), product_listing_id = %product.id(), event_id = %event_id, outcome = "success");
        Ok(WithdrawProductListingResult {
            product_listing_id: product.id(),
            event_id,
        })
    }
}

#[async_trait::async_trait]
impl<U, R, E, A> WithdrawProductListingUseCase for WithdrawProductListingHandler<U, R, E, A>
where
    U: UnitOfWork,
    R: ProductListingRepositoryFactory<U::Tx>,
    E: ProductListingEventStoreFactory<U::Tx>,
    A: PartnerProductListingAuthorizerFactory<U::Tx>,
{
    #[tracing::instrument(name = "withdraw_product_listing", skip_all, fields(product_listing_id = %product_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute(
        &self,
        context: &OperationContext,
        product_listing_id: ProductListingId,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        self.withdraw(context, WithdrawTarget::Id(product_listing_id))
            .await
    }

    #[tracing::instrument(name = "withdraw_product_listing_by_key", skip_all, fields(shop_id = %product_key.shop_id, shop_listing_id = %product_key.shop_listing_id, principal_type = context.principal.kind(), actor_id = tracing::field::Empty, request_id = %context.request_id, correlation_id = %context.correlation_id))]
    async fn execute_by_key(
        &self,
        context: &OperationContext,
        product_key: ProductListingKey,
    ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
        self.withdraw(context, WithdrawTarget::Key(product_key))
            .await
    }
}

fn partner_actor(principal: &Principal) -> Option<UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
impl From<OperationAuthorizationError> for WithdrawProductListingError {
    fn from(error: OperationAuthorizationError) -> Self {
        match error {
            OperationAuthorizationError::AuthenticationRequired(_) => {
                Self::AuthenticatedActorRequired
            }
            OperationAuthorizationError::Forbidden
            | OperationAuthorizationError::InsufficientCapability { .. } => Self::Forbidden,
        }
    }
}
impl From<PartnerProductListingAuthorizationError> for WithdrawProductListingError {
    fn from(error: PartnerProductListingAuthorizationError) -> Self {
        match error {
            PartnerProductListingAuthorizationError::ShopNotFound => Self::ShopNotFound,
            PartnerProductListingAuthorizationError::Forbidden => Self::Forbidden,
            PartnerProductListingAuthorizationError::TemporarilyUnavailable { source } => {
                Self::PartnerAuthorizationTemporarilyUnavailable { source }
            }
            PartnerProductListingAuthorizationError::Internal { source } => {
                Self::PartnerAuthorizationInternal { source }
            }
        }
    }
}
impl From<ProductListingRepositoryError> for WithdrawProductListingError {
    fn from(error: ProductListingRepositoryError) -> Self {
        match error {
            ProductListingRepositoryError::ProductListingLookupByIdFailed
            | ProductListingRepositoryError::ProductListingLookupByKeyFailed { .. }
            | ProductListingRepositoryError::ProductListingInsertFailed
            | ProductListingRepositoryError::ProductListingUpdateFailed
            | ProductListingRepositoryError::ProductListingCurrentEventIdConflict
            | ProductListingRepositoryError::ShopListingAlreadyExists
            | ProductListingRepositoryError::ProductListingSlugAlreadyExists
            | ProductListingRepositoryError::InvalidProductListingSlugPersisted
            | ProductListingRepositoryError::IncompleteTitlePersisted
            | ProductListingRepositoryError::InvalidTitleLanguagePersisted
            | ProductListingRepositoryError::IncompleteDescriptionPersisted
            | ProductListingRepositoryError::InvalidDescriptionLanguagePersisted
            | ProductListingRepositoryError::IncompletePricePersisted
            | ProductListingRepositoryError::NegativePriceAmountPersisted
            | ProductListingRepositoryError::InvalidPriceCurrencyPersisted
            | ProductListingRepositoryError::InvalidListingAvailabilityPersisted
            | ProductListingRepositoryError::InvalidListingLifecyclePersisted
            | ProductListingRepositoryError::InvalidProductListingUrlPersisted
            | ProductListingRepositoryError::InvalidProductListingImagesPersisted
            | ProductListingRepositoryError::InvalidProductListingImageUrlPersisted
            | ProductListingRepositoryError::InvalidProductListingImageProhibitedContentPersisted
            | ProductListingRepositoryError::InvalidAggregateStatePersisted => {
                Self::PersistenceFailed
            }
        }
    }
}
impl From<ProductListingEventStoreError> for WithdrawProductListingError {
    fn from(_: ProductListingEventStoreError) -> Self {
        Self::EventStoreFailed
    }
}
