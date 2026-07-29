#![allow(dead_code)]

use crate::event_store::SqlxProductEventStore;
use crate::repository::SqlxProductRepository;
use common::operation_context::OperationContext;
use product_service::ports::product_event_store::ProductEventStore;
use product_service::ports::product_repository::ProductRepository;
use product_service::use_cases::commands::delete_product::{
    DeleteProductCommand, DeleteProductError, DeleteProductResult, DeleteProductUseCase,
};

pub struct PostgresDeleteProductHandler {
    pool: sqlx::PgPool,
}

impl PostgresDeleteProductHandler {
    pub fn new(pool: sqlx::PgPool) -> Self {
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
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(&context.principal.label()),
        );

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| DeleteProductError::BeginTransactionFailed)?;
        let loaded = SqlxProductRepository::new(&mut tx)
            .find_by_id(command.product_id)
            .await?
            .ok_or(DeleteProductError::ProductNotFound)?;
        let expected_event_id = loaded.version;
        let mut product = loaded.value;
        product.delete();
        let events = product.take_pending_events();
        let event_id = events
            .last()
            .map(|event| event.event_id)
            .unwrap_or(expected_event_id);

        if !events.is_empty() {
            SqlxProductRepository::new(&mut tx)
                .update(&product, expected_event_id, event_id)
                .await?;
            for event in &events {
                SqlxProductEventStore::new(&mut tx).append(event).await?;
            }
        }

        tx.commit()
            .await
            .map_err(|_| DeleteProductError::CommitTransactionFailed)?;

        tracing::info!(
            event = "product.deleted",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
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
