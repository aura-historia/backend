use crate::core::shop::Shop;
use crate::dynamodb::repository::ShopDynamoDbRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::{batch::Batch, shop_id::ShopIdentifier};

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum GetShopError {
    #[error("Shop with identifier '{0}' not found")]
    ShopNotFound(ShopIdentifier),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0}")]
    SdkBatchGetItemError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>,
    ),

    #[error("Unable to resolve unprocessed items after '{0}' retries. Failing entire operation.")]
    UnprocessedAfterMaxRetries(u32),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::get_service::GetShopError;
    use common::api::error::ApiError;
    use common::api::error_code::{SHOP_NOT_FOUND, UNPROCESSED_AFTER_MAX_RETRIES};

    impl From<GetShopError> for ApiError {
        fn from(err: GetShopError) -> Self {
            match err {
                GetShopError::ShopNotFound(_) => ApiError::not_found(SHOP_NOT_FOUND, Box::new(err)),
                GetShopError::SdkGetItemError(err) => err.into(),
                GetShopError::SdkBatchGetItemError(err) => err.into(),
                GetShopError::UnprocessedAfterMaxRetries(_) => {
                    ApiError::service_unavailable(UNPROCESSED_AFTER_MAX_RETRIES, Box::new(err))
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetShopService {
    async fn find_shop(&self, shop_identifier: &ShopIdentifier) -> Result<Shop, GetShopError>;

    async fn find_shops(
        &self,
        shop_identifiers: Vec<ShopIdentifier>,
    ) -> Result<Vec<Shop>, GetShopError>;
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
    async fn find_shop(&self, shop_identifier: &ShopIdentifier) -> Result<Shop, GetShopError> {
        let shop_record_opt = match shop_identifier {
            ShopIdentifier::ShopId(shop_id) => {
                self.repository.get_shop_record_by_id(shop_id).await?
            }
            ShopIdentifier::ShopDomain(domain) => {
                self.repository.get_shop_record_by_domain(domain).await?
            }
        };
        let shop_record =
            shop_record_opt.ok_or(GetShopError::ShopNotFound(shop_identifier.clone()))?;

        Ok(shop_record.into())
    }

    async fn find_shops(
        &self,
        shop_identifiers: Vec<ShopIdentifier>,
    ) -> Result<Vec<Shop>, GetShopError> {
        const MAX_RETRIES: u32 = 3;
        const BASE_DELAY_MS: u64 = 100;

        let mut views = Vec::with_capacity(shop_identifiers.len());
        let mut unprocessed = shop_identifiers;
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
}

impl<'a> GetShopServiceImpl<'a> {
    async fn find_shops_with_unprocessed(
        &self,
        shop_identifiers: Vec<ShopIdentifier>,
    ) -> Result<(Vec<Shop>, Vec<ShopIdentifier>), GetShopError> {
        let mut unprocessed = Vec::new();
        let mut shop_records = Vec::with_capacity(shop_identifiers.len());
        for batch in Batch::chunked_from(shop_identifiers.into_iter()) {
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
    use rstest;

    use crate::dynamodb::repository::MockShopDynamoDbRepository;
    use crate::service::get_service::{GetShopError, GetShopService, GetShopServiceImpl};
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
    };
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};

    #[tokio::test]
    async fn should_return_shop_when_exists() {
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record_by_id()
            .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let actual = service.find_shop(&ShopId::new().into()).await;
        assert!(actual.is_ok());
    }

    #[tokio::test]
    async fn should_return_shop_not_found_err_when_shop_does_not_exist() {
        let shop_id = ShopId::new();
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record_by_id()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let actual = service.find_shop(&shop_id.into()).await;

        assert!(actual.is_err());
        match actual.unwrap_err() {
            GetShopError::ShopNotFound(err_shop_id) => {
                assert_eq!(err_shop_id, shop_id.into());
            }
            _ => panic!("expected GetShopError::ShopNotFound"),
        }
    }

    #[tokio::test]
    #[rstest::rstest]
    #[trace]
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
    async fn should_propagate_sdk_error(
        #[case] expected: SdkError<
            aws_sdk_dynamodb::operation::get_item::GetItemError,
            aws_sdk_dynamodb::config::http::HttpResponse,
        >,
    ) {
        let shop_id = ShopId::new();
        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record_by_id()
            .return_once(|_| Box::pin(async { Err(expected) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };
        let actual = service.find_shop(&shop_id.into()).await;

        assert!(actual.is_err());
        match actual.unwrap_err() {
            GetShopError::SdkGetItemError(_) => {}
            _ => panic!("expected GetShopError::ShopNotFound"),
        }
    }
}
