use crate::core::product::{LocalizedProductView, Product};
use crate::core::product_event::ProductDomainEvent;
use crate::core::product_event::domain::LocalizedProductDomainEventPayloadView;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::repository::ProductDynamoDbRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::batch::Batch;
use common::currency::domain::Currency;
use common::event::Event;
use common::language::domain::Language;
use common::price::domain::MonetaryAmountOverflowError;
use common::product_id::{ProductId, ProductKey};
use common::product_slug_id::ProductSlugId;
use common::shop_id::ShopId;
use common::shop_slug_id::ShopSlugId;
use common::shops_product_id::ShopsProductId;
use tracing::error;

#[derive(thiserror::Error, Debug)]
pub enum GetProductError {
    #[error("Product with ShopId '{0}' and ShopsProductId '{1}' not found.")]
    ProductNotFound(ShopId, ShopsProductId),

    #[error("Product with ShopSlugId '{0}' and ProductSlugId '{1}' not found.")]
    ProductSlugNotFound(ShopSlugId, ProductSlugId),

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
                GetProductError::UnprocessedAfterMaxRetries(_) => {
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
        shop_slug_id: &ShopSlugId,
        product_slug_id: &ProductSlugId,
    ) -> Result<Product, GetProductError>;

    async fn find_products(&self, items: Vec<ProductKey>) -> Result<Vec<Product>, GetProductError>;

    async fn view_product(
        &self,
        shop_id: &ShopId,
        shops_product_id: &ShopsProductId,
        languages: &[Language],
        currency: &Currency,
    ) -> Result<LocalizedProductView, GetProductError>;

