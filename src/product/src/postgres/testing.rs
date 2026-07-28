use crate::core::product_aggregate::Product;
use crate::postgres::event_store::SqlxProductEventStore;
use crate::postgres::repository::SqlxProductRepository;
use crate::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use crate::service::ports::product_repository::{ProductRepository, ProductRepositoryError};
use common::event_id::EventId;
use common::product_id::{ProductId, ProductKey};

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedProductForTest {
    pub product: Product,
    pub current_event_id: EventId,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProductPostgresTestError {
    #[error("concurrent product update")]
    ConcurrencyConflict,
    #[error("product key conflict")]
    ProductKeyConflict,
    #[error("product slug conflict")]
    ProductSlugConflict,
    #[error("product event already exists")]
    EventConflict,
    #[error("invalid product persistence state")]
    InvalidPersistedState,
    #[error("temporary product persistence failure")]
    TemporarilyUnavailable,
    #[error("internal product persistence failure")]
    Internal,
}

pub struct ProductPostgresTestKit<'tx> {
    connection: &'tx mut sqlx::PgConnection,
}

impl<'tx> ProductPostgresTestKit<'tx> {
    pub fn new(connection: &'tx mut sqlx::PgConnection) -> Self {
        Self { connection }
    }

    pub async fn find_by_id(
        &mut self,
        product_id: ProductId,
    ) -> Result<Option<LoadedProductForTest>, ProductPostgresTestError> {
        SqlxProductRepository::new(&mut *self.connection)
            .find_by_id(product_id)
            .await
            .map(|loaded| loaded.map(LoadedProductForTest::from))
            .map_err(ProductPostgresTestError::from)
    }

    pub async fn find_by_key(
        &mut self,
        key: &ProductKey,
    ) -> Result<Option<LoadedProductForTest>, ProductPostgresTestError> {
        SqlxProductRepository::new(&mut *self.connection)
            .find_by_key(key)
            .await
            .map(|loaded| loaded.map(LoadedProductForTest::from))
            .map_err(ProductPostgresTestError::from)
    }

    pub async fn insert(
        &mut self,
        product: &Product,
        current_event_id: EventId,
    ) -> Result<(), ProductPostgresTestError> {
        SqlxProductRepository::new(&mut *self.connection)
            .insert(product, current_event_id)
            .await
            .map_err(ProductPostgresTestError::from)
    }

    pub async fn update(
        &mut self,
        product: &Product,
        expected_event_id: EventId,
        new_event_id: EventId,
    ) -> Result<(), ProductPostgresTestError> {
        SqlxProductRepository::new(&mut *self.connection)
            .update(product, expected_event_id, new_event_id)
            .await
            .map_err(ProductPostgresTestError::from)
    }

    pub async fn append_event(
        &mut self,
        event: &crate::core::product_aggregate::ProductDomainEvent,
    ) -> Result<(), ProductPostgresTestError> {
        SqlxProductEventStore::new(&mut *self.connection)
            .append(event)
            .await
            .map_err(ProductPostgresTestError::from)
    }

    pub async fn find_current_event_id(
        &mut self,
        product_id: ProductId,
    ) -> Result<Option<EventId>, ProductPostgresTestError> {
        SqlxProductEventStore::new(&mut *self.connection)
            .find_current_event_id(product_id)
            .await
            .map_err(ProductPostgresTestError::from)
    }
}

impl From<crate::service::ports::product_repository::LoadedProduct> for LoadedProductForTest {
    fn from(value: crate::service::ports::product_repository::LoadedProduct) -> Self {
        Self {
            product: value.product,
            current_event_id: value.current_event_id,
        }
    }
}

impl From<ProductRepositoryError> for ProductPostgresTestError {
    fn from(error: ProductRepositoryError) -> Self {
        match error {
            ProductRepositoryError::ConcurrencyConflict => Self::ConcurrencyConflict,
            ProductRepositoryError::ProductKeyConflict => Self::ProductKeyConflict,
            ProductRepositoryError::SlugConflict => Self::ProductSlugConflict,
            ProductRepositoryError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ProductRepositoryError::InvalidPersistedState => Self::InvalidPersistedState,
            ProductRepositoryError::Internal => Self::Internal,
        }
    }
}

impl From<ProductEventStoreError> for ProductPostgresTestError {
    fn from(error: ProductEventStoreError) -> Self {
        match error {
            ProductEventStoreError::EventConflict => Self::EventConflict,
            ProductEventStoreError::TemporarilyUnavailable => Self::TemporarilyUnavailable,
            ProductEventStoreError::InvalidEvent => Self::InvalidPersistedState,
            ProductEventStoreError::Internal => Self::Internal,
        }
    }
}
