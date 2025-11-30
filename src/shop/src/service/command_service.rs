use std::collections::HashMap;

use crate::{
    core::shop::Shop,
    dynamodb::{
        repository::ShopDynamoDbRepository,
        shop_record::{ShopRecord, mk_pk_as_shop_host},
        shop_record_update::ShopRecordUpdate,
    },
    service::command::{CreateShopCommand, UpdateShopCommand},
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
    #[error("Shop with identifier '{0}' not found")]
    ShopNotFound(ShopIdentifier),

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
    use common::api::error_code::{
        SHOP_EXISTS_ALREADY, SHOP_NOT_FOUND, SHOP_TOO_MANY_URLS, UNPROCESSED_ITEMS,
    };

    impl From<CommandShopError> for ApiError {
        fn from(err: CommandShopError) -> Self {
            match err {
                CommandShopError::ShopNotFound(_) => {
                    ApiError::conflict(SHOP_NOT_FOUND, Box::new(err))
                }
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
    async fn update(
        &self,
        shop_identifier: &ShopIdentifier,
        command: UpdateShopCommand,
    ) -> Result<Shop, CommandShopError>;
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

    async fn update(
        &self,
        shop_identifier: &ShopIdentifier,
        command: UpdateShopCommand,
    ) -> Result<Shop, CommandShopError> {
        let shop_record = match shop_identifier {
            ShopIdentifier::ShopId(shop_id) => self.repository.get_shop_record_by_id(shop_id),
            ShopIdentifier::ShopUrl(url) => self.repository.get_shop_record_by_url(url),
        }
        .await?
        .ok_or_else(|| CommandShopError::ShopNotFound(shop_identifier.clone()))?;

        if command.is_empty() {
            return Ok(shop_record.into());
        }

        let mut existing_shop_identifiers = shop_record
            .urls
            .clone()
            .into_iter()
            .map(ShopIdentifier::from)
            .collect::<Vec<_>>();
        existing_shop_identifiers.push(ShopIdentifier::from(shop_record.shop_id));
        let existing_shop_identifiers = Batch::try_from(existing_shop_identifiers)?;
        let existing_shop_records = self
            .repository
            .get_shop_records(&existing_shop_identifiers)
            .await?;
        if existing_shop_records.unprocessed.is_some() {
            return Err(CommandShopError::SdkBatchGetItemUnprocessed);
        }

        let update_record = ShopRecordUpdate {
            name: command.name,
            urls: command.urls,
            image: command.image,
            updated: OffsetDateTime::now_utc(),
        };

        let mut put = vec![];
        let mut update = HashMap::new();
        let mut delete = vec![];
        for existing_shop_record in existing_shop_records.items {
            match existing_shop_record.url {
                None => {
                    // is a Shop-Id-Record
                    update.insert(
                        ShopIdentifier::from(existing_shop_record.shop_id),
                        update_record.clone(),
                    );
                }
                Some(url) => {
                    // is a Shop-Url-Record
                    match update_record.urls {
                        None => {
                            // urls don't change => no new/deleted shop-records, just update
                            update.insert(ShopIdentifier::from(url), update_record.clone());
                        }
                        Some(ref urls) => {
                            // urls change => possible new/deleted shop-records
                            if urls.contains(&url) {
                                // existing record will exist further
                                update.insert(ShopIdentifier::from(url), update_record.clone());
                            } else {
                                // record exists no more after update
                                delete.push(ShopIdentifier::from(url));
                            }
                        }
                    }
                }
            }
        }

        let new_urls = update_record
            .urls
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(ShopIdentifier::from)
            .filter(|shop_identifier| {
                !update.contains_key(shop_identifier) && !delete.contains(shop_identifier)
            })
            .filter_map(|shop_identifier| match shop_identifier {
                ShopIdentifier::ShopId(_) => None,
                ShopIdentifier::ShopUrl(url) => Some(url),
            });
        for new_url in new_urls {
            let new_shop_url_record = ShopRecord {
                pk: mk_pk_as_shop_host(&new_url)
                    .ok_or_else(|| {
                        SdkError::construction_failure(format!(
                            "Failed constructing pk for '{new_url}' in put-operation"
                        ))
                    })
                    .map_err(CommandShopError::SdkTransactWriteItemsError)?,
                sk: "shop#details".to_owned(),
                shop_id: shop_record.shop_id,
                name: update_record
                    .name
                    .clone()
                    .unwrap_or(shop_record.name.clone()),
                url: Some(new_url),
                urls: update_record
                    .urls
                    .clone()
                    .unwrap_or(shop_record.urls.clone()),
                image: update_record.image.clone().or(shop_record.image.clone()),
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };
            put.push(new_shop_url_record);
        }

        let _ = self.repository.transact_write(put, update, delete).await?;

        Ok(Shop {
            shop_id: shop_record.shop_id,
            name: update_record
                .name
                .clone()
                .unwrap_or(shop_record.name.clone()),
            urls: update_record
                .urls
                .clone()
                .unwrap_or(shop_record.urls.clone()),
            image: update_record.image.clone().or(shop_record.image.clone()),
            created: shop_record.created,
            updated: OffsetDateTime::now_utc(),
        })
    }
}

#[cfg(test)]
mod tests {

    mod create {
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
        use std::collections::HashSet;
        use url::Url;

        #[tokio::test]
        async fn should_err_when_shop_urls_empty() {
            let shop_repository = MockShopDynamoDbRepository::default();
            let service = CommandShopServiceImpl::new(&shop_repository);

            let create_cmd = CreateShopCommand {
                name: Faker.fake(),
                urls: HashSet::new(),
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
                urls: (0..count)
                    .map(|i| Url::parse(&format!("https:/foo.com/{i}")).unwrap())
                    .collect(),
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
                .return_once(|_| {
                    Box::pin(async { Ok(TransactWriteItemsOutput::builder().build()) })
                });

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
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
            >,
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

    mod update {
        use crate::{
            core::shop::Shop,
            dynamodb::{
                repository::MockShopDynamoDbRepository, shop_record::ShopRecord,
                shop_record_update::ShopRecordUpdate,
            },
            service::{
                command::UpdateShopCommand,
                command_service::{CommandShopService, CommandShopServiceImpl},
            },
        };
        use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput;
        use common::{batch::dynamodb::BatchGetItemResult, shop_id::ShopIdentifier};
        use fake::{Fake, Faker};
        use std::collections::HashSet;
        use url::Url;

        #[tokio::test]
        async fn should_no_op_when_command_is_empty_for_shop_identifier_id() {
            let expected = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from_shop_as_shop_id_record(expected.clone());

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));

            let service = CommandShopServiceImpl::new(&shop_repository);
            let actual = service
                .update(&ShopIdentifier::from(expected.shop_id), Default::default())
                .await
                .unwrap();

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_no_op_when_command_is_empty_for_shop_identifier_url() {
            let mut expected = Faker.fake::<Shop>();
            expected.urls = [Url::parse("https://foo.bar").unwrap()].into();
            let shop_record = ShopRecord::try_clone_from_shop_as_shop_url_records(&expected)
                .unwrap()
                .first()
                .unwrap()
                .clone();

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record_by_url()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));

            let service = CommandShopServiceImpl::new(&shop_repository);
            let actual = service
                .update(
                    &ShopIdentifier::from(Url::parse("https://foo.bar").unwrap()),
                    Default::default(),
                )
                .await
                .unwrap();

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_update_for_shop_identifier_id_when_just_update() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from_shop_as_shop_id_record(shop.clone());
            let shop_identifiers = shop_record.shop_identifiers();
            let mut shop_records =
                ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
            shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository
                .expect_get_shop_records()
                .return_once(move |_| {
                    Box::pin(async move {
                        Ok(BatchGetItemResult {
                            items: shop_records,
                            unprocessed: None,
                        })
                    })
                });
            shop_repository
                .expect_transact_write()
                .return_once(move |put, update, delete| {
                    assert!(update.values().all(|update_record: &ShopRecordUpdate| {
                        update_record.name == Some("Hanses shoppy".into())
                            && update_record.urls.is_none()
                            && update_record.image
                                == Some(Url::parse("https://hanses.shoppy/img/foo").unwrap())
                    }));
                    assert!(put.is_empty());
                    assert!(delete.is_empty());
                    assert_eq!(
                        shop_identifiers,
                        update.keys().cloned().collect::<HashSet<_>>()
                    );
                    Box::pin(async { Ok(TransactWriteItemsOutput::builder().build()) })
                });

            let service = CommandShopServiceImpl::new(&shop_repository);
            let cmd = UpdateShopCommand {
                name: Some("Hanses shoppy".into()),
                urls: None,
                image: Some(Url::parse("https://hanses.shoppy/img/foo").unwrap()),
            };
            let actual = service
                .update(&ShopIdentifier::from(shop.shop_id), cmd)
                .await
                .unwrap();

            assert_eq!("Hanses shoppy", actual.name.to_string());
            assert_eq!(
                "https://hanses.shoppy/img/foo",
                actual.image.unwrap().to_string()
            )
        }

        #[tokio::test]
        async fn should_update_for_shop_identifier_id_when_put() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from_shop_as_shop_id_record(shop.clone());
            let shop_identifiers = shop_record.shop_identifiers();
            let mut shop_urls = shop_record.urls.clone();
            let mut shop_records =
                ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
            shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository
                .expect_get_shop_records()
                .return_once(move |_| {
                    Box::pin(async move {
                        Ok(BatchGetItemResult {
                            items: shop_records,
                            unprocessed: None,
                        })
                    })
                });
            shop_repository
                .expect_transact_write()
                .return_once(move |put, update, delete| {
                    assert!(update.values().all(|update_record: &ShopRecordUpdate| {
                        update_record
                            .urls
                            .clone()
                            .unwrap()
                            .iter()
                            .any(|url| url.as_str() == "https://what-da-helly.com/")
                    }));
                    assert_eq!(1, put.len());
                    assert_eq!(
                        "https://what-da-helly.com/",
                        put.first().unwrap().clone().url.unwrap().as_str()
                    );
                    assert!(delete.is_empty());
                    assert_eq!(
                        shop_identifiers,
                        update.keys().cloned().collect::<HashSet<_>>()
                    );
                    Box::pin(async { Ok(TransactWriteItemsOutput::builder().build()) })
                });

            shop_urls.insert(Url::parse("https://what-da-helly.com/").unwrap());
            let cmd = UpdateShopCommand {
                name: None,
                urls: Some(shop_urls.clone()),
                image: None,
            };
            let service = CommandShopServiceImpl::new(&shop_repository);
            let actual = service
                .update(&ShopIdentifier::from(shop.shop_id), cmd)
                .await
                .unwrap();

            assert_eq!(shop_urls, actual.urls);
        }

        #[tokio::test]
        async fn should_update_for_shop_identifier_id_when_delete() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from_shop_as_shop_id_record(shop.clone());
            let shop_identifiers = shop_record.shop_identifiers();
            let mut shop_urls = shop_record.urls.clone();
            let mut shop_records =
                ShopRecord::try_clone_from_shop_as_shop_url_records(&shop).unwrap();
            shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record_by_id()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository
                .expect_get_shop_records()
                .return_once(move |_| {
                    Box::pin(async move {
                        Ok(BatchGetItemResult {
                            items: shop_records,
                            unprocessed: None,
                        })
                    })
                });
            shop_repository
                .expect_transact_write()
                .return_once(move |put, update, delete| {
                    assert_eq!(1, delete.len());
                    assert!(put.is_empty());
                    assert_eq!(shop_identifiers.len() - 1, update.keys().len());
                    Box::pin(async { Ok(TransactWriteItemsOutput::builder().build()) })
                });

            shop_urls = shop_urls
                .clone()
                .into_iter()
                .take(shop_urls.len() - 1)
                .collect::<HashSet<_>>();
            let cmd = UpdateShopCommand {
                name: None,
                urls: Some(shop_urls.clone()),
                image: None,
            };
            let service = CommandShopServiceImpl::new(&shop_repository);
            let actual = service
                .update(&ShopIdentifier::from(shop.shop_id), cmd)
                .await
                .unwrap();

            assert_eq!(shop_urls, actual.urls);
        }
    }
}
