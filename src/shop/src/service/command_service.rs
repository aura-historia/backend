use crate::{
    core::shop::Shop,
    dynamodb::{repository::ShopDynamoDbRepository, shop_record::ShopRecord},
    service::command::CreateShopCommand,
};
use aws_sdk_dynamodb::error::SdkError;
use common::{
    batch::{Batch, BatchConstructionError},
    shop_id::{ShopId, ShopIdentifier},
    shop_name::ShopName,
};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
#[allow(clippy::large_enum_variant)]
pub enum CommandShopError {
    #[error(
        "Shop with name '{0}' exists already due - an URL for any domain of shop is already registered."
    )]
    ShopExistsAlready(ShopName),

    #[error("Shop can only have 100 URLs but was given more: '{0}'")]
    ShopCanOnlyHave100Urls(#[from] BatchConstructionError<100>),

    #[error(
        "Did not succeed checking existence of shop due to DynamoDB Batch-Response containing unprocessed items"
    )]
    SdkBatchGetItemUnprocessed,

    #[error("Encountered DynamoDB SdkError for GetItem: {0}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),

    #[error("Encountered DynamoDB TransactWriteItemsError for TransactWriteItems: {0}")]
    SdkTransactWriteItemsError(
        #[from]
        SdkError<aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError>,
    ),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0}")]
    SdkBatchGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError>,
    ),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::command_service::CommandShopError;
    use common::api::error::ApiError;
    use common::api::error_code::{SHOP_EXISTS_ALREADY, SHOP_TOO_MANY_URLS, UNPROCESSED_ITEMS};

    impl From<CommandShopError> for ApiError {
        fn from(err: CommandShopError) -> Self {
            match err {
                CommandShopError::ShopExistsAlready(_) => {
                    ApiError::conflict(SHOP_EXISTS_ALREADY, Box::new(err))
                }
                CommandShopError::ShopCanOnlyHave100Urls(_) => {
                    ApiError::bad_request(SHOP_TOO_MANY_URLS, Box::new(err))
                }
                CommandShopError::SdkBatchGetItemUnprocessed => {
                    ApiError::service_unavailable(UNPROCESSED_ITEMS, Box::new(err))
                }
                CommandShopError::SdkGetItemError(err) => err.into(),
                CommandShopError::SdkTransactWriteItemsError(err) => err.into(),
                CommandShopError::SdkBatchGetItemError(err) => err.into(),
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait CommandShopService {
    async fn create(&self, command: CreateShopCommand) -> Result<Shop, CommandShopError>;
}

pub struct CommandShopServiceImpl<'a> {
    repository: &'a (dyn ShopDynamoDbRepository + Sync),
}

impl<'a> CommandShopServiceImpl<'a> {
    pub fn new(repository: &'a (dyn ShopDynamoDbRepository + Sync)) -> Self {
        Self { repository }
    }
}

#[async_trait::async_trait]
impl<'a> CommandShopService for CommandShopServiceImpl<'a> {
    async fn create(&self, command: CreateShopCommand) -> Result<Shop, CommandShopError> {
        let shop_identifiers =
            Batch::try_from_iter(command.urls.clone().into_iter().map(ShopIdentifier::from))?;
        let get_res = self.repository.get_shop_records(&shop_identifiers).await?;
        if get_res.unprocessed.is_some() {
            return Err(CommandShopError::SdkBatchGetItemUnprocessed);
        }
        if !get_res.items.is_empty() {
            return Err(CommandShopError::ShopExistsAlready(command.name));
        }

        let shop = Shop {
            shop_id: ShopId::new(),
            name: command.name,
            urls: command.urls,
            image: command.image,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };
        let mut shop_records = ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).ok_or(
            CommandShopError::SdkTransactWriteItemsError(SdkError::construction_failure(
                "Failed constructing shop-url-records",
            )),
        )?;
        shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));

        let _ = self
            .repository
            .put_shop_records_transact(shop_records)
            .await?;

        Ok(shop)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        dynamodb::repository::MockShopDynamoDbRepository,
        service::{
            command::CreateShopCommand,
            command_service::{CommandShopError, CommandShopService, CommandShopServiceImpl},
        },
    };
    use aws_sdk_dynamodb::{
        config::http::HttpResponse,
        error::{ConnectorError, SdkError},
        operation::transact_write_items::TransactWriteItemsOutput,
    };
    use common::batch::dynamodb::BatchGetItemResult;
    use fake::{Fake, Faker};
    use url::Url;

    #[tokio::test]
    async fn should_err_when_shop_urls_empty() {
        let shop_repository = MockShopDynamoDbRepository::default();
        let service = CommandShopServiceImpl::new(&shop_repository);

        let create_cmd = CreateShopCommand {
            name: Faker.fake(),
            urls: vec![],
            image: None,
        };
        let actual = service.create(create_cmd).await;

        assert!(actual.is_err());
    }

    #[rstest::rstest]
    #[case(101)]
    #[case(110)]
    #[case(142)]
    #[case(169)]
    #[case(1234)]
    #[tokio::test]
    async fn should_err_when_shop_urls_more_than_100(#[case] count: usize) {
        let shop_repository = MockShopDynamoDbRepository::default();
        let service = CommandShopServiceImpl::new(&shop_repository);
        let create_cmd = CreateShopCommand {
            name: Faker.fake(),
            urls: fake::vec![Url; count],
            image: None,
        };
        let actual = service.create(create_cmd).await;

        assert!(actual.is_err());
    }

    #[tokio::test]
    async fn should_err_when_unprocessed() {
        let mut shop_repository = MockShopDynamoDbRepository::default();
        shop_repository.expect_get_shop_records().return_once(|_| {
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: vec![],
                    unprocessed: Some(Faker.fake()),
                })
            })
        });

        let service = CommandShopServiceImpl::new(&shop_repository);
        let create_cmd: CreateShopCommand = Faker.fake();
        let actual = service.create(create_cmd.clone()).await;

        assert!(actual.is_err());
    }

    #[tokio::test]
    async fn should_create_shop_when_not_exists_and_none_unprocessed() {
        let mut shop_repository = MockShopDynamoDbRepository::default();
        shop_repository.expect_get_shop_records().return_once(|_| {
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: vec![],
                    unprocessed: None,
                })
            })
        });
        shop_repository
            .expect_put_shop_records_transact()
            .return_once(|_| Box::pin(async { Ok(TransactWriteItemsOutput::builder().build()) }));

        let service = CommandShopServiceImpl::new(&shop_repository);

        let create_cmd: CreateShopCommand = Faker.fake();
        let actual = service.create(create_cmd.clone()).await.unwrap();

        assert_eq!(create_cmd.name, actual.name);
        assert_eq!(create_cmd.image, actual.image);
        assert_eq!(create_cmd.urls, actual.urls);
    }

    #[tokio::test]
    async fn should_transact_put_n_plus_1_records() {
        let mut shop_repository = MockShopDynamoDbRepository::default();
        shop_repository.expect_get_shop_records().return_once(|_| {
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: vec![],
                    unprocessed: None,
                })
            })
        });

        let create_cmd: CreateShopCommand = Faker.fake();
        let create_cmd_clone: CreateShopCommand = create_cmd.clone();
        shop_repository
            .expect_put_shop_records_transact()
            .return_once(move |cmd| {
                Box::pin(async move {
                    assert_eq!(create_cmd_clone.urls.len() + 1, cmd.len());
                    Ok(TransactWriteItemsOutput::builder().build())
                })
            });

        let service = CommandShopServiceImpl::new(&shop_repository);

        let _ = service.create(create_cmd.clone()).await.unwrap();
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
    async fn should_propagate_sdk_error_for_batch_get(
        #[case] expected: SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError>,
    ) {
        let mut shop_repository = MockShopDynamoDbRepository::default();
        shop_repository
            .expect_get_shop_records()
            .return_once(|_| Box::pin(async { Err(expected) }));
        let service = CommandShopServiceImpl::new(&shop_repository);

        let actual = service.create(Faker.fake()).await;

        assert!(actual.is_err());
        match actual.unwrap_err() {
            CommandShopError::SdkBatchGetItemError(_) => {}
            _ => panic!("expected CommandShopError::SdkBatchGetItemError"),
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
            aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError::unhandled("Something went wrong"),
            HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
    async fn should_propagate_sdk_error_for_transact_write(
        #[case] expected: SdkError<
            aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError,
        >,
    ) {
        let mut shop_repository = MockShopDynamoDbRepository::default();
        shop_repository.expect_get_shop_records().return_once(|_| {
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: vec![],
                    unprocessed: None,
                })
            })
        });
        shop_repository
            .expect_put_shop_records_transact()
            .return_once(|_| Box::pin(async { Err(expected) }));
        let service = CommandShopServiceImpl::new(&shop_repository);

        let actual = service.create(Faker.fake()).await;

        assert!(actual.is_err());
        match actual.unwrap_err() {
            CommandShopError::SdkTransactWriteItemsError(_) => {}
            _ => panic!("expected CommandShopError::SdkTransactWriteItemsError"),
        }
    }
}
