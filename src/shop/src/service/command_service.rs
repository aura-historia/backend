use crate::{
    core::{
        affiliate_configuration::AffiliateConfiguration, partner_status::ShopPartnerStatus,
        shop::Shop,
    },
    dynamodb::{repository::ShopDynamoDbRepository, shop_record_update::ShopRecordUpdate},
    service::command::{CreateShopCommand, UpdateShopCommand},
};
use aws_sdk_dynamodb::error::SdkError;
use common::{
    actor::{domain::Actor, RequestContext},
    shop_id::ShopId,
    shop_name::ShopName,
    slug_id::SlugId,
    user_id::UserId,
};
use time::OffsetDateTime;
use tracing::info;

use super::geocoding_service::{GeocodingError, GeocodingService};

#[derive(Debug, thiserror::Error)]
#[allow(clippy::large_enum_variant)]
pub enum CommandShopError {
    #[error("Shop with id '{0}' not found")]
    ShopNotFound(ShopId),

    #[error("Shop with name '{0}' exists already - the shop-slug '{1}' is already registered.")]
    ShopSlugExistsAlready(ShopName, SlugId<0>),

    #[error("Shop '{0}' is not a partner shop")]
    NotAPartnerShop(ShopId),

    #[error("User '{0}' is not the partner of shop '{1}'")]
    NotThePartnerUser(UserId, ShopId),

