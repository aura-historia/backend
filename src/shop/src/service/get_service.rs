use crate::core::shop::Shop;
use crate::dynamodb::repository::ShopDynamoDbRepository;
use crate::{
    core::partner_shop::PartnerShop, service::get_service::VerifyPartnerShopError::NotAPartnerShop,
};
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::{batch::Batch, domain::Domain, shop_id::ShopId, shop_slug_id::ShopSlugId};

#[derive(thiserror::Error, Debug)]
pub enum GetShopError {
    #[error("Shop with identifier '{0}' not found")]
    ShopNotFound(ShopId),

    #[error("Shop with SlugId '{0}' not found")]
    ShopSlugIdNotFound(ShopSlugId),

    #[error("Shop with Shopify domain '{0}' not found")]
    ShopifyDomainNotFound(Domain),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(
        #[source] Box<SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for Query: {0:?}")]
    SdkQueryError(
        #[source] Box<SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0:?}")]
    SdkBatchGetItemError(
        #[source]
        Box<
            SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>,
        >,
    ),

    #[error("Unable to resolve unprocessed items after '{0}' retries. Failing entire operation.")]
    UnprocessedAfterMaxRetries(u32),
}

impl From<SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>>
    for GetShopError
{
    fn from(
        error: SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ) -> Self {
        Self::SdkGetItemError(Box::new(error))
    }
}

impl From<SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>> for GetShopError {
    fn from(error: SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>) -> Self {
        Self::SdkQueryError(Box::new(error))
    }
}

impl From<SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>>
    for GetShopError
{
    fn from(
        error: SdkError<
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
            HttpResponse,
        >,
    ) -> Self {
        Self::SdkBatchGetItemError(Box::new(error))
    }
}

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum VerifyPartnerShopError {
    #[error("Shop with identifier '{0}' not found")]
    ShopNotFound(ShopId),

    #[error("Shop '{0}' is not a partner shop")]
    NotAPartnerShop(ShopId),

    #[error("API key mismatch for shop '{0}'")]
    ApiKeyMismatch(ShopId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::get_service::{GetShopError, VerifyPartnerShopError};
    use common::api::error::ApiError;
    use common::api::error_code::{
        AURA_HISTORIA_API_KEY_MISMATCH, PARTNER_SHOP_NOT_PARTNERED, SHOP_NOT_FOUND,
        UNPROCESSED_AFTER_MAX_RETRIES,
    };

    impl From<GetShopError> for ApiError {
        fn from(err: GetShopError) -> Self {
            match err {
                GetShopError::ShopNotFound(_) => ApiError::not_found(SHOP_NOT_FOUND, Box::new(err)),
                GetShopError::ShopSlugIdNotFound(_) => {
                    ApiError::not_found(SHOP_NOT_FOUND, Box::new(err))
                }
                GetShopError::ShopifyDomainNotFound(_) => {
                    ApiError::not_found(SHOP_NOT_FOUND, Box::new(err))
                }
                GetShopError::SdkGetItemError(err) => (*err).into(),
                GetShopError::SdkQueryError(err) => (*err).into(),
                GetShopError::SdkBatchGetItemError(err) => (*err).into(),
                GetShopError::UnprocessedAfterMaxRetries(_) => {
                    ApiError::service_unavailable(UNPROCESSED_AFTER_MAX_RETRIES, Box::new(err))
                }
            }
        }
    }

    impl From<VerifyPartnerShopError> for ApiError {
        fn from(err: VerifyPartnerShopError) -> Self {
            match err {
                VerifyPartnerShopError::ShopNotFound(_) => {
                    ApiError::not_found(SHOP_NOT_FOUND, Box::new(err))
                }
                VerifyPartnerShopError::NotAPartnerShop(_) => {
                    ApiError::forbidden(PARTNER_SHOP_NOT_PARTNERED).with_detail(err.to_string())
                }
                VerifyPartnerShopError::ApiKeyMismatch(_) => {
                    ApiError::unauthorized(AURA_HISTORIA_API_KEY_MISMATCH)
                        .with_header_field("Authorization")
                }
                VerifyPartnerShopError::SdkGetItemError(err) => err.into(),
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetShopService {
    async fn find_shop(&self, shop_id: &ShopId) -> Result<Shop, GetShopError>;

    async fn find_shop_by_slug(&self, shop_slug_id: &ShopSlugId) -> Result<Shop, GetShopError>;

    async fn find_shops(&self, shop_ids: Vec<ShopId>) -> Result<Vec<Shop>, GetShopError>;

    async fn find_shop_by_shopify_domain(
        &self,
        shopify_domain: &Domain,
    ) -> Result<Shop, GetShopError>;

    async fn find_partner_shop(
        &self,
        shop_id: &ShopId,
    ) -> Result<PartnerShop, VerifyPartnerShopError>;
}

pub struct GetShopServiceImpl<'a> {
    repository: &'a (dyn ShopDynamoDbRepository + Sync),
}

impl<'a> GetShopServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ShopDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait]
impl<'a> GetShopService for GetShopServiceImpl<'a> {
    async fn find_shop(&self, shop_id: &ShopId) -> Result<Shop, GetShopError> {
        let shop_record = self
            .repository
            .get_shop_record(shop_id)
            .await?
            .ok_or(GetShopError::ShopNotFound(*shop_id))?;

        Ok(shop_record.into())
    }

    async fn find_shop_by_slug(&self, shop_slug_id: &ShopSlugId) -> Result<Shop, GetShopError> {
        let shop_id_opt = self.repository.query_shop_id(shop_slug_id).await?;
        match shop_id_opt {
            Some(shop_id) => self.find_shop(&shop_id).await,
            None => Err(GetShopError::ShopSlugIdNotFound(shop_slug_id.clone())),
        }
    }

    async fn find_shops(&self, shop_ids: Vec<ShopId>) -> Result<Vec<Shop>, GetShopError> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let mut views = Vec::with_capacity(shop_ids.len());
        let mut unprocessed = shop_ids;
        let mut retry_count = 0;
        loop {
            let (mut local_shops, local_unprocessed) =
                self.find_shops_with_unprocessed(unprocessed).await?;
            views.append(&mut local_shops);

            if local_unprocessed.is_empty() {
                break;
            } else if retry_count >= MAX_RETRIES {
                return Err(GetShopError::UnprocessedAfterMaxRetries(MAX_RETRIES));
            }

            retry_count += 1;
            let delay_ms = BASE_DELAY_MS * 2_u64.pow(retry_count - 1);
            tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms)).await;

            unprocessed = local_unprocessed;
        }

        Ok(views)
    }

    async fn find_shop_by_shopify_domain(
        &self,
        shopify_domain: &Domain,
    ) -> Result<Shop, GetShopError> {
        let record = self
            .repository
            .query_shop_by_shopify_domain(shopify_domain)
            .await?
            .ok_or_else(|| GetShopError::ShopifyDomainNotFound(shopify_domain.clone()))?;
        Ok(record.into())
    }

    async fn find_partner_shop(
        &self,
        shop_id: &ShopId,
    ) -> Result<PartnerShop, VerifyPartnerShopError> {
        let shop_record = self
            .repository
            .get_shop_record(shop_id)
            .await
            .map_err(VerifyPartnerShopError::SdkGetItemError)?
            .ok_or(VerifyPartnerShopError::ShopNotFound(*shop_id))?;

        Ok(PartnerShop::try_from(shop_record).map_err(|_| NotAPartnerShop(*shop_id))?)
    }
}

impl<'a> GetShopServiceImpl<'a> {
    // Keep the legacy AWS error-by-value API stable until the shop crate is retired.
    #[allow(clippy::result_large_err)]
    async fn find_shops_with_unprocessed(
        &self,
        shop_ids: Vec<ShopId>,
    ) -> Result<(Vec<Shop>, Vec<ShopId>), GetShopError> {
        let mut unprocessed = Vec::new();
        let mut shop_records = Vec::with_capacity(shop_ids.len());
        for batch in Batch::chunked_from(shop_ids.into_iter()) {
            let mut res = self.repository.get_shop_records(&batch).await?;
            if let Some(local_unprocessed) = res.unprocessed {
                unprocessed.extend(local_unprocessed);
            }
            shop_records.append(&mut res.items);
        }

        let shops = shop_records.into_iter().map(Shop::from).collect();
        Ok((shops, unprocessed))
    }
}

#[cfg(test)]
mod tests {
    use crate::dynamodb::repository::MockShopDynamoDbRepository;
    use crate::dynamodb::shop_record::ShopRecord;
    use crate::service::get_service::{
        GetShopError, GetShopService, GetShopServiceImpl, VerifyPartnerShopError,
    };
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
    };
    use common::domain::Domain;
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use rstest;

    #[tokio::test]
    async fn should_return_shop_when_exists() {
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let actual = service.find_shop(&ShopId::new()).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_return_shop_not_found_err_when_shop_does_not_exist() {
        let shop_id = ShopId::new();
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let actual = service.find_shop(&shop_id).await;

        assert!(actual.is_err());
        match actual.unwrap_err() {
            GetShopError::ShopNotFound(err_shop_id) => {
                assert_eq!(err_shop_id, shop_id);
            }
            _ => panic!("expected GetShopError::ShopNotFound"),
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
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(|_| Box::pin(async { Err(expected) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let actual = service.find_shop(&shop_id).await;

        assert!(actual.is_err());
        match actual.unwrap_err() {
            GetShopError::SdkGetItemError(_) => {}
            _ => panic!("expected GetShopError::ShopNotFound"),
        }
    }

    #[tokio::test]
    async fn should_return_shop_when_shopify_domain_exists_for_find_shop_by_shopify_domain() {
        let shopify_domain = Domain::try_from("partner-shop.myshopify.com").unwrap();
        let mut record: ShopRecord = Faker.fake();
        record.shopify_domain = Some(shopify_domain.clone());
        let shop_id = record.shop_id;

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_query_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));

        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let result = service
            .find_shop_by_shopify_domain(&shopify_domain)
            .await
            .unwrap();
        assert_eq!(result.shop_id, shop_id);
    }

    #[tokio::test]
    async fn should_return_not_found_when_shopify_domain_missing_for_find_shop_by_shopify_domain() {
        let shopify_domain = Domain::try_from("missing.myshopify.com").unwrap();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_query_shop_by_shopify_domain()
            .return_once(move |_| Box::pin(async move { Ok(None) }));

        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let result = service.find_shop_by_shopify_domain(&shopify_domain).await;
        assert!(matches!(
            result.unwrap_err(),
            GetShopError::ShopifyDomainNotFound(_)
        ));
    }

    #[tokio::test]
    async fn should_return_not_found_for_find_partner_shop_when_shop_does_not_exist() {
        let shop_id = ShopId::new();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(move |_| Box::pin(async { Ok(None) }));

        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let result = service.find_partner_shop(&shop_id).await;
        assert!(matches!(
            result.unwrap_err(),
            VerifyPartnerShopError::ShopNotFound(_)
        ));
    }
}
