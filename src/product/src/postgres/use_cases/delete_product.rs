#![allow(dead_code)]

use crate::postgres::event_store::SqlxProductEventStore;
use crate::postgres::repository::SqlxProductRepository;
use crate::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use crate::service::ports::product_repository::{ProductRepository, ProductRepositoryError};
use crate::service::use_cases::commands::delete_product::{
    DeleteProductCommand, DeleteProductError, DeleteProductResult, DeleteProductUseCase,
};
use common::operation_context::OperationContext;

pub(crate) struct PostgresDeleteProductHandler {
    pool: sqlx::PgPool,
}

impl PostgresDeleteProductHandler {
    pub(crate) fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl DeleteProductUseCase for PostgresDeleteProductHandler {
    #[tracing::instrument(
        name = "delete_product",
        skip_all,
        fields(
            product_id = %command.product_id,
            principal_type = context.principal.kind(),
            actor_id = tracing::field::Empty,
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        command: DeleteProductCommand,
    ) -> Result<DeleteProductResult, DeleteProductError> {
        let actor = super::actor_label(context).ok_or(DeleteProductError::Forbidden)?;
        tracing::Span::current().record("actor_id", tracing::field::display(&actor));

        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let loaded = SqlxProductRepository::new(&mut tx)
            .find_by_id(command.product_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(DeleteProductError::NotFound)?;
        let expected_event_id = loaded.current_event_id;
        let mut product = loaded.product;
        product.delete();
        let events = product.take_pending_events();
        let event_id = events
            .last()
            .map(|event| event.event_id)
            .unwrap_or(expected_event_id);

        if !events.is_empty() {
            SqlxProductRepository::new(&mut tx)
                .update(&product, expected_event_id, event_id, &actor)
                .await
                .map_err(map_repository_error)?;
            for event in &events {
                SqlxProductEventStore::new(&mut tx)
                    .append(event, &actor)
                    .await
                    .map_err(map_event_store_error)?;
            }
        }

        tx.commit().await.map_err(map_sqlx_error)?;

        tracing::info!(
            event = "product.deleted",
            actor_type = context.principal.kind(),
            actor_id = %actor,
            product_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        Ok(DeleteProductResult {
            product_id: product.id(),
            event_id,
        })
    }
}

fn map_repository_error(error: ProductRepositoryError) -> DeleteProductError {
    match error {
        ProductRepositoryError::ConcurrencyConflict => DeleteProductError::ConcurrencyConflict,
        ProductRepositoryError::TemporarilyUnavailable => {
            DeleteProductError::TemporarilyUnavailable
        }
        ProductRepositoryError::ProductKeyConflict
        | ProductRepositoryError::SlugConflict
        | ProductRepositoryError::InvalidPersistedState
        | ProductRepositoryError::Internal => DeleteProductError::Internal,
    }
}

fn map_event_store_error(error: ProductEventStoreError) -> DeleteProductError {
    match error {
        ProductEventStoreError::TemporarilyUnavailable => {
            DeleteProductError::TemporarilyUnavailable
        }
        ProductEventStoreError::EventConflict
        | ProductEventStoreError::InvalidEvent
        | ProductEventStoreError::Internal => DeleteProductError::Internal,
    }
}

fn map_sqlx_error(_error: sqlx::Error) -> DeleteProductError {
    DeleteProductError::Internal
}