    async fn view_product_by_slug(
        &self,
        shop_slug_id: &ShopSlugId,
        product_slug_id: &ProductSlugId,
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
        let product_record = self
            .repository
            .get_product_record(shop_id, shops_product_id)
            .await?
            .ok_or(GetProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;

        Ok(product_record.into())
    }

    async fn find_product_by_slug(
        &self,
        shop_slug_id: &ShopSlugId,
        product_slug_id: &ProductSlugId,
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
        let product_record = self
            .repository
            .get_product_record(shop_id, shops_product_id)
            .await?
            .ok_or(GetProductError::ProductNotFound(
                *shop_id,
                shops_product_id.clone(),
            ))?;
        let product: Product = product_record.into();
        let product_view = product.localized(currency, preferred_languages);

        Ok(product_view)
    }

    async fn view_product_by_slug(
        &self,
        shop_slug_id: &ShopSlugId,
        product_slug_id: &ProductSlugId,
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
        let products = self.find_products(product_keys).await?;
        let product_views = products
            .into_iter()
            .map(|product| product.localized(currency, languages))
            .collect();

        Ok(product_views)
    }

    async fn find_products(&self, items: Vec<ProductKey>) -> Result<Vec<Product>, GetProductError> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let mut products = Vec::with_capacity(items.len());
        let mut unprocessed = items;
        let mut retry_count = 0;
        loop {
            let (mut local_views, local_unprocessed) =
                self.view_products_with_unprocessed(unprocessed).await?;
            products.append(&mut local_views);

            if local_unprocessed.is_empty() {
                break;
            } else if retry_count >= MAX_RETRIES {
                return Err(GetProductError::UnprocessedAfterMaxRetries(MAX_RETRIES));
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            unprocessed = local_unprocessed;
        }

        Ok(products)
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

impl<'a> GetProductServiceImpl<'a> {
    // Keep the legacy AWS error-by-value API stable until the product crate is retired.
    #[allow(clippy::result_large_err)]
    async fn view_products_with_unprocessed(
        &self,
        items: Vec<ProductKey>,
    ) -> Result<(Vec<Product>, Vec<ProductKey>), GetProductError> {
        let mut products = Vec::with_capacity(items.len());
        let mut unprocessed = Vec::new();
        for batch in Batch::chunked_from(items.into_iter()) {
            let result = self.repository.get_product_records(&batch).await?;
            if let Some(up) = result.unprocessed {
                unprocessed.extend(up);
            }
            let local_products = result.items.into_iter().map(Product::from);
            products.extend(local_products);
        }

        Ok((products, unprocessed))
    }
}

#[cfg(test)]
mod tests {
    mod find_product {
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{shop_id::ShopId, shops_product_id::ShopsProductId};
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_product_when_exists() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_product(&ShopId::new(), &ShopsProductId::new())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_product_not_found_err_when_product_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service.find_product(&shop_id, &shops_product_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_product_id, shops_product_id);
                }
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let shop_id = ShopId::new();
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service.find_product(&shop_id, &shops_product_id).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkGetItemError(_) => {}
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }
    }

    mod find_product_by_slug {
        use crate::dynamodb::repository::MockProductDynamoDbRepository;
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_product_when_exists() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_query_product_key()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_product_by_slug(&Faker.fake(), &Faker.fake())
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        async fn should_return_product_not_found_err_when_product_does_not_exist() {
            let shop_slug_id = Faker.fake();
            let product_slug_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_query_product_key()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_product_by_slug(&shop_slug_id, &product_slug_id)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::ProductSlugNotFound(err_shop_slug_id, err_product_slug_id) => {
                    assert_eq!(err_shop_slug_id, shop_slug_id);
                    assert_eq!(err_product_slug_id, product_slug_id);
                }
                _ => panic!("expected GetProductError::ProductSlugNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::query::QueryError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_for_query(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::query::QueryError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_query_product_key()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_product_by_slug(&Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkQueryError(_) => {}
                _ => panic!("expected GetProductError::SdkQueryError"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error_for_get_item(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_query_product_key()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .find_product_by_slug(&Faker.fake(), &Faker.fake())
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkGetItemError(_) => {}
                _ => panic!("expected GetProductError::SdkGetItemError"),
            }
        }
    }

    mod view_product {
        use crate::dynamodb::{
            product_record::ProductRecord, repository::MockProductDynamoDbRepository,
        };
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{
            currency::domain::Currency,
            language::{
                domain::Language,
                domain::Language::*,
                record::{LanguageRecord, TextRecord},
            },
            shop_id::ShopId,
            shops_product_id::ShopsProductId,
        };
        use fake::{Fake, Faker};

        #[tokio::test]
        async fn should_return_product_when_exists() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(&ShopId::new(), &ShopsProductId::new(), &[], &Currency::Eur)
                .await;
            assert!(actual.is_ok());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::eur(Currency::Eur, 2)]
        #[case::gbp(Currency::Gbp, 4)]
        #[case::usd(Currency::Usd, 10)]
        #[case::aud(Currency::Aud, 1000)]
        #[case::cad(Currency::Cad, 4000)]
        #[case::nzd(Currency::Nzd, 42)]
        #[trace]
        async fn should_respect_currency(#[case] currency: Currency, #[case] expected_amount: u64) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.price_native = None;
            expected_record.price_eur = Some(2);
            expected_record.price_gbp = Some(4);
            expected_record.price_usd = Some(10);
            expected_record.price_aud = Some(1000);
            expected_record.price_cad = Some(4000);
            expected_record.price_nzd = Some(42);
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_price = service
                .view_product(&ShopId::new(), &ShopsProductId::new(), &[], &currency)
                .await
                .unwrap()
                .price
                .unwrap();
            assert_eq!(currency, actual_price.currency);
            assert_eq!(expected_amount, u64::from(actual_price.monetary_amount));
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], En, "English")]
        #[case(&[De], De, "German")]
        #[case(&[De, En], De, "German")]
        #[case(&[De, Fr], De, "German")]
        #[case(&[Fr, De, En, Es], Fr, "French")]
        #[case(&[En], En, "English")]
        #[case(&[En, De, Fr, Es], En, "English")]
        #[case(&[En, De, Es], En, "English")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        #[trace]
        async fn should_respect_language_for_title(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_title: &str,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = Some("German".to_string());
            expected_record.title_en = Some("English".to_string());
            expected_record.title_fr = Some("French".to_string());
            expected_record.title_es = Some("Spanish".to_string());
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .title;
            assert_eq!(expected_language, actual_title.localization);
            assert_eq!(expected_title, actual_title.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], Es, "Spanish")]
        #[case(&[De], Es, "Spanish")]
        #[case(&[De, En], Es, "Spanish")]
        #[case(&[De, Fr], Es, "Spanish")]
        #[case(&[Fr, De, En, Es], Es, "Spanish")]
        #[case(&[En], Es, "Spanish")]
        #[case(&[En, De, Fr, Es], Es, "Spanish")]
        #[case(&[En, De, Es], Es, "Spanish")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        #[trace]
        async fn should_fallback_to_native_when_only_native_exists_for_title(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_title: &str,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.title_native = TextRecord::new("Spanish", LanguageRecord::Es);
            expected_record.title_de = None;
            expected_record.title_en = None;
            expected_record.title_fr = None;
            expected_record.title_es = None;
            expected_record.title_it = None;
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_title = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .title;
            assert_eq!(expected_language, actual_title.localization);
            assert_eq!(expected_title, actual_title.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[], Es, "Spanish")]
        #[case(&[De], Es, "Spanish")]
        #[case(&[De, En], Es, "Spanish")]
        #[case(&[De, Fr], Es, "Spanish")]
        #[case(&[Fr, De, En, Es], Es, "Spanish")]
        #[case(&[En], Es, "Spanish")]
        #[case(&[En, De, Fr, Es], Es, "Spanish")]
        #[case(&[En, De, Es], Es, "Spanish")]
        #[case(&[Es, De, En], Es, "Spanish")]
        #[case(&[Es, En, De], Es, "Spanish")]
        #[trace]
        async fn should_fallback_to_native_when_only_native_exists_for_description(
            #[case] languages: &[Language],
            #[case] expected_language: Language,
            #[case] expected_description: &str,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.description_native =
                Some(TextRecord::new("Spanish", LanguageRecord::Es));
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .description
                .unwrap();
            assert_eq!(expected_language, actual_description.localization);
            assert_eq!(expected_description, actual_description.payload.as_ref());
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case(&[])]
        #[case(&[De])]
        #[case(&[De, En])]
        #[case(&[De, Fr])]
        #[case(&[Fr, De, En, Es])]
        #[case(&[En])]
        #[case(&[En, De, Fr, Es])]
        #[case(&[En, De, Es])]
        #[case(&[Es, De, En])]
        #[case(&[Es, En, De])]
        #[trace]
        async fn should_return_product_without_description_when_none_exists(
            #[case] languages: &[Language],
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut expected_record: ProductRecord = Faker.fake();
            expected_record.description_native = None;
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(Some(expected_record)) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual_description = service
                .view_product(
                    &ShopId::new(),
                    &ShopsProductId::new(),
                    languages,
                    &Currency::Gbp,
                )
                .await
                .unwrap()
                .description;
            assert!(actual_description.is_none());
        }

