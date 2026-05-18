use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_event::domain::LocalizedProductDomainEventPayloadView;
use crate::core::product_event::{ProductDomainEvent, ProductEvent};
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::repository::ProductDynamoDbRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::aggregate::Aggregate;
use common::currency::domain::Currency;
use common::event::Event;
use common::language::domain::Language;
use common::price::domain::MonetaryAmountOverflowError;
use common::product_id::{ProductId, ProductKey};
use common::shop_id::ShopId;
use common::shops_product_id::ShopsProductId;
use common::slug_id::SlugId;
use tracing::error;

#[derive(thiserror::Error, Debug)]
pub enum GetProductError {
    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

    #[error("Product with ShopSlugId '{0}' and ProductSlugId '{1}' not found.")]
    ProductSlugNotFound(SlugId<0>, SlugId<6>),

    #[error("{0}")]
    MonetaryAmountOverflowError(#[from] MonetaryAmountOverflowError),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0:?}")]
    SdkBatchGetItemError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for QueryItem: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Unable to resolve unprocessed items after '{0}' retries. Failing entire operation.")]
    UnprocessedAfterMaxRetries(u32),

    #[error("Failed replaying product events: {0}")]
    ProductReplayError(#[from] crate::core::product::ProductReplayError),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::get_service::GetProductError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        MONETARY_AMOUNT_OVERFLOW, PRODUCT_NOT_FOUND, UNPROCESSED_AFTER_MAX_RETRIES,
    };

    impl From<GetProductError> for ApiError {
        fn from(err: GetProductError) -> Self {
            match err {
                GetProductError::ProductNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                GetProductError::ProductSlugNotFound(_, _) => {
                    ApiError::not_found(PRODUCT_NOT_FOUND, Box::new(err))
                }
                GetProductError::MonetaryAmountOverflowError(_) => {
                    ApiError::internal_server_error(MONETARY_AMOUNT_OVERFLOW, Box::new(err))
                }
                GetProductError::SdkGetItemError(err) => err.into(),
                GetProductError::SdkBatchGetItemError(err) => err.into(),
                GetProductError::SdkQueryError(err) => err.into(),
                GetProductError::UnprocessedAfterMaxRetries(_)
                | GetProductError::ProductReplayError(_) => {
                    ApiError::service_unavailable(UNPROCESSED_AFTER_MAX_RETRIES, Box::new(err))
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetProductService {
    async fn find_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Product, GetProductError>;

    async fn find_product_by_slug(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
    ) -> Result<Product, GetProductError>;

    async fn view_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedProductView, GetProductError>;

    async fn view_product_by_slug(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedProductView, GetProductError>;

    async fn view_products(
        &self,
        items: Vec<ProductKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<LocalizedProductView>, GetProductError>;

    async fn view_product_history(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<Event<ProductId, LocalizedProductDomainEventPayloadView>>, GetProductError>;
}

pub struct GetProductServiceImpl<'a> {
    repository: &'a (dyn ProductDynamoDbRepository + Sync),
}

impl<'a> GetProductServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ProductDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> GetProductService for GetProductServiceImpl<'a> {
    async fn find_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
    ) -> Result<Product, GetProductError> {
        let event_records = self
            .repository
            .query_product_event_records(shop_id, shops_product_id)
            .await?;
        if event_records.is_empty() {
            return Err(GetProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ));
        }
        let events =
            event_records
                .into_iter()
                .filter_map(|record| match ProductEvent::try_from(record) {
                    Ok(event) => Some(event),
                    Err(err) => {
                        error!(error = %err, "Failed mapping ProductEventRecord.");
                        None
                    }
                });

        Ok(Product::replay(events)?)
    }

    async fn find_product_by_slug(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
    ) -> Result<Product, GetProductError> {
        let product_key_opt = self
            .repository
            .query_product_key(shop_slug_id, product_slug_id)
            .await?;
        match product_key_opt {
            Some(product_key) => {
                self.find_product(&product_key.shop_id, &product_key.shops_product_id)
                    .await
            }
            None => Err(GetProductError::ProductSlugNotFound(
                shop_slug_id.clone(),
                product_slug_id.clone(),
            )),
        }
    }

    async fn view_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        preferred_languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedProductView, GetProductError> {
        let product = self
            .repository
            .query_product_event_records(shop_id, shops_product_id)
            .await?;
        if product.is_empty() {
            return Err(GetProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ));
        }
        let product = Product::replay(product.into_iter().filter_map(|record| {
            ProductEvent::try_from(record)
                .map_err(|err| error!(error = %err, "Failed mapping ProductEventRecord."))
                .ok()
        }))?;
        let product_view = product.localized(currency, preferred_languages);

        Ok(product_view)
    }

    async fn view_product_by_slug(
        &self,
        shop_slug_id: &SlugId<0>,
        product_slug_id: &SlugId<6>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedProductView, GetProductError> {
        let product_key_opt = self
            .repository
            .query_product_key(shop_slug_id, product_slug_id)
            .await?;
        match product_key_opt {
            Some(product_key) => {
                self.view_product(
                    &product_key.shop_id,
                    &product_key.shops_product_id,
                    languages,
                    currency,
                )
                .await
            }
            None => Err(GetProductError::ProductSlugNotFound(
                shop_slug_id.clone(),
                product_slug_id.clone(),
            )),
        }
    }

    async fn view_products(
        &self,
        product_keys: Vec<ProductKey>,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<LocalizedProductView>, GetProductError> {
        let mut products = Vec::with_capacity(product_keys.len());
        for product_key in product_keys {
            products.push(
                self.find_product(&product_key.shop_id, &product_key.shops_product_id)
                    .await?,
            );
        }
        let product_views = products
            .into_iter()
            .map(|product| product.localized(currency, languages))
            .collect();

        Ok(product_views)
    }

    async fn view_product_history(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        _languages: &[Language],
        currency: &Currency,
    ) -> Result<Vec<Event<ProductId, LocalizedProductDomainEventPayloadView>>, GetProductError>
    {
        let events: Vec<Event<ProductId, LocalizedProductDomainEventPayloadView>> = self
            .repository
            .query_product_domain_event_records(shop_id, shops_product_id)
            .await?
            .into_iter()
            .filter_map(
                |event_record| match ProductDomainEvent::try_from(event_record) {
                    Ok(event) => Some(event),
                    Err(err) => {
                        error!(
                            error = %err,
                            fromtype = %std::any::type_name::<ProductDomainEventRecord>(),
                            totype = %std::any::type_name::<ProductDomainEvent>(),
                            "Failed mapping"
                        );
                        None
                    }
                },
            )
            .map(|event| event.map_payload(|payload| payload.localized(currency)))
            .collect();

        if events.is_empty() {
            Err(GetProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))
        } else {
            Ok(events)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamodb::product_event_record::ProductEventRecord;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use common::has_key::HasKey;
    use fake::{Fake, Faker};

    fn created_event_record() -> (ShopId, ShopsProductId, ProductEventRecord) {
        let event = Product::create(
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
            Faker.fake(),
        );
        let key = event.payload.key();
        (
            key.shop_id,
            key.shops_product_id,
            ProductEventRecord::Domain(event.into()),
        )
    }

    #[tokio::test]
    async fn should_find_product_when_event_stream_exists() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![record]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .find_product(&shop_id, &shops_product_id)
            .await
            .unwrap();

        assert_eq!(ProductKey::new(shop_id, shops_product_id), actual.key());
    }

    #[tokio::test]
    async fn should_return_product_not_found_when_event_stream_is_empty() {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service.find_product(&shop_id, &shops_product_id).await;

        assert!(matches!(
            actual,
            Err(GetProductError::ProductNotFound(_, _))
        ));
    }

    #[tokio::test]
    async fn should_view_products_by_replaying_each_event_stream() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![record]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .view_products(
                vec![ProductKey::new(shop_id, shops_product_id)],
                &[],
                &Currency::Eur,
            )
            .await
            .unwrap();

        assert_eq!(1, actual.len());
    }

    #[tokio::test]
    async fn should_find_product_by_slug_when_product_key_exists() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let mut repository = MockProductDynamoDbRepository::default();
        let expected_key = ProductKey::new(shop_id, shops_product_id.clone());
        repository
            .expect_query_product_key()
            .return_once(move |_, _| {
                Box::pin(async move { Ok(Some(ProductKey::new(shop_id, shops_product_id))) })
            });
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![record]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .find_product_by_slug(&Faker.fake(), &Faker.fake())
            .await
            .unwrap();

        assert_eq!(expected_key, actual.key());
    }

    #[tokio::test]
    async fn should_return_product_slug_not_found_when_product_key_does_not_exist() {
        let shop_slug_id = Faker.fake();
        let product_slug_id = Faker.fake();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_key()
            .return_once(|_, _| Box::pin(async { Ok(None) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .find_product_by_slug(&shop_slug_id, &product_slug_id)
            .await;

        assert!(matches!(
            actual,
            Err(GetProductError::ProductSlugNotFound(_, _))
        ));
    }

    #[tokio::test]
    async fn should_view_product_when_event_stream_exists() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![record]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .view_product(&shop_id, &shops_product_id, &[Language::En], &Currency::Eur)
            .await
            .unwrap();

        assert_eq!(shop_id, actual.shop_id);
        assert_eq!(shops_product_id, actual.shops_product_id);
    }

    #[tokio::test]
    async fn should_view_product_by_slug_when_product_key_exists() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let expected_key = ProductKey::new(shop_id, shops_product_id.clone());
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_key()
            .return_once(move |_, _| {
                Box::pin(async move { Ok(Some(ProductKey::new(shop_id, shops_product_id))) })
            });
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![record]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .view_product_by_slug(
                &Faker.fake(),
                &Faker.fake(),
                &[Language::En],
                &Currency::Eur,
            )
            .await
            .unwrap();

        assert_eq!(expected_key.shop_id, actual.shop_id);
        assert_eq!(expected_key.shops_product_id, actual.shops_product_id);
    }

    #[tokio::test]
    async fn should_view_product_history_when_domain_events_exist() {
        let (shop_id, shops_product_id, record) = created_event_record();
        let ProductEventRecord::Domain(domain_record) = record else {
            panic!("expected domain event record")
        };
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_domain_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![domain_record]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .view_product_history(&shop_id, &shops_product_id, &[Language::En], &Currency::Eur)
            .await
            .unwrap();

        assert_eq!(1, actual.len());
    }

    #[tokio::test]
    async fn should_return_product_not_found_when_product_history_is_empty() {
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_domain_event_records()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));
        let service = GetProductServiceImpl::new(&repository);

        let actual = service
            .view_product_history(&ShopId::new(), &ShopsProductId::new(), &[], &Currency::Eur)
            .await;

        assert!(matches!(
            actual,
            Err(GetProductError::ProductNotFound(_, _))
        ));
    }
}
