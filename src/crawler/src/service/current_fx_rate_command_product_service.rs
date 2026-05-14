use async_trait::async_trait;
use common::product_id::ProductKey;
use fxrate::{
    dynamodb::record::FxRatesRecord,
    service::{FxRateService, FxRateServiceError},
};
use product::{
    dynamodb::repository::ProductDynamoDbRepository,
    service::{
        command_service::{CommandProductService, CommandProductServiceImpl},
        product_command::{CreateProductCommand, UpdateProductCommand, UpsertProductCommand},
    },
};
use shop::service::{get_service::GetShopService, seller_service::SellerService};
use std::collections::HashMap;
use tracing::warn;

pub struct CurrentFxRateCommandProductService<'a> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    fx_rate_service: &'a (dyn FxRateService + Sync),
    get_shop_service: &'a (dyn GetShopService + Sync),
    seller_service: &'a (dyn SellerService + Sync),
}

impl<'a> CurrentFxRateCommandProductService<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        fx_rate_service: &'a (dyn FxRateService + Sync),
        get_shop_service: &'a (dyn GetShopService + Sync),
        seller_service: &'a (dyn SellerService + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate_service,
            get_shop_service,
            seller_service,
        }
    }

    async fn get_current_fx_rate(
        &self,
        operation: &'static str,
        command_count: usize,
    ) -> Result<FxRatesRecord, FxRateServiceError> {
        self.fx_rate_service.get_current().await.map_err(|err| {
            warn!(
                error = ?err,
                operation,
                command_count,
                "Failed loading current FX rates for crawler product command batch. Returning batch for retry."
            );
            err
        })
    }
}

#[async_trait]
impl CommandProductService for CurrentFxRateCommandProductService<'_> {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand> {
        let command_count = cmds.len();
        let fx_rate = match self.get_current_fx_rate("create", command_count).await {
            Ok(fx_rate) => fx_rate,
            Err(_) => return cmds,
        };

        let command_service = CommandProductServiceImpl::new(
            self.dynamodb_repository,
            &fx_rate,
            self.get_shop_service,
            self.seller_service,
        );
        command_service.create(cmds).await
    }

    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand> {
        let command_count = cmds.len();
        let fx_rate = match self.get_current_fx_rate("update", command_count).await {
            Ok(fx_rate) => fx_rate,
            Err(_) => return cmds,
        };

        let command_service = CommandProductServiceImpl::new(
            self.dynamodb_repository,
            &fx_rate,
            self.get_shop_service,
            self.seller_service,
        );
        command_service.update(cmds).await
    }

    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        let command_count = cmds.len();
        let fx_rate = match self.get_current_fx_rate("upsert", command_count).await {
            Ok(fx_rate) => fx_rate,
            Err(_) => return cmds,
        };

        let command_service = CommandProductServiceImpl::new(
            self.dynamodb_repository,
            &fx_rate,
            self.get_shop_service,
            self.seller_service,
        );
        command_service.upsert(cmds).await
    }
}

#[cfg(test)]
mod tests {
    use super::CurrentFxRateCommandProductService;
    use common::{
        batch::dynamodb::BatchGetItemResult,
        domain::Domain,
        language::domain::Language,
        localized::Localized,
        price::domain::{FixedFxRate, Price},
        product_id::ProductKey,
        product_state::domain::ProductState,
        shop_id::ShopId,
        shop_name::ShopName,
        shops_product_id::ShopsProductId,
        slug_id::SlugId,
    };
    use fxrate::{
        dynamodb::record::FxRatesRecord,
        service::{FxRateServiceError, MockFxRateService},
    };
    use product::{
        core::title::Title,
        dynamodb::repository::MockProductDynamoDbRepository,
        service::{
            command_service::CommandProductService,
            product_command::{CreateProductCommand, UpdateProductCommand, UpsertProductCommand},
        },
    };
    use shop::{
        core::{partner_status::ShopPartnerStatus, shop::Shop, shop_type::ShopType},
        service::{get_service::MockGetShopService, seller_service::MockSellerService},
    };
    use std::collections::HashMap;
    use time::OffsetDateTime;
    use url::Url;

