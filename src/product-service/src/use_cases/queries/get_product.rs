use crate::ports::{
    ProductDetailsReadError, ProductDetailsReader, ProductDetailsReaderFactory,
    ProductTranslationReadError, ProductTranslationReader, ProductTranslationReaderFactory,
};
use common::currency::domain::Currency;
use common::event_id::EventId;
use common::language::domain::Language;
use common::localized::Localized;
use common::operation_context::OperationContext;
use common::price::domain::Price;
use common::product_id::ProductId;
use common::product_lifecycle::domain::ProductLifecycle;
use common::product_slug_id::ProductSlugId;
use common::product_state::domain::ProductState;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use common::transaction::{Transaction, UnitOfWork};
use indexmap::IndexSet;
use product_core::description::Description;
use product_core::product::{ProductAddress, ProductAuction, ProductPricing};
use product_core::product_image::ProductImage;
use product_core::title::Title;
use time::OffsetDateTime;
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum ProductLookup {
    ById(ProductId),
    BySlug {
        shop_slug_id: ShopSlugId,
        product_slug_id: ProductSlugId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct GetProductRequest {
    pub lookup: ProductLookup,
    pub language: Language,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProductDetailsView {
    pub product_id: ProductId,
    pub product_slug_id: ProductSlugId,
    pub event_id: EventId,
    pub shop_id: ShopId,
    pub seller_id: ShopId,
    pub shops_product_id: ShopsProductId,
    pub shop_name: ShopName,
    pub seller_name: ShopName,
    pub shop_slug_id: ShopSlugId,
    pub seller_slug_id: ShopSlugId,
    pub address: ProductAddress,
    pub product_title: Option<Localized<Language, Title>>,
    pub product_description: Option<Localized<Language, Description>>,
    pub title: Option<Localized<Language, Title>>,
    pub description: Option<Localized<Language, Description>>,
    pub pricing: ProductPricing,
    pub price: Option<Price>,
    pub price_estimate_min: Option<Price>,
    pub price_estimate_max: Option<Price>,
    pub currency: Option<Currency>,
    pub state: ProductState,
    pub lifecycle: ProductLifecycle,
    pub url: Url,
    pub view_url: Url,
    pub images: IndexSet<ProductImage>,
    pub auction: ProductAuction,
    pub created: OffsetDateTime,
    pub updated: OffsetDateTime,
}

#[derive(Debug, thiserror::Error)]
pub enum GetProductError {
    #[error("product not found")]
    NotFound,
    #[error("product details query failed")]
    ProductDetailsQueryFailed,
    #[error("product details read model is invalid")]
    ProductDetailsReadModelInvalid,
    #[error("product translation lookup failed")]
    ProductTranslationLookupFailed,
    #[error("product translation read model is invalid")]
    ProductTranslationReadModelInvalid,
    #[error("failed to begin get product transaction")]
    BeginTransactionFailed,
    #[error("failed to commit get product transaction")]
    CommitTransactionFailed,
}

#[async_trait::async_trait]
pub trait GetProductUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductRequest,
    ) -> Result<ProductDetailsView, GetProductError>;
}

pub struct GetProductHandler<U, D, T> {
    unit_of_work: U,
    details_reader: D,
    translation_reader: T,
}

impl<U, D, T> GetProductHandler<U, D, T> {
    pub fn new(unit_of_work: U, details_reader: D, translation_reader: T) -> Self {
        Self {
            unit_of_work,
            details_reader,
            translation_reader,
        }
    }
}

#[async_trait::async_trait]
impl<U, D, T> GetProductUseCase for GetProductHandler<U, D, T>
where
    U: UnitOfWork,
    D: ProductDetailsReaderFactory<U::Tx>,
    T: ProductTranslationReaderFactory<U::Tx>,
{
    #[tracing::instrument(
        name = "get_product",
        skip_all,
        fields(
            principal_type = context.principal.kind(),
            request_id = %context.request_id,
            correlation_id = %context.correlation_id,
        )
    )]
    async fn execute(
        &self,
        context: &OperationContext,
        request: GetProductRequest,
    ) -> Result<ProductDetailsView, GetProductError> {
        let mut tx = self
            .unit_of_work
            .begin()
            .await
            .map_err(|_| GetProductError::BeginTransactionFailed)?;
        let mut details = self
            .details_reader
            .in_transaction(&mut tx)
            .find_details(&request)
            .await?
            .ok_or(GetProductError::NotFound)?;
        let translations = self
            .translation_reader
            .in_transaction(&mut tx)
            .find_for_product(details.product_id)
            .await?;

        details.title =
            translations.resolve_title(details.product_title.clone(), &[request.language]);
        details.description = translations
            .resolve_description(details.product_description.clone(), &[request.language]);

        tx.commit()
            .await
            .map_err(|_| GetProductError::CommitTransactionFailed)?;

        Ok(details)
    }
}

impl From<ProductDetailsReadError> for GetProductError {
    fn from(error: ProductDetailsReadError) -> Self {
        match error {
            ProductDetailsReadError::ProductDetailsQueryFailed => Self::ProductDetailsQueryFailed,
            ProductDetailsReadError::ProductDetailsReadModelInvalid => {
                Self::ProductDetailsReadModelInvalid
            }
        }
    }
}

