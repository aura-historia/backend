use crate::core::partner_shop::PartnerShop;
use crate::core::partner_shop_api_key::PartnerShopApiKey;
use crate::core::shop::Shop;
use crate::dynamodb::repository::ShopDynamoDbRepository;
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use common::error::missing_field::MissingPersistenceField;
use common::{batch::Batch, shop_id::ShopId, slug_id::SlugId};

#[derive(thiserror::Error, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum GetShopError {
    #[error("Shop with identifier '{0}' not found")]
    ShopNotFound(ShopId),

    #[error("Shop with SlugId '{0}' not found")]
    ShopSlugIdNotFound(SlugId<0>),

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>,
    ),

    #[error("Encountered DynamoDB SdkError for Query: {0}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError, HttpResponse>),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0}")]
    SdkBatchGetItemError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError, HttpResponse>,
    ),

    #[error("Unable to resolve unprocessed items after '{0}' retries. Failing entire operation.")]
    UnprocessedAfterMaxRetries(u32),
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

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError, HttpResponse>),

    #[error("Missing persistence field: {0}")]
    MissingPersistenceField(#[from] MissingPersistenceField),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::get_service::{GetShopError, VerifyPartnerShopError};
    use common::api::error::ApiError;
    use common::api::error_code::{
        PARTNER_SHOP_API_KEY_MISMATCH, PARTNER_SHOP_NOT_PARTNERED, SHOP_NOT_FOUND,
        UNPROCESSED_AFTER_MAX_RETRIES,
    };

    impl From<GetShopError> for ApiError {
        fn from(err: GetShopError) -> Self {
            match err {
                GetShopError::ShopNotFound(_) => ApiError::not_found(SHOP_NOT_FOUND, Box::new(err)),
                GetShopError::ShopSlugIdNotFound(_) => {
                    ApiError::not_found(SHOP_NOT_FOUND, Box::new(err))
                }
                GetShopError::SdkGetItemError(err) => err.into(),
                GetShopError::SdkQueryError(err) => err.into(),
                GetShopError::SdkBatchGetItemError(err) => err.into(),
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
                    ApiError::unauthorized(PARTNER_SHOP_API_KEY_MISMATCH)
                        .with_header_field("x-api-key")
                }
                VerifyPartnerShopError::SdkGetItemError(err) => err.into(),
                VerifyPartnerShopError::MissingPersistenceField(_) => {
                    ApiError::internal_server_error(
                        common::api::error_code::INTERNAL_SERVER_ERROR,
                        Box::new(err),
                    )
                }
            }
        }
    }
}

#[async_trait]
#[mockall::automock]
pub trait GetShopService {
    async fn find_shop(&self, shop_id: &ShopId) -> Result<Shop, GetShopError>;

    async fn find_shop_by_slug(&self, shop_slug_id: &SlugId<0>) -> Result<Shop, GetShopError>;

    async fn find_shops(&self, shop_ids: Vec<ShopId>) -> Result<Vec<Shop>, GetShopError>;

