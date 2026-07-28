#![allow(dead_code)]

use crate::postgres::event_store::SqlxProductEventStore;
use crate::postgres::repository::SqlxProductRepository;
use crate::service::ports::product_event_store::{ProductEventStore, ProductEventStoreError};
use crate::service::ports::product_repository::{ProductRepository, ProductRepositoryError};
use crate::service::use_cases::commands::update_product::{
    UpdateProductCommand, UpdateProductError, UpdateProductResult, UpdateProductUseCase,
};
use common::operation_context::OperationContext;
use common::patch_field::PatchField;

pub(crate) struct PostgresUpdateProductHandler {
    pool: sqlx::PgPool,
}

impl PostgresUpdateProductHandler {
    pub(crate) fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl UpdateProductUseCase for PostgresUpdateProductHandler {
    #[tracing::instrument(
        name = "update_product",
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
        command: UpdateProductCommand,
    ) -> Result<UpdateProductResult, UpdateProductError> {
        let actor = super::actor_label(context).ok_or(UpdateProductError::Forbidden)?;
        tracing::Span::current().record("actor_id", tracing::field::display(&actor));

        let mut tx = self.pool.begin().await.map_err(map_sqlx_error)?;
        let loaded = SqlxProductRepository::new(&mut tx)
            .find_by_id(command.product_id)
            .await
            .map_err(map_repository_error)?
            .ok_or(UpdateProductError::NotFound)?;
        let expected_event_id = loaded.current_event_id;
        let mut product = loaded.product;

        apply_command(&mut product, command)?;
        let events = product.take_pending_events();
        let event_id = events.last().map(|event| event.event_id);

        if let Some(new_event_id) = event_id {
            SqlxProductRepository::new(&mut tx)
                .update(&product, expected_event_id, new_event_id, &actor)
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

        if let Some(event_id) = event_id {
            tracing::info!(
                event = "product.updated",
                actor_type = context.principal.kind(),
                actor_id = %actor,
                product_id = %product.id(),
                event_id = %event_id,
                outcome = "success",
            );
        }

        Ok(UpdateProductResult {
            product_id: product.id(),
            event_id,
        })
    }
}

fn apply_command(
    product: &mut crate::core::product_aggregate::Product,
    command: UpdateProductCommand,
) -> Result<(), UpdateProductError> {
    match command.address {
        PatchField::Unchanged => {}
        PatchField::Set(address) => {
            product.replace_address(address);
        }
        PatchField::Clear => {
            product.replace_address(Default::default());
        }
    }
    match command.pricing {
        PatchField::Unchanged => {}
        PatchField::Set(pricing) => {
            product.replace_pricing(pricing);
        }
        PatchField::Clear => {
            product.replace_pricing(Default::default());
        }
    }
    match command.state {
        PatchField::Unchanged => {}
        PatchField::Set(state) => {
            product.change_state(state);
        }
        PatchField::Clear => return Err(UpdateProductError::InvalidProduct),
    }
    match command.url {
        PatchField::Unchanged => {}
        PatchField::Set(url) => {
            product.change_url(url);
        }
        PatchField::Clear => return Err(UpdateProductError::InvalidProduct),
    }
    match command.images {
        PatchField::Unchanged => {}
        PatchField::Set(images) => {
            product.replace_images(images);
        }
        PatchField::Clear => {
            product.replace_images(Default::default());
        }
    }
    match command.embedding {
        PatchField::Unchanged => {}
        PatchField::Set(embedding) => {
            product.replace_embedding(Some(embedding));
        }
        PatchField::Clear => {
            product.replace_embedding(None);
        }
    }
    match command.auction {
        PatchField::Unchanged => {}
        PatchField::Set(auction) => {
            product.replace_auction(auction);
        }
        PatchField::Clear => {
            product.replace_auction(Default::default());
        }
    }

    Ok(())
}

fn map_repository_error(error: ProductRepositoryError) -> UpdateProductError {
    match error {
        ProductRepositoryError::ConcurrencyConflict => UpdateProductError::ConcurrencyConflict,
        ProductRepositoryError::TemporarilyUnavailable => {
            UpdateProductError::TemporarilyUnavailable
        }
        ProductRepositoryError::InvalidPersistedState => UpdateProductError::InvalidProduct,
        ProductRepositoryError::ProductKeyConflict
        | ProductRepositoryError::SlugConflict
        | ProductRepositoryError::Internal => UpdateProductError::Internal,
    }
}

fn map_event_store_error(error: ProductEventStoreError) -> UpdateProductError {
    match error {
        ProductEventStoreError::TemporarilyUnavailable => {
            UpdateProductError::TemporarilyUnavailable
        }
        ProductEventStoreError::InvalidEvent => UpdateProductError::InvalidProduct,
        ProductEventStoreError::EventConflict | ProductEventStoreError::Internal => {
            UpdateProductError::Internal
        }
    }
}

fn map_sqlx_error(_error: sqlx::Error) -> UpdateProductError {
    UpdateProductError::Internal
}