impl From<ProductTranslationReadError> for GetProductError {
    fn from(error: ProductTranslationReadError) -> Self {
        match error {
            ProductTranslationReadError::ProductTranslationLookupFailed => {
                Self::ProductTranslationLookupFailed
            }
            ProductTranslationReadError::ProductTranslationReadModelInvalid => {
                Self::ProductTranslationReadModelInvalid
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::ProductTranslationsView;
    use common::operation_context::{CorrelationId, Principal, RequestId};
    use common::price::domain::MonetaryAmount;
    use common::transaction::TransactionError;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, MutexGuard};

    #[derive(Debug, Default)]
    struct FakeState {
        begin_error: bool,
        commit_error: bool,
        find_details_result: Option<Result<Option<ProductDetailsView>, ProductDetailsReadError>>,
        find_translations_result:
            Option<Result<ProductTranslationsView, ProductTranslationReadError>>,
        requested_translation_product_ids: Vec<ProductId>,
        commit_count: usize,
    }

    type SharedState = Arc<Mutex<FakeState>>;

    #[derive(Clone)]
    struct FakeUnitOfWork {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeDetailsReaderFactory {
        state: SharedState,
    }

    #[derive(Clone)]
    struct FakeTranslationReaderFactory {
        state: SharedState,
    }

    struct FakeTx {
        state: SharedState,
    }

    struct FakeDetailsReader {
        state: SharedState,
    }

    struct FakeTranslationReader {
        state: SharedState,
    }

    fn state() -> SharedState {
        Arc::new(Mutex::new(FakeState::default()))
    }

    fn lock_state(state: &SharedState) -> MutexGuard<'_, FakeState> {
        match state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    #[async_trait::async_trait]
    impl UnitOfWork for FakeUnitOfWork {
        type Tx = FakeTx;

        async fn begin(&self) -> Result<Self::Tx, TransactionError> {
            if lock_state(&self.state).begin_error {
                Err(TransactionError::BeginFailed)
            } else {
                Ok(FakeTx {
                    state: Arc::clone(&self.state),
                })
            }
        }
    }

    #[async_trait::async_trait]
    impl Transaction for FakeTx {
        async fn commit(self) -> Result<(), TransactionError> {
            let mut state = lock_state(&self.state);
            state.commit_count += 1;
            if state.commit_error {
                Err(TransactionError::CommitFailed)
            } else {
                Ok(())
            }
        }
    }

    impl ProductDetailsReaderFactory<FakeTx> for FakeDetailsReaderFactory {
        fn in_transaction<'tx>(&'tx self, _tx: &'tx mut FakeTx) -> impl ProductDetailsReader + 'tx {
            FakeDetailsReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    impl ProductTranslationReaderFactory<FakeTx> for FakeTranslationReaderFactory {
        fn in_transaction<'tx>(
            &'tx self,
            _tx: &'tx mut FakeTx,
        ) -> impl ProductTranslationReader + 'tx {
            FakeTranslationReader {
                state: Arc::clone(&self.state),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductDetailsReader for FakeDetailsReader {
        async fn find_details(
            &mut self,
            _request: &GetProductRequest,
        ) -> Result<Option<ProductDetailsView>, ProductDetailsReadError> {
            let mut state = lock_state(&self.state);
            match state.find_details_result.take() {
                Some(result) => result,
                None => Ok(None),
            }
        }
    }

    #[async_trait::async_trait]
    impl ProductTranslationReader for FakeTranslationReader {
        async fn find_for_product(
            &mut self,
            product_id: ProductId,
        ) -> Result<ProductTranslationsView, ProductTranslationReadError> {
            let mut state = lock_state(&self.state);
            state.requested_translation_product_ids.push(product_id);
            match state.find_translations_result.take() {
                Some(result) => result,
                None => Ok(ProductTranslationsView {
                    product_id,
                    titles: HashMap::new(),
                    descriptions: HashMap::new(),
                }),
            }
        }
    }

    fn handler(
        state: &SharedState,
    ) -> GetProductHandler<FakeUnitOfWork, FakeDetailsReaderFactory, FakeTranslationReaderFactory>
    {
        GetProductHandler::new(
            FakeUnitOfWork {
                state: Arc::clone(state),
            },
            FakeDetailsReaderFactory {
                state: Arc::clone(state),
            },
            FakeTranslationReaderFactory {
                state: Arc::clone(state),
            },
        )
    }

    fn context() -> OperationContext {
        OperationContext {
            principal: Principal::System,
            request_id: RequestId::new("request"),
            correlation_id: CorrelationId::new("correlation"),
        }
    }

    fn request(language: Language) -> GetProductRequest {
        GetProductRequest {
            lookup: ProductLookup::ById(ProductId::new()),
            language,
        }
    }

    fn url(value: &str) -> Result<Url, url::ParseError> {
        Url::parse(value)
    }

    fn details_view() -> Result<ProductDetailsView, url::ParseError> {
        let product_id = ProductId::new();
        Ok(ProductDetailsView {
            product_id,
            product_slug_id: ProductSlugId::from("cabinet-abcdef"),
            event_id: EventId::new(),
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::new(),
            shop_name: ShopName::from("Shop"),
            seller_name: ShopName::from("Seller"),
            shop_slug_id: ShopSlugId::from("shop"),
            seller_slug_id: ShopSlugId::from("seller"),
            address: ProductAddress::default(),
            product_title: Some(Localized::new(Language::En, Title::from("Cabinet"))),
            product_description: Some(Localized::new(Language::En, Description::from("Native"))),
            title: None,
            description: None,
            pricing: ProductPricing::default(),
            price: Some(Price::new(MonetaryAmount::from(100_u64), Currency::Eur)),
            price_estimate_min: None,
            price_estimate_max: None,
            currency: Some(Currency::Eur),
            state: ProductState::Listed,
            lifecycle: ProductLifecycle::Active,
            url: url("https://shop.example/products/1")?,
            view_url: url("https://aura.example/products/cabinet-abcdef")?,
            images: IndexSet::<ProductImage>::new(),
            auction: ProductAuction::default(),
            created: OffsetDateTime::UNIX_EPOCH,
            updated: OffsetDateTime::UNIX_EPOCH,
        })
    }

    fn translations(product_id: ProductId) -> ProductTranslationsView {
        ProductTranslationsView {
            product_id,
            titles: HashMap::from([(Language::De, Title::from("Schrank"))]),
            descriptions: HashMap::from([(Language::De, Description::from("Deutsch"))]),
        }
    }

    #[tokio::test]
    async fn should_resolve_requested_current_product_translation() -> Result<(), url::ParseError> {
        let state = state();
        let view = details_view()?;
        let product_id = view.product_id;
        lock_state(&state).find_details_result = Some(Ok(Some(view)));
        lock_state(&state).find_translations_result = Some(Ok(translations(product_id)));

        let result = handler(&state)
            .execute(&context(), request(Language::De))
            .await;

        assert!(
            matches!(result, Ok(view) if view.title == Some(Localized::new(Language::De, Title::from("Schrank")))
            && view.description == Some(Localized::new(Language::De, Description::from("Deutsch"))))
        );
        assert_eq!(
            vec![product_id],
            lock_state(&state).requested_translation_product_ids
        );
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_fall_back_to_stored_current_product_content_when_translation_is_missing()
    -> Result<(), url::ParseError> {
        let state = state();
        let view = details_view()?;
        lock_state(&state).find_details_result = Some(Ok(Some(view)));

        let result = handler(&state)
            .execute(&context(), request(Language::Fr))
            .await;

        assert!(
            matches!(result, Ok(view) if view.title == Some(Localized::new(Language::En, Title::from("Cabinet")))
            && view.description == Some(Localized::new(Language::En, Description::from("Native"))))
        );
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_not_found_without_translation_lookup_or_commit() {
        let state = state();

        let result = handler(&state)
            .execute(&context(), request(Language::En))
            .await;

        assert!(matches!(result, Err(GetProductError::NotFound)));
        assert!(
            lock_state(&state)
                .requested_translation_product_ids
                .is_empty()
        );
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_detail_read_failure_without_commit() {
        let state = state();
        lock_state(&state).find_details_result =
            Some(Err(ProductDetailsReadError::ProductDetailsQueryFailed));

        let result = handler(&state)
            .execute(&context(), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::ProductDetailsQueryFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
    }

    #[tokio::test]
    async fn should_map_translation_read_failure_without_commit() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).find_details_result = Some(Ok(Some(details_view()?)));
        lock_state(&state).find_translations_result = Some(Err(
            ProductTranslationReadError::ProductTranslationLookupFailed,
        ));

        let result = handler(&state)
            .execute(&context(), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::ProductTranslationLookupFailed)
        ));
        assert_eq!(0, lock_state(&state).commit_count);
        Ok(())
    }

    #[tokio::test]
    async fn should_map_begin_failure() {
        let state = state();
        lock_state(&state).begin_error = true;

        let result = handler(&state)
            .execute(&context(), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::BeginTransactionFailed)
        ));
    }

    #[tokio::test]
    async fn should_map_commit_failure_after_localizing() -> Result<(), url::ParseError> {
        let state = state();
        lock_state(&state).commit_error = true;
        lock_state(&state).find_details_result = Some(Ok(Some(details_view()?)));

        let result = handler(&state)
            .execute(&context(), request(Language::En))
            .await;

        assert!(matches!(
            result,
            Err(GetProductError::CommitTransactionFailed)
        ));
        assert_eq!(1, lock_state(&state).commit_count);
        Ok(())
    }

    #[test]
    fn should_map_all_get_product_reader_errors() {
        assert!(matches!(
            GetProductError::from(ProductDetailsReadError::ProductDetailsReadModelInvalid),
            GetProductError::ProductDetailsReadModelInvalid
        ));
        assert!(matches!(
            GetProductError::from(ProductTranslationReadError::ProductTranslationReadModelInvalid),
            GetProductError::ProductTranslationReadModelInvalid
        ));
    }
}