    async fn verify_partner_shop(
        &self,
        api_key: &PartnerShopApiKey,
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

    async fn find_shop_by_slug(&self, shop_slug_id: &SlugId<0>) -> Result<Shop, GetShopError> {
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

    async fn verify_partner_shop(
        &self,
        api_key: &PartnerShopApiKey,
        shop_id: &ShopId,
    ) -> Result<PartnerShop, VerifyPartnerShopError> {
        use std::sync::OnceLock;
        static PARTNER_SHOP_CACHE: OnceLock<PartnerShop> = OnceLock::new();

        if let Some(cached) = PARTNER_SHOP_CACHE.get().filter(|c| {
            c.shop_id == *shop_id
                && c.hashed_api_key
                    .as_ref()
                    .is_some_and(|h| api_key.check(h))
        }) {
            return Ok(cached.clone());
        }

        let shop_record = self
            .repository
            .get_shop_record(shop_id)
            .await
            .map_err(VerifyPartnerShopError::SdkGetItemError)?
            .ok_or(VerifyPartnerShopError::ShopNotFound(*shop_id))?;

        if shop_record.partner_user_id.is_none() {
            return Err(VerifyPartnerShopError::NotAPartnerShop(*shop_id));
        }

        let partner_shop = PartnerShop::try_from(shop_record)?;

        match &partner_shop.hashed_api_key {
            Some(hashed) if api_key.check(hashed) => {}
            _ => return Err(VerifyPartnerShopError::ApiKeyMismatch(*shop_id)),
        }

        let _ = PARTNER_SHOP_CACHE.set(partner_shop.clone());

        Ok(partner_shop)
    }
}

impl<'a> GetShopServiceImpl<'a> {
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
    use rstest;

    use crate::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
    use crate::dynamodb::repository::MockShopDynamoDbRepository;
    use crate::dynamodb::shop_record::ShopRecord;
    use crate::service::get_service::{
        GetShopError, GetShopService, GetShopServiceImpl, VerifyPartnerShopError,
    };
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
    };
    use common::shop_id::ShopId;
    use common::user_id::UserId;
    use fake::{Fake, Faker};

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

    fn make_partner_shop_record(api_key: &PartnerShopApiKey) -> ShopRecord {
        let hashed: HashedPartnerShopApiKey = api_key.clone().into();
        let mut record: ShopRecord = Faker.fake();
        record.partner_api_key_short = Some(hashed.short_token().to_string());
        record.partner_api_key_long_hash = Some(hashed.long_token_hash().to_string());
        record.partner_user_id = Some(UserId::new());
        record
    }

    #[tokio::test]
    async fn should_verify_partner_shop_when_valid_api_key() {
        let api_key = PartnerShopApiKey::new();
        let record = make_partner_shop_record(&api_key);
        let shop_id = record.shop_id;

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };

        let result = service.verify_partner_shop(&api_key, &shop_id).await;
        assert!(result.is_ok());
        let partner = result.unwrap();
        assert_eq!(partner.shop_id, shop_id);
    }

    #[tokio::test]
    async fn should_return_shop_not_found_when_verifying_nonexistent_shop() {
        let shop_id = ShopId::new();
        let api_key = PartnerShopApiKey::new();

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(|_| Box::pin(async { Ok(None) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };

        let result = service.verify_partner_shop(&api_key, &shop_id).await;
        assert!(matches!(
            result.unwrap_err(),
            VerifyPartnerShopError::ShopNotFound(_)
        ));
    }

    #[tokio::test]
    async fn should_return_not_partner_when_shop_has_no_partner_user_id() {
        let api_key = PartnerShopApiKey::new();
        let mut record: ShopRecord = Faker.fake();
        record.partner_user_id = None;
        let shop_id = record.shop_id;

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };

        let result = service.verify_partner_shop(&api_key, &shop_id).await;
        assert!(matches!(
            result.unwrap_err(),
            VerifyPartnerShopError::NotAPartnerShop(_)
        ));
    }

    #[tokio::test]
    async fn should_return_api_key_mismatch_when_wrong_api_key() {
        let correct_key = PartnerShopApiKey::new();
        let wrong_key = PartnerShopApiKey::new();
        let record = make_partner_shop_record(&correct_key);
        let shop_id = record.shop_id;

        let mut repository = MockShopDynamoDbRepository::default();
        repository
            .expect_get_shop_record()
            .return_once(move |_| Box::pin(async move { Ok(Some(record)) }));
        let service = GetShopServiceImpl {
            repository: &repository,
        };

        let result = service.verify_partner_shop(&wrong_key, &shop_id).await;
        assert!(matches!(
            result.unwrap_err(),
            VerifyPartnerShopError::ApiKeyMismatch(_)
        ));
    }
}
