//! Ordered synchronous integration batches. Each existing item use case owns its transaction.
use super::{
    create_product_listing::*, update_product_listing::*, upsert_product_listing::*,
    withdraw_product_listing::*,
};
use application::operation_context::{OperationContext, Principal};
use product_listing_core::product_listing_id::ProductListingKey;
use std::{future::Future, sync::Arc};

pub const MAX_PRODUCT_LISTING_BATCH_SIZE: usize = 100;

#[derive(Debug, thiserror::Error)]
pub enum ProductListingBatchInputError {
    #[error("authenticated actor required to process product listing batch")]
    AuthenticationRequired,
    #[error("Body cannot contain more than {MAX_PRODUCT_LISTING_BATCH_SIZE} products.")]
    TooManyItems,
}

#[derive(Debug)]
pub struct ProductListingBatchItem<T, E> {
    pub index: usize,
    pub product_key: ProductListingKey,
    pub result: Result<T, E>,
}

#[derive(Debug)]
pub struct ProductListingBatchResult<T, E> {
    pub items: Vec<ProductListingBatchItem<T, E>>,
}

pub type CreateProductListingsResult =
    ProductListingBatchResult<CreateProductListingResult, CreateProductListingError>;
pub type UpdateProductListingsResult =
    ProductListingBatchResult<UpdateProductListingResult, UpdateProductListingError>;
pub type UpsertProductListingsResult =
    ProductListingBatchResult<UpsertProductListingResult, UpsertProductListingError>;
pub type WithdrawProductListingsResult =
    ProductListingBatchResult<WithdrawProductListingResult, WithdrawProductListingError>;

#[async_trait::async_trait]
pub trait PartnerProductListingBatchesUseCase: Send + Sync {
    async fn create(
        &self,
        context: &OperationContext,
        commands: Vec<CreateProductListingCommand>,
    ) -> Result<CreateProductListingsResult, ProductListingBatchInputError>;
    async fn update(
        &self,
        context: &OperationContext,
        commands: Vec<(ProductListingKey, UpdateProductListingCommand)>,
    ) -> Result<UpdateProductListingsResult, ProductListingBatchInputError>;
    async fn upsert(
        &self,
        context: &OperationContext,
        commands: Vec<UpsertProductListingCommand>,
    ) -> Result<UpsertProductListingsResult, ProductListingBatchInputError>;
    async fn withdraw(
        &self,
        context: &OperationContext,
        keys: Vec<ProductListingKey>,
    ) -> Result<WithdrawProductListingsResult, ProductListingBatchInputError>;
}

pub struct PartnerProductListingBatchesHandler {
    create: Arc<dyn CreateProductListingUseCase>,
    update: Arc<dyn UpdateProductListingUseCase>,
    upsert: Arc<dyn UpsertProductListingUseCase>,
    withdraw: Arc<dyn WithdrawProductListingUseCase>,
}

impl PartnerProductListingBatchesHandler {
    pub fn new(
        create: Arc<dyn CreateProductListingUseCase>,
        update: Arc<dyn UpdateProductListingUseCase>,
        upsert: Arc<dyn UpsertProductListingUseCase>,
        withdraw: Arc<dyn WithdrawProductListingUseCase>,
    ) -> Self {
        Self {
            create,
            update,
            upsert,
            withdraw,
        }
    }
}

#[async_trait::async_trait]
impl PartnerProductListingBatchesUseCase for PartnerProductListingBatchesHandler {
    async fn create(
        &self,
        context: &OperationContext,
        commands: Vec<CreateProductListingCommand>,
    ) -> Result<CreateProductListingsResult, ProductListingBatchInputError> {
        execute_batch(
            context,
            commands,
            |command| {
                ProductListingKey::new(command.listing_source_id, command.source_listing_id.clone())
            },
            |command| self.create.execute(context, command),
        )
        .await
    }
    async fn update(
        &self,
        context: &OperationContext,
        commands: Vec<(ProductListingKey, UpdateProductListingCommand)>,
    ) -> Result<UpdateProductListingsResult, ProductListingBatchInputError> {
        execute_batch(
            context,
            commands,
            |(key, _)| key.clone(),
            |(key, command)| self.update.execute_by_key(context, key, command),
        )
        .await
    }
    async fn upsert(
        &self,
        context: &OperationContext,
        commands: Vec<UpsertProductListingCommand>,
    ) -> Result<UpsertProductListingsResult, ProductListingBatchInputError> {
        execute_batch(
            context,
            commands,
            |command| {
                ProductListingKey::new(command.listing_source_id, command.source_listing_id.clone())
            },
            |command| self.upsert.execute(context, command),
        )
        .await
    }
    async fn withdraw(
        &self,
        context: &OperationContext,
        keys: Vec<ProductListingKey>,
    ) -> Result<WithdrawProductListingsResult, ProductListingBatchInputError> {
        execute_batch(context, keys, Clone::clone, |key| {
            self.withdraw.execute_by_key(context, key)
        })
        .await
    }
}

