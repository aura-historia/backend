#![allow(dead_code)]

use crate::core::product_aggregate::Product;
use crate::postgres::event_store::SqlxProductEventStore;
use crate::postgres::repository::SqlxProductRepository;
use crate::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use crate::service::ports::product_repository::{ProductRepository, ProductRepositoryError};
use crate::service::use_cases::commands::create_product::{
    CreateProductCommand, CreateProductError, CreateProductResult, CreateProductUseCase,
};
use common::operation_context::OperationContext;
use common::product_id::ProductId;

pub(crate) struct PostgresCreateProductHandler {
    pool: sqlx::PgPool,
}

impl PostgresCreateProductHandler {
    pub(crate) fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl CreateProductUseCase for PostgresCreateProductHandler {
    #[tracing::instrument(
        name = "create_product",
        skip_all,
        fields(
            shop_id = %command.shop_id,
            shops_product_id = %command.shops_product_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: CreateProductCommand,
    ) -> Result<CreateProductResult, CreateProductError> {
        let actor = super::actor_label(context).ok_or(CreateProductError::Forbidden)?;
        tracing::Span::current().record("actor_id", tracing::field::display(&actor));

        let product = Product::create(command.into_new_product(ProductId::new()))
            .map_err(|_| CreateProductError::InvalidProduct)?;
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductError::Internal)?;

        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        SqlxProductRepository::new(&mut tx)
            .insert(&product, event_id, &actor)
            .await
            .map_err(map_repository_error)?;
        for event in product.pending_events() {
            SqlxProductEventStore::new(&mut tx)
                .append(event, &actor)
                .await
                .map_err(map_event_store_error)?;
        }
        tx.commit().await.map_err(map_sqlx_error)?;

        tracing::info!(
            event = "product.created",
            actor_type = context.principal.kind(),
            actor_id = %actor,
            product_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        CreateProductResult::try_from(&product)
    }
}

fn map_repository_error(error: ProductRepositoryError) -> CreateProductError {
    match error {
        ProductRepositoryError::ProductKeyConflict => CreateProductError::ProductConflict,
        ProductRepositoryError::SlugConflict => CreateProductError::SlugConflict,
        ProductRepositoryError::TemporarilyUnavailable => {
            CreateProductError::TemporarilyUnavailable
        }
        ProductRepositoryError::InvalidPersistedState => CreateProductError::InvalidProduct,
        ProductRepositoryError::ConcurrencyConflict | ProductRepositoryError::Internal => {
            CreateProductError::Internal
        }
    }
}

fn map_event_store_error(error: ProductEventStoreError) -> CreateProductError {
    match error {
        ProductEventStoreError::TemporarilyUnavailable => {
            CreateProductError::TemporarilyUnavailable
        }
        ProductEventStoreError::EventConflict => CreateProductError::ProductConflict,
        ProductEventStoreError::InvalidEvent => CreateProductError::InvalidProduct,
        ProductEventStoreError::Internal => CreateProductError::Internal,
    }
}

fn map_sqlx_error(_error: sqlx::Error) -> CreateProductError {
    CreateProductError::Internal
}
