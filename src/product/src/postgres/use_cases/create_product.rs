#![allow(dead_code)]

use crate::core::product_aggregate::Product;
use crate::postgres::event_store::SqlxProductEventStore;
use crate::postgres::repository::SqlxProductRepository;
use crate::service::ports::product_event_store::ProductEventStore;
use crate::service::ports::product_repository::ProductRepository;
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
        tracing::Span::current().record(
            "actor_id",
            tracing::field::display(&context.principal.label()),
        );

        let product = Product::create(command.into_new_product(ProductId::new()))?;
        let event_id = product
            .pending_events()
            .last()
            .map(|event| event.event_id)
            .ok_or(CreateProductError::CreatedEventMissing)?;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CreateProductError::BeginTransactionFailed)?;
        SqlxProductRepository::new(&mut tx)
            .insert(&product, event_id)
            .await?;
        for event in product.pending_events() {
            SqlxProductEventStore::new(&mut tx).append(event).await?;
        }
        tx.commit()
            .await
            .map_err(|_| CreateProductError::CommitTransactionFailed)?;

        tracing::info!(
            event = "product.created",
            actor_type = context.principal.kind(),
            actor_id = %context.principal.label(),
            product_id = %product.id(),
            event_id = %event_id,
            outcome = "success",
        );

        CreateProductResult::try_from(&product)
    }
}