async fn execute_batch<C, T, E, F: Future<Output = Result<T, E>>>(
    context: &OperationContext,
    commands: Vec<C>,
    key: impl Fn(&C) -> ProductListingKey,
    execute: impl Fn(C) -> F,
) -> Result<ProductListingBatchResult<T, E>, ProductListingBatchInputError> {
    if matches!(context.principal, Principal::Anonymous) {
        return Err(ProductListingBatchInputError::AuthenticationRequired);
    }
    if commands.len() > MAX_PRODUCT_LISTING_BATCH_SIZE {
        return Err(ProductListingBatchInputError::TooManyItems);
    }
    let mut items = Vec::with_capacity(commands.len());
    // Await every item before the next, including duplicate keys. Failures never undo prior commits.
    for (index, command) in commands.into_iter().enumerate() {
        let product_key = key(&command);
        items.push(ProductListingBatchItem {
            index,
            product_key,
            result: execute(command).await,
        });
    }
    Ok(ProductListingBatchResult { items })
}

#[cfg(test)]
mod tests {
    use super::*;
    use application::patch_field::PatchField;
    use application::{
        error::static_error,
        operation_context::{CorrelationId, Principal, RequestId},
    };
    use domain_primitives::change_outcome::ChangeOutcome;
    use listing_source_core::ListingSourceId;
    use product_listing_core::{
        product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
        source_listing_id::SourceListingId,
    };
    use std::sync::Mutex;