    fn make_shop(shop_id: ShopId) -> Shop {
        Shop {
            shop_id,
            shop_slug_id: SlugId::from("crawler-test-shop"),
            name: ShopName::from("Crawler Test Shop"),
            shop_type: ShopType::CommercialDealer,
            domains: [Domain::try_from("example.com").unwrap()].into(),
            shopify_domain: None,
            shopify_currency: None,
            url: None,
            image: None,
            structured_address: None,
            geo_address: None,
            phone: None,
            email: None,
            partner_status: ShopPartnerStatus::Scraped,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn make_upsert_command() -> UpsertProductCommand {
        UpsertProductCommand {
            shop_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("crawler-product-1"),
            seller_name_raw: None,
            structured_address: None,
            geo_address: None,
            native_title: Some(Localized::new(Language::En, Title::from("Crawler product"))),
            native_description: None,
            native_price: Some(Price::new(
                100_u64.into(),
                common::currency::domain::Currency::Eur,
            )),
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            state: Some(ProductState::Available),
            url: Some(Url::parse("https://example.com/products/1").unwrap()),
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    fn make_create_command() -> CreateProductCommand {
        CreateProductCommand::from(make_upsert_command())
    }

    fn make_update_command() -> (ProductKey, UpdateProductCommand) {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::from("crawler-product-update-1");
        (
            ProductKey {
                shop_id,
                shops_product_id: shops_product_id.clone(),
            },
            UpdateProductCommand {
                native_price: Some(Price::new(
                    200_u64.into(),
                    common::currency::domain::Currency::Eur,
                )),
                state: Some(ProductState::Removed),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: Some(Url::parse("https://example.com/products/updated").unwrap()),
                images: Some(vec![]),
                auction_start: None,
                auction_end: None,
            },
        )
    }

    #[tokio::test]
    async fn should_upsert_with_current_fx_rate_snapshot() {
        let cmd = make_upsert_command();
        let shop = make_shop(cmd.shop_id);
        let mut repository = MockProductDynamoDbRepository::default();
        repository.expect_get_product_records().return_once(|_| {
            Box::pin(async {
                Ok(BatchGetItemResult {
                    items: vec![],
                    unprocessed: None,
                })
            })
        });
        repository
            .expect_put_product_event_records()
            .return_once(|_| {
                Box::pin(async {
                    Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                })
            });

        let mut fx_rate_service = MockFxRateService::default();
        fx_rate_service
            .expect_get_current()
            .return_once(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));

        let mut get_shop_service = MockGetShopService::default();
        get_shop_service
            .expect_find_shop()
            .return_once(move |_| Box::pin(async move { Ok(shop) }));

        let seller_service = MockSellerService::default();

        let service = CurrentFxRateCommandProductService::new(
            &repository,
            &fx_rate_service,
            &get_shop_service,
            &seller_service,
        );

        let failures = service.upsert(vec![cmd]).await;

        assert!(failures.is_empty());
    }

    #[tokio::test]
    async fn should_return_create_commands_for_retry_when_fx_rates_record_missing() {
        let cmd = make_create_command();
        let expected = cmd.clone();
        let repository = MockProductDynamoDbRepository::default();

        let mut fx_rate_service = MockFxRateService::default();
        fx_rate_service
            .expect_get_current()
            .return_once(|| Box::pin(async { Err(FxRateServiceError::FxRatesNotExists) }));

        let get_shop_service = MockGetShopService::default();
        let seller_service = MockSellerService::default();

        let service = CurrentFxRateCommandProductService::new(
            &repository,
            &fx_rate_service,
            &get_shop_service,
            &seller_service,
        );

        let failures = service.create(vec![cmd]).await;

        assert_eq!(vec![expected], failures);
    }

    #[tokio::test]
    async fn should_return_update_commands_for_retry_when_fx_rate_lookup_fails() {
        let (key, cmd) = make_update_command();
        let expected = cmd.clone();
        let repository = MockProductDynamoDbRepository::default();

        let mut fx_rate_service = MockFxRateService::default();
        fx_rate_service
            .expect_get_current()
            .return_once(|| Box::pin(async { Err(FxRateServiceError::FxratesApiError) }));

        let get_shop_service = MockGetShopService::default();
        let seller_service = MockSellerService::default();

        let service = CurrentFxRateCommandProductService::new(
            &repository,
            &fx_rate_service,
            &get_shop_service,
            &seller_service,
        );

        let failures = service.update(HashMap::from([(key.clone(), cmd)])).await;

        assert_eq!(Some(&expected), failures.get(&key));
    }

    #[tokio::test]
    async fn should_return_upsert_commands_for_retry_when_fx_rate_lookup_fails() {
        let cmd = make_upsert_command();
        let expected = cmd.clone();
        let repository = MockProductDynamoDbRepository::default();

        let mut fx_rate_service = MockFxRateService::default();
        fx_rate_service
            .expect_get_current()
            .return_once(|| Box::pin(async { Err(FxRateServiceError::FxratesApiError) }));

        let get_shop_service = MockGetShopService::default();
        let seller_service = MockSellerService::default();

        let service = CurrentFxRateCommandProductService::new(
            &repository,
            &fx_rate_service,
            &get_shop_service,
            &seller_service,
        );

        let failures = service.upsert(vec![cmd]).await;

        assert_eq!(vec![expected], failures);
    }
}