        #[tokio::test]
        async fn should_return_product_not_found_err_when_product_does_not_exist() {
            let shop_id = ShopId::new();
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Ok(None) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(&shop_id, &shops_product_id, &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                    assert_eq!(err_shop_id, shop_id);
                    assert_eq!(err_shops_product_id, shops_product_id);
                }
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::get_item::GetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_propagate_sdk_error(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::get_item::GetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let shop_id = ShopId::new();
            let shops_product_id = "non-existent".into();
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product(&shop_id, &shops_product_id, &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkGetItemError(_) => {}
                _ => panic!("expected GetProductError::ProductNotFound"),
            }
        }
    }

    mod view_products {
        use crate::dynamodb::{
            product_record::ProductRecord, repository::MockProductDynamoDbRepository,
        };
        use crate::service::get_service::{
            GetProductError, GetProductService, GetProductServiceImpl,
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{batch::Batch, currency::domain::Currency};
        use common::{batch::dynamodb::BatchGetItemResult, product_id::ProductKey};

        #[tokio::test]
        async fn should_return_products_when_all_processed() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: fake::vec![ProductRecord; 42],
                        unprocessed: None,
                    })
                })
            });
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_products(fake::vec![ProductKey; 42], &[], &Currency::Eur)
                .await
                .unwrap();
            assert_eq!(42, actual.len());
        }

        #[tokio::test]
        async fn should_completely_fail_when_some_processed() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: fake::vec![ProductRecord; 37],
                        unprocessed: Some(
                            Batch::try_from_iter(fake::vec![ProductKey; 5].into_iter()).unwrap(),
                        ),
                    })
                })
            });
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_products(fake::vec![ProductKey; 42], &[], &Currency::Eur)
                .await
                .unwrap_err();
            match actual {
                GetProductError::UnprocessedAfterMaxRetries(_) => {}
                other => {
                    panic!(
                        "Expected 'GetProductError::UnprocessedAfterMaxRetries'. Got '{other:?}'."
                    )
                }
            }
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_fail_entire_operation_and_propagate_sdk_error_when_batch_operation_fails(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_records()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_products(fake::vec![ProductKey; 222], &[], &Currency::Eur)
                .await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                GetProductError::SdkBatchGetItemError(_) => {}
                _ => panic!("expected GetProductError::SdkBatchGetItemError"),
            }
        }
    }

    mod view_product_events {
        use crate::{
            dynamodb::{
                product_event_record::domain::ProductDomainEventRecord,
                repository::MockProductDynamoDbRepository,
            },
            service::get_service::{GetProductError, GetProductService, GetProductServiceImpl},
        };
        use common::{
            currency::domain::Currency, shop_id::ShopId, shops_product_id::ShopsProductId,
        };
        use itertools::Itertools;

        #[tokio::test]
        async fn should_keep_history_in_exact_order_as_dynamodb_read() {
            let mut repository = MockProductDynamoDbRepository::default();
            let mut events = fake::vec![ProductDomainEventRecord; 100];
            events.sort_by_key(|l| l.product_id);
            let expected = events
                .clone()
                .into_iter()
                .map(|record| record.product_id)
                .collect_vec();
            repository
                .expect_query_product_domain_event_records()
                .return_once(|_, _| Box::pin(async { Ok(events) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let actual = service
                .view_product_history(&ShopId::new(), &ShopsProductId::new(), &[], &Currency::Eur)
                .await
                .unwrap()
                .into_iter()
                .map(|event| event.aggregate_id)
                .collect_vec();

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_err_product_not_found_when_events_empty() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_query_product_domain_event_records()
                .return_once(|_, _| Box::pin(async { Ok(vec![]) }));
            let service = GetProductServiceImpl {
                repository: &repository,
            };
            let shop_id = ShopId::new();
            let shops_product_id = ShopsProductId::new();
            let actual = service
                .view_product_history(&shop_id, &shops_product_id, &[], &Currency::Eur)
                .await
                .unwrap_err();

            match actual {
                GetProductError::ProductNotFound(err_shop_id, err_shops_product_id) => {
                    assert_eq!(shop_id, err_shop_id);
                    assert_eq!(shops_product_id, err_shops_product_id);
                }
                other => {
                    panic!("Expected 'GetProductError::ProductNotFound'. Got: '{other}'")
                }
            }
        }
    }
}