    #[derive(Default)]
    struct Items {
        attempts: Mutex<Vec<String>>,
        commits: Mutex<Vec<String>>,
    }
    impl Items {
        fn apply(&self, key: &str) -> bool {
            let mut attempts = self.attempts.lock().unwrap();
            attempts.push(key.to_owned());
            if attempts.len() == 2 {
                return false;
            }
            self.commits.lock().unwrap().push(key.to_owned());
            true
        }
    }
    fn created() -> CreateProductListingResult {
        CreateProductListingResult {
            product_listing_id: ProductListingId::new(),
            product_listing_title_slug_id: ProductListingSlugId::raw("listing-a1b2c3").unwrap(),
        }
    }
    #[async_trait::async_trait]
    impl CreateProductListingUseCase for Items {
        async fn execute(
            &self,
            _: &OperationContext,
            command: CreateProductListingCommand,
        ) -> Result<CreateProductListingResult, CreateProductListingError> {
            self.apply(command.source_listing_id.as_ref())
                .then(created)
                .ok_or_else(|| {
                    CreateProductListingError::CommitTransactionFailed(static_error(
                        "commit uncertain",
                    ))
                })
        }
    }
    #[async_trait::async_trait]
    impl UpdateProductListingUseCase for Items {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ProductListingId,
            _: UpdateProductListingCommand,
        ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
            unreachable!()
        }
        async fn execute_by_key(
            &self,
            _: &OperationContext,
            key: ProductListingKey,
            _: UpdateProductListingCommand,
        ) -> Result<UpdateProductListingResult, UpdateProductListingError> {
            self.apply(key.source_listing_id.as_ref())
                .then(|| UpdateProductListingResult {
                    product_listing_id: ProductListingId::new(),
                    outcome: ChangeOutcome::Changed,
                })
                .ok_or(UpdateProductListingError::NotFound)
        }
    }
    #[async_trait::async_trait]
    impl UpsertProductListingUseCase for Items {
        async fn execute(
            &self,
            _: &OperationContext,
            command: UpsertProductListingCommand,
        ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
            self.apply(command.source_listing_id.as_ref())
                .then(|| UpsertProductListingResult::Created(created()))
                .ok_or(UpsertProductListingError::Forbidden)
        }
    }
    #[async_trait::async_trait]
    impl WithdrawProductListingUseCase for Items {
        async fn execute(
            &self,
            _: &OperationContext,
            _: ProductListingId,
        ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
            unreachable!()
        }
        async fn execute_by_key(
            &self,
            _: &OperationContext,
            key: ProductListingKey,
        ) -> Result<WithdrawProductListingResult, WithdrawProductListingError> {
            self.apply(key.source_listing_id.as_ref())
                .then(|| WithdrawProductListingResult {
                    product_listing_id: ProductListingId::new(),
                    outcome: ChangeOutcome::Unchanged,
                })
                .ok_or(WithdrawProductListingError::NotFound)
        }
    }
    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::User(user_core::user_id::UserId::new()),
            request_id: RequestId::new("batch"),
            correlation_id: CorrelationId::new("batch"),
        }
    }
    fn handler(items: &Arc<Items>) -> PartnerProductListingBatchesHandler {
        PartnerProductListingBatchesHandler::new(
            items.clone(),
            items.clone(),
            items.clone(),
            items.clone(),
        )
    }
    fn key() -> ProductListingKey {
        ProductListingKey::new(
            ListingSourceId::new(),
            SourceListingId::try_from("duplicate").unwrap(),
        )
    }
    fn command(key: &ProductListingKey) -> CreateProductListingCommand {
        CreateProductListingCommand {
            listing_source_id: key.listing_source_id,
            source_listing_id: key.source_listing_id.clone(),
            title: None,
            description: None,
            pricing: Default::default(),
            availability: None,
            url: "https://example.test/listing".parse().unwrap(),
            images: Default::default(),
            auction: None,
        }
    }
    fn assert_report<T, E>(result: &ProductListingBatchResult<T, E>, items: &Items) {
        assert_eq!(
            vec![0, 1, 2],
            result
                .items
                .iter()
                .map(|item| item.index)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            vec![true, false, true],
            result
                .items
                .iter()
                .map(|item| item.result.is_ok())
                .collect::<Vec<_>>()
        );
        assert_eq!(vec!["duplicate"; 3], *items.attempts.lock().unwrap());
        assert_eq!(vec!["duplicate"; 2], *items.commits.lock().unwrap());
    }
    #[tokio::test]
    async fn should_keep_prior_commits_and_continue_after_failure_for_all_four_operations() {
        let key = key();
        let items = Arc::new(Items::default());
        let result = handler(&items)
            .create(&context(), vec![command(&key); 3])
            .await
            .unwrap();
        assert_report(&result, &items);
        let error = result.items[1].result.as_ref().unwrap_err();
        assert_eq!(
            "commit uncertain",
            std::error::Error::source(error).unwrap().to_string()
        );
        let items = Arc::new(Items::default());
        let result = handler(&items)
            .update(
                &context(),
                vec![(key.clone(), UpdateProductListingCommand::default()); 3],
            )
            .await
            .unwrap();
        assert_report(&result, &items);
        let items = Arc::new(Items::default());
        let create = command(&key);
        let upsert = UpsertProductListingCommand {
            listing_source_id: key.listing_source_id,
            source_listing_id: key.source_listing_id.clone(),
            title: PatchField::Unchanged,
            description: PatchField::Unchanged,
            price: Default::default(),
            price_estimate_min: Default::default(),
            price_estimate_max: Default::default(),
            availability: Default::default(),
            url: Some(create.url),
            images: Default::default(),
            auction: Default::default(),
        };
        let result = handler(&items)
            .upsert(&context(), vec![upsert; 3])
            .await
            .unwrap();
        assert_report(&result, &items);
        let items = Arc::new(Items::default());
        let result = handler(&items)
            .withdraw(&context(), vec![key; 3])
            .await
            .unwrap();
        assert_report(&result, &items);
    }
    #[tokio::test]
    async fn should_authenticate_empty_batches_and_enforce_limit_before_item_calls() {
        let items = Arc::new(Items::default());
        let handler = handler(&items);
        assert!(
            handler
                .withdraw(&context(), vec![])
                .await
                .unwrap()
                .items
                .is_empty()
        );
        let mut anonymous = context();
        anonymous.principal = Principal::Anonymous;
        assert!(matches!(
            handler.withdraw(&anonymous, vec![]).await,
            Err(ProductListingBatchInputError::AuthenticationRequired)
        ));
        assert!(matches!(
            handler.withdraw(&context(), vec![key(); 101]).await,
            Err(ProductListingBatchInputError::TooManyItems)
        ));
        assert!(items.attempts.lock().unwrap().is_empty());
        assert_eq!(
            100,
            handler
                .withdraw(&context(), vec![key(); 100])
                .await
                .unwrap()
                .items
                .len()
        );
    }

    #[tokio::test]
    async fn should_preserve_authenticated_empty_batches_without_calling_item_authorization() {
        let items = Arc::new(Items::default());
        let handler = handler(&items);
        let mut context = context();
        context.principal = Principal::DelegatedUser {
            user_id: user_core::user_id::UserId::new(),
            capabilities: Default::default(),
        };
        assert!(
            handler
                .create(&context, vec![])
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            handler
                .update(&context, vec![])
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            handler
                .upsert(&context, vec![])
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(
            handler
                .withdraw(&context, vec![])
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert!(items.attempts.lock().unwrap().is_empty());
    }
}
