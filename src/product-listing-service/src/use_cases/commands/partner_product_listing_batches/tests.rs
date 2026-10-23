use super::*;
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
                CreateProductListingError::CommitTransactionFailed(static_error("commit uncertain"))
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
        title: None,
        description: None,
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