    #[error(
        "Did not succeed checking existence of shop due to DynamoDB Batch-Response containing unprocessed items"
    )]
    SdkBatchGetItemUnprocessed,

    #[error("Encountered DynamoDB SdkError for GetItem: {0:?}")]
    SdkGetItemError(#[from] SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>),

    #[error("Encountered DynamoDB SdkError for BatchGetItem: {0:?}")]
    SdkBatchGetItemError(
        #[from] SdkError<aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError>,
    ),

    #[error("Encountered DynamoDB SdkError for Query: {0:?}")]
    SdkQueryError(#[from] SdkError<aws_sdk_dynamodb::operation::query::QueryError>),

    #[error("Encountered DynamoDB SdkError for PutItem: {0:?}")]
    SdkPutItemError(#[from] SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>),

    #[error("Encountered DynamoDB SdkError for UpdateItem: {0:?}")]
    SdkUpdateItemError(#[from] SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError>),

    #[error("Failed to geocode shop address: {0}")]
    GeocodingError(#[from] GeocodingError),
}

#[cfg(feature = "data")]
pub mod api {
    use crate::service::command_service::CommandShopError;
    use common::api::error::ApiError;
    use common::api::error_code::{
        PARTNER_SHOP_NOT_PARTNERED, SHOP_EXISTS_ALREADY, SHOP_NOT_FOUND, UNPROCESSED_ITEMS,
    };

    impl From<CommandShopError> for ApiError {
        fn from(err: CommandShopError) -> Self {
            match err {
                CommandShopError::ShopNotFound(_) => {
                    ApiError::not_found(SHOP_NOT_FOUND, Box::new(err))
                }
                CommandShopError::ShopSlugExistsAlready(_, _) => {
                    ApiError::conflict(SHOP_EXISTS_ALREADY, Box::new(err))
                }
                CommandShopError::NotAPartnerShop(_) => {
                    ApiError::forbidden(PARTNER_SHOP_NOT_PARTNERED).with_detail(err.to_string())
                }
                CommandShopError::NotThePartnerUser(_, _) => {
                    ApiError::forbidden(PARTNER_SHOP_NOT_PARTNERED).with_detail(err.to_string())
                }
                CommandShopError::SdkBatchGetItemUnprocessed => {
                    ApiError::service_unavailable(UNPROCESSED_ITEMS, Box::new(err))
                }
                CommandShopError::SdkGetItemError(err) => err.into(),
                CommandShopError::SdkBatchGetItemError(err) => err.into(),
                CommandShopError::SdkQueryError(err) => err.into(),
                CommandShopError::SdkPutItemError(err) => err.into(),
                CommandShopError::SdkUpdateItemError(err) => err.into(),
                CommandShopError::GeocodingError(err) => {
                    ApiError::bad_request(common::api::error_code::BAD_BODY_VALUE, Box::new(err))
                }
            }
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait CommandShopService {
    async fn create(
        &self,
        ctx: &RequestContext,
        command: CreateShopCommand,
    ) -> Result<Shop, CommandShopError>;
    async fn update(
        &self,
        ctx: &RequestContext,
        shop_id: &ShopId,
        command: UpdateShopCommand,
    ) -> Result<Shop, CommandShopError>;
}

pub struct CommandShopServiceImpl<'a> {
    repository: &'a (dyn ShopDynamoDbRepository + Sync),
    geocoding_service: &'a (dyn GeocodingService + Sync),
}

impl<'a> CommandShopServiceImpl<'a> {
    pub fn new(
        repository: &'a (dyn ShopDynamoDbRepository + Sync),
        geocoding_service: &'a (dyn GeocodingService + Sync),
    ) -> Self {
        Self {
            repository,
            geocoding_service,
        }
    }
}

#[async_trait::async_trait]
impl<'a> CommandShopService for CommandShopServiceImpl<'a> {
    async fn create(
        &self,
        ctx: &RequestContext,
        command: CreateShopCommand,
    ) -> Result<Shop, CommandShopError> {
        let shop_slug_id = SlugId::from(command.name.as_ref());
        let existing_shop_opt = self.repository.query_shop_id(&shop_slug_id).await?;
        if existing_shop_opt.is_some() {
            return Err(CommandShopError::ShopSlugExistsAlready(
                command.name,
                shop_slug_id,
            ));
        }

        let geo_address = match command.structured_address.as_ref() {
            Some(address) => Some(self.geocoding_service.geocode(address).await?),
            None => None,
        };

        let view_url = command.url.as_ref().map(|u| {
            command
                .affiliate_configuration
                .as_ref()
                .map(|a| a.build_url(u))
                .unwrap_or_else(|| crate::dynamodb::utm::append_utm_params(u.clone()))
        });
        let shop = Shop {
            shop_id: ShopId::new(),
            shop_slug_id: SlugId::from(command.name.as_ref()),
            name: command.name,
            shop_type: command.shop_type,
            domains: command.domains,
            shopify_domain: command.shopify_domain,
            shopify_currency: command.shopify_currency,
            shopify_language: command.shopify_language,
            woocommerce_webhook_secret: command.woocommerce_webhook_secret,
            woocommerce_currency: command.woocommerce_currency,
            woocommerce_language: command.woocommerce_language,
            url: command.url,
            view_url,
            image: command.image,
            structured_address: command.structured_address,
            geo_address,
            phone: command.phone,
            email: command.email,
            partner_status: ShopPartnerStatus::default(),
            affiliate_configuration: command.affiliate_configuration,
            created_by: ctx.actor,
            updated_by: ctx.actor,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let _ = self.repository.put_shop_record(shop.clone().into()).await?;

        info!(actor = %ctx.actor, shopId = %shop.shop_id, name = %shop.name, slug = %shop.shop_slug_id, domains = ?shop.domains, "Created Shop.");

        Ok(shop)
    }

    async fn update(
        &self,
        ctx: &RequestContext,
        shop_id: &ShopId,
        command: UpdateShopCommand,
    ) -> Result<Shop, CommandShopError> {
        let shop_record = self
            .repository
            .get_shop_record(shop_id)
            .await?
            .ok_or_else(|| CommandShopError::ShopNotFound(*shop_id))?;

        if command.is_empty() {
            return Ok(shop_record.into());
        }

        let geo_address = match command.structured_address.as_ref() {
            Some(address) => Some(self.geocoding_service.geocode(address).await?),
            None => None,
        };

        let update = ShopRecordUpdate {
            gsi3_pk: command
                .shopify_domain
                .as_ref()
                .map(crate::dynamodb::shop_record::mk_gsi3_pk),
            gsi3_sk: command
                .shopify_domain
                .as_ref()
                .map(|_| crate::dynamodb::shop_record::mk_gsi3_sk().to_owned()),
            shop_type: command.shop_type.map(Into::into),
            shop_partner_status: command.shop_partner_status.map(Into::into),
            domains: command.domains.clone(),
            shopify_domain: command.shopify_domain.clone(),
            shopify_currency: command.shopify_currency.map(Into::into),
            shopify_language: command.shopify_language.map(Into::into),
            woocommerce_webhook_secret: command.woocommerce_webhook_secret.clone(),
            woocommerce_currency: command.woocommerce_currency.map(Into::into),
            woocommerce_language: command.woocommerce_language.map(Into::into),
            url: command.url.clone(),
            view_url: command.url.as_ref().map(|u| {
                shop_record
                    .affiliate_configuration
                    .as_ref()
                    .cloned()
                    .map(|a| AffiliateConfiguration::from(a).build_url(u))
                    .unwrap_or_else(|| crate::dynamodb::utm::append_utm_params(u.clone()))
            }),
            image: command.image.clone(),
            structured_address_addressline: command
                .structured_address
                .as_ref()
                .and_then(|a| a.addressline.clone()),
            structured_address_addressline_extra: command
                .structured_address
                .as_ref()
                .and_then(|a| a.addressline_extra.clone()),
            structured_address_locality: command
                .structured_address
                .as_ref()
                .and_then(|address| address.locality.clone()),
            structured_address_region: command
                .structured_address
                .as_ref()
                .and_then(|address| address.region.clone()),
            structured_address_postal_code: command
                .structured_address
                .as_ref()
                .and_then(|address| address.postal_code.clone()),
            structured_address_country: command.structured_address.as_ref().and_then(|a| a.country),
            geo_address_lat: geo_address.as_ref().map(|address| address.lat),
            geo_address_lon: geo_address.as_ref().map(|address| address.lon),
            phone: command.phone.clone(),
            email: command.email.clone(),
            updated_by: ctx.actor.into(),
            updated: OffsetDateTime::now_utc(),
        };
        let shop_record = self
            .repository
            .update_shop_record(shop_id, update)
            .await?
            .ok_or_else(|| {
                CommandShopError::SdkUpdateItemError(SdkError::construction_failure(
                    "failed retrieving new shop on update",
                ))
            })?;

        info!(actor = %ctx.actor, shopId = %shop_record.shop_id, name = %shop_record.name, slug = %shop_record.shop_slug_id, payload = ?command, "Updated Shop.");

        Ok(shop_record.into())
    }
}

#[cfg(test)]
mod tests {
    mod create {
        use crate::{
            core::{
                address::{GeoAddress, StructuredAddress},
                continent::Continent,
            },
            dynamodb::repository::MockShopDynamoDbRepository,
            service::{
                command::CreateShopCommand,
                command_service::{CommandShopError, CommandShopService, CommandShopServiceImpl},
                geocoding_service::MockGeocodingService,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
            operation::put_item::PutItemOutput,
        };
        use fake::{Fake, Faker};
        use isocountry::CountryCode;
        use std::collections::HashSet;

        #[tokio::test]
        async fn should_err_when_shop_slug_exists_already() {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_query_shop_id()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let cmd = CreateShopCommand {
                name: Faker.fake(),
                shop_type: Faker.fake(),
                shop_partner_status: Faker.fake(),
                domains: HashSet::new(),
                shopify_domain: None,
                shopify_currency: None,
                shopify_language: None,
                woocommerce_webhook_secret: None,
                woocommerce_currency: None,
                woocommerce_language: None,
                url: None,
                image: None,
                structured_address: None,
                phone: None,
                email: None,
                affiliate_configuration: None,
            };
            let actual = service.create(cmd).await.unwrap_err();
            match actual {
                CommandShopError::ShopSlugExistsAlready(_, _) => {}
                other => {
                    panic!("Expected 'CommandShopError::ShopSlugExistsAlready'. Got '{other}'")
                }
            }
        }

        #[tokio::test]
        async fn should_create_shop_when_slug_not_exists() {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_query_shop_id()
                .return_once(|_| Box::pin(async { Ok(None) }));
            shop_repository
                .expect_put_shop_record()
                .return_once(|_| Box::pin(async { Ok(PutItemOutput::builder().build()) }));

            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let create_cmd: CreateShopCommand = Faker.fake();
            let actual = service.create(create_cmd.clone()).await.unwrap();

            assert_eq!(create_cmd.name, actual.name);
            assert_eq!(create_cmd.shop_type, actual.shop_type);
            assert_eq!(create_cmd.image, actual.image);
            assert_eq!(create_cmd.domains, actual.domains);
        }

        #[tokio::test]
        async fn should_geocode_structured_address_when_creating_shop() {
            let structured_address = StructuredAddress {
                addressline: Some("Pariser Platz 1".to_string()),
                addressline_extra: None,
                locality: Some("Berlin".to_string()),
                region: None,
                postal_code: Some("10117".to_string()),
                country: Some(CountryCode::DEU),
                continent: Some(Continent::Europe),
            };
            let geo_address = GeoAddress {
                lat: 52.516275,
                lon: 13.377704,
            };
            let mut geocoding_service = MockGeocodingService::default();
            geocoding_service
                .expect_geocode()
                .withf(|address| address.locality.as_deref() == Some("Berlin"))
                .return_once(move |_| Box::pin(async move { Ok(geo_address) }));

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_query_shop_id()
                .return_once(|_| Box::pin(async { Ok(None) }));
            shop_repository
                .expect_put_shop_record()
                .return_once(|record| {
                    assert_eq!(Some(52.516275), record.geo_address_lat);
                    assert_eq!(Some(13.377704), record.geo_address_lon);
                    Box::pin(async { Ok(PutItemOutput::builder().build()) })
                });

            let service = CommandShopServiceImpl::new(&shop_repository, &geocoding_service);
            let cmd = CreateShopCommand {
                name: Faker.fake(),
                shop_type: Faker.fake(),
                shop_partner_status: Faker.fake(),
                domains: HashSet::new(),
                shopify_domain: None,
                shopify_currency: None,
                shopify_language: None,
                woocommerce_webhook_secret: None,
                woocommerce_currency: None,
                woocommerce_language: None,
                url: None,
                image: None,
                structured_address: Some(structured_address),
                phone: None,
                email: None,
                affiliate_configuration: None,
            };

            let actual = service.create(cmd).await.unwrap();

            assert_eq!(Some(geo_address), actual.geo_address);
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
            #[case] expected: SdkError<aws_sdk_dynamodb::operation::query::QueryError>,
        ) {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_query_shop_id()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let actual = service.create(Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                CommandShopError::SdkQueryError(_) => {}
                _ => panic!("expected CommandShopError::SdkQueryError"),
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
                aws_sdk_dynamodb::operation::put_item::PutItemError::unhandled("Something went wrong"),
                HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
            ))]
        #[trace]
        async fn should_propagate_sdk_error_for_put_item(
            #[case] expected: SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>,
        ) {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_query_shop_id()
                .return_once(|_| Box::pin(async { Ok(None) }));
            shop_repository
                .expect_put_shop_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let actual = service.create(Faker.fake()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                CommandShopError::SdkPutItemError(_) => {}
                _ => panic!("expected CommandShopError::SdkPutItemError"),
            }
        }
    }

    mod update {
        use crate::{
            core::{
                address::{GeoAddress, StructuredAddress},
                continent::Continent,
                shop::Shop,
            },
            dynamodb::{
                repository::MockShopDynamoDbRepository, shop_record::ShopRecord,
                shop_record_update::ShopRecordUpdate,
            },
            service::{
                command::UpdateShopCommand,
                command_service::{CommandShopError, CommandShopService, CommandShopServiceImpl},
                geocoding_service::MockGeocodingService,
            },
        };
        use aws_sdk_dynamodb::{
            config::http::HttpResponse,
            error::{ConnectorError, SdkError},
        };
        use common::{domain::Domain, shop_id::ShopId};
        use fake::{Fake, Faker};
        use isocountry::CountryCode;

        use url::Url;

        #[tokio::test]
        async fn should_err_when_shop_not_found() {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(|_| Box::pin(async { Ok(None) }));
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let actual = service.update(&ShopId::new(), Default::default()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                CommandShopError::ShopNotFound(_) => {}
                other => panic!("Expected 'CommandShopError::ShopNotFound'. Got '{other}'"),
            }
        }

        #[tokio::test]
        async fn should_no_op_when_command_is_empty() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from(shop);
            let expected = Shop::from(shop_record.clone());

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository.expect_update_shop_record().never();

            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );
            let actual = service
                .update(&expected.shop_id, Default::default())
                .await
                .unwrap();

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_update_image_when_image_command() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from(shop.clone());
            let new_image_url = Url::parse("https://hanses.shoppy/img/foo").unwrap();
            let mut updated_shop = shop.clone();
            updated_shop.image = Some(new_image_url.clone());
            let updated_record = ShopRecord::from(updated_shop);

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository.expect_update_shop_record().return_once(
                move |_, update: ShopRecordUpdate| {
                    assert!(update.shop_type.is_none());
                    assert!(update.domains.is_none());
                    assert_eq!(
                        update.image,
                        Some(Url::parse("https://hanses.shoppy/img/foo").unwrap())
                    );
                    Box::pin(async move { Ok(Some(updated_record)) })
                },
            );

            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );
            let cmd = UpdateShopCommand {
                shop_type: None,
                domains: None,
                image: Some(new_image_url),
                ..Default::default()
            };
            let actual = service.update(&shop.shop_id, cmd).await.unwrap();

            assert_eq!(
                "https://hanses.shoppy/img/foo",
                actual.image.unwrap().to_string()
            );
        }

        #[tokio::test]
        async fn should_update_domains_when_domain_added() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from(shop.clone());
            let mut new_domains = shop.domains.clone();
            new_domains.insert(Domain::try_from("https://what-da-helly.com/").unwrap());

            let mut updated_shop = shop.clone();
            updated_shop.domains = new_domains.clone();
            let updated_record = ShopRecord::from(updated_shop);

            let expected_domains = new_domains.clone();
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository.expect_update_shop_record().return_once(
                move |_, update: ShopRecordUpdate| {
                    assert_eq!(update.domains, Some(expected_domains));
                    assert!(update.shop_type.is_none());
                    assert!(update.image.is_none());
                    Box::pin(async move { Ok(Some(updated_record)) })
                },
            );

            let cmd = UpdateShopCommand {
                shop_type: None,
                domains: Some(new_domains.clone()),
                shopify_domain: None,
                image: None,
                ..Default::default()
            };
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );
            let actual = service.update(&shop.shop_id, cmd).await.unwrap();

            assert_eq!(new_domains, actual.domains);
        }

        #[tokio::test]
        async fn should_update_domains_when_domain_removed() {
            let mut shop = Faker.fake::<Shop>();
            shop.domains
                .insert(Domain::try_from("https://extra-one.com/").unwrap());
            shop.domains
                .insert(Domain::try_from("https://to-be-removed.com/").unwrap());

            let shop_record = ShopRecord::from(shop.clone());
            let mut reduced_domains = shop.domains.clone();
            reduced_domains.remove(&Domain::try_from("https://to-be-removed.com/").unwrap());

            let mut updated_shop = shop.clone();
            updated_shop.domains = reduced_domains.clone();
            let updated_record = ShopRecord::from(updated_shop);

            let expected_domains = reduced_domains.clone();
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));

            shop_repository.expect_update_shop_record().return_once(
                move |_, update: ShopRecordUpdate| {
                    assert_eq!(update.domains, Some(expected_domains));
                    assert!(update.shop_type.is_none());
                    assert!(update.image.is_none());
                    Box::pin(async move { Ok(Some(updated_record)) })
                },
            );

            let cmd = UpdateShopCommand {
                shop_type: None,
                domains: Some(reduced_domains.clone()),
                shopify_domain: None,
                image: None,
                ..Default::default()
            };
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );
            let actual = service.update(&shop.shop_id, cmd).await.unwrap();

            assert_eq!(reduced_domains, actual.domains);
        }

        #[tokio::test]
        async fn should_geocode_structured_address_when_updating_shop() {
            let shop = Faker.fake::<Shop>();
            let shop_record = ShopRecord::from(shop.clone());
            let structured_address = StructuredAddress {
                addressline: Some("1600 Amphitheatre Parkway".to_string()),
                addressline_extra: None,
                locality: Some("Mountain View".to_string()),
                region: Some("CA".to_string()),
                postal_code: None,
                country: Some(CountryCode::USA),
                continent: Some(Continent::NorthAmerica),
            };
            let geo_address = GeoAddress {
                lat: 37.422,
                lon: -122.084,
            };
            let mut updated_shop = shop.clone();
            updated_shop.structured_address = Some(structured_address.clone());
            updated_shop.geo_address = Some(geo_address);
            let updated_record = ShopRecord::from(updated_shop);

            let mut geocoding_service = MockGeocodingService::default();
            geocoding_service
                .expect_geocode()
                .return_once(move |_| Box::pin(async move { Ok(geo_address) }));

            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(move |_| Box::pin(async move { Ok(Some(shop_record)) }));
            shop_repository.expect_update_shop_record().return_once(
                move |_, update: ShopRecordUpdate| {
                    assert_eq!(Some(37.422), update.geo_address_lat);
                    assert_eq!(Some(-122.084), update.geo_address_lon);
                    assert_eq!(
                        Some("1600 Amphitheatre Parkway".to_string()),
                        update.structured_address_addressline
                    );
                    Box::pin(async move { Ok(Some(updated_record)) })
                },
            );

            let service = CommandShopServiceImpl::new(&shop_repository, &geocoding_service);
            let actual = service
                .update(
                    &shop.shop_id,
                    UpdateShopCommand {
                        structured_address: Some(structured_address),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();

            assert_eq!(Some(geo_address), actual.geo_address);
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
            #[case] expected: SdkError<aws_sdk_dynamodb::operation::get_item::GetItemError>,
        ) {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(|_| Box::pin(async { Err(expected) }));
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let actual = service.update(&ShopId::new(), Default::default()).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                CommandShopError::SdkGetItemError(_) => {}
                _ => panic!("expected CommandShopError::SdkGetItemError"),
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
                aws_sdk_dynamodb::operation::update_item::UpdateItemError::unhandled("Something went wrong"),
                HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
            ))]
        #[trace]
        async fn should_propagate_sdk_error_for_update_item(
            #[case] expected: SdkError<aws_sdk_dynamodb::operation::update_item::UpdateItemError>,
        ) {
            let mut shop_repository = MockShopDynamoDbRepository::default();
            shop_repository
                .expect_get_shop_record()
                .return_once(|_| Box::pin(async { Ok(Some(Faker.fake())) }));
            shop_repository
                .expect_update_shop_record()
                .return_once(|_, _| Box::pin(async { Err(expected) }));
            let service = CommandShopServiceImpl::new(
                &shop_repository,
                &crate::service::geocoding_service::NoopGeocodingService,
            );

            let cmd = UpdateShopCommand {
                shop_type: None,
                domains: None,
                image: Some(Url::parse("https://example.com/img").unwrap()),
                ..Default::default()
            };
            let actual = service.update(&ShopId::new(), cmd).await;

            assert!(actual.is_err());
            match actual.unwrap_err() {
                CommandShopError::SdkUpdateItemError(_) => {}
                _ => panic!("expected CommandShopError::SdkUpdateItemError"),
            }
        }
    }
}
