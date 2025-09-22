use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::shop_id::ShopId;
use shop_core::shop::Shop;
use shop_dynamodb::repository::ShopDynamoDbRepository;
use tracing::error;

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum GetShopError {
    #[error("Shop with id '{0}'")]
    ShopNotFound(ShopId),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetShopError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),
}

#[cfg(feature = "api")]
pub mod api {
    use crate::get_service::GetShopError;
    use common::api::error::ApiError;
    use common::api::error_code::SHOP_NOT_FOUND;
    use tracing::error;

    impl From<GetShopError> for ApiError {
        fn from(err: GetShopError) -> Self {
            match err {
                GetShopError::ShopNotFound(_) => ApiError::not_found(SHOP_NOT_FOUND),
                GetShopError::SdkGetShopError(err) => {
                    error!(error = ?err, "Encountered SdkGetShopError while getting shop.");
                    err.into()
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetShopService {
    async fn find_shop(&self, shop_id: &ShopId) -> Result<Shop, GetShopError>;
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
}

#[cfg(test)]
mod tests {
    use crate::get_service::{GetShopError, GetShopService, GetShopServiceImpl};
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
    };
    use common::shop_id::ShopId;
    use fake::{Fake, Faker};
    use shop_dynamodb::repository::MockShopDynamoDbRepository;

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
            GetShopError::SdkGetShopError(_) => {}
            _ => panic!("expected GetShopError::ShopNotFound"),
        }
    }
}
