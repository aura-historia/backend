use crate::core::product::Product;
use crate::core::product_event::{
    ProductDomainEvent, ProductEvent, ProductEventLog, ProductEventPayload,
};
use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_meta_record::ProductMetaRecord;
use crate::dynamodb::repository::ProductDynamoDbRepository;
use crate::dynamodb::utm::strip_utm_params;
use crate::service::product_command::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use async_trait::async_trait;
use common::aggregate::Aggregate;
use common::has_key::HasKey;
use common::logging::{LogEventType, LogWriteSource};
use common::price::domain::FxRate;
use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::{FxRateService, FxRateServiceError};
use shop::core::shop_type::ShopType;
use shop::service::get_service::GetShopService;
use shop::service::seller_service::SellerService;
use tracing::{error, warn};

#[async_trait]
#[mockall::automock]
pub trait CommandProductService {
    async fn create(&self, cmd: CreateProductCommand) -> Option<CreateProductCommand>;

    async fn update(
        &self,
        key: ProductKey,
        cmd: UpdateProductCommand,
    ) -> Option<(ProductKey, UpdateProductCommand)>;

    async fn upsert(&self, cmd: UpsertProductCommand) -> Option<UpsertProductCommand>;
}

pub struct CommandProductServiceImpl<'a> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    fx_rate: FxRatesRecord,
    get_shop_service: &'a (dyn GetShopService + Sync),
    seller_service: &'a (dyn SellerService + Sync),
}

struct ResolvedShopInformation {
    seller_id: ShopId,
    seller_name: ShopName,
    shop_name: ShopName,
    shop_type: ShopType,
}

impl<'a> CommandProductServiceImpl<'a> {
    pub async fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        fx_rate_service: &(dyn FxRateService + Sync),
        get_shop_service: &'a (dyn GetShopService + Sync),
        seller_service: &'a (dyn SellerService + Sync),
    ) -> Result<Self, FxRateServiceError> {
        let fx_rate = fx_rate_service.get_current().await?;
        Ok(Self {
            dynamodb_repository,
            fx_rate,
            get_shop_service,
            seller_service,
        })
    }

    fn enrich_price(&self, cmd: &mut CreateProductCommand) {
        if let Some(price) = &cmd.native_price {
            match self
                .fx_rate
                .exchange_all(price.currency, price.monetary_amount)
            {
                Ok(other) => cmd.other_price = other,
                Err(err) => {
                    warn!(error = %err, "Failed to convert native_price. Defaulting to empty.")
                }
            }
        }
        if let Some(price) = &cmd.native_price_estimate_min {
            match self
                .fx_rate
                .exchange_all(price.currency, price.monetary_amount)
            {
                Ok(other) => cmd.other_price_estimate_min = other,
                Err(err) => {
                    warn!(error = %err, "Failed to convert native_price_estimate_min. Defaulting to empty.")
                }
            }
        }
        if let Some(price) = &cmd.native_price_estimate_max {
            match self
                .fx_rate
                .exchange_all(price.currency, price.monetary_amount)
            {
                Ok(other) => cmd.other_price_estimate_max = other,
                Err(err) => {
                    warn!(error = %err, "Failed to convert native_price_estimate_max. Defaulting to empty.")
                }
            }
        }
    }

    async fn enrich_shop_information(
        &self,
        cmd: &mut CreateProductCommand,
    ) -> Option<ResolvedShopInformation> {
        let shop = match self.get_shop_service.find_shop(&cmd.shop_id).await {
            Ok(shop) => shop,
            Err(err) => {
                warn!(
                    error = ?err,
                    shopId = %cmd.shop_id,
                    shopsProductId = %cmd.shops_product_id,
                    "Failed resolving shop information for product command."
                );
                return None;
            }
        };

        if cmd.structured_address.is_none() && cmd.geo_address.is_none() {
            cmd.structured_address = shop.structured_address.clone();
            cmd.geo_address = shop.geo_address;
        }

        let (seller_id, seller_name) = match shop.shop_type {
            ShopType::AuctionPlatform | ShopType::Marketplace => {
                if let Some(raw_name) = cmd.seller_name_raw.as_deref() {
                    let shop_name = ShopName::from(raw_name);
                    match self
                        .seller_service
                        .get_seller_shop_details(&shop_name)
                        .await
                    {
                        Ok((id, _, name)) => (id, name),
                        Err(err) => {
                            warn!(
                                error = ?err,
                                shopId = %cmd.shop_id,
                                shopsProductId = %cmd.shops_product_id,
                                sellerNameRaw = raw_name,
                                "Failed resolving seller information for product command."
                            );
                            return None;
                        }
                    }
                } else {
                    (shop.shop_id, shop.name.clone())
                }
            }
            _ => (shop.shop_id, shop.name.clone()),
        };

        Some(ResolvedShopInformation {
            seller_id,
            seller_name,
            shop_name: shop.name,
            shop_type: shop.shop_type,
        })
    }

    async fn persist_events(
        &self,
        events: Vec<ProductEventRecord>,
        meta_record: ProductMetaRecord,
        expected_event_version: u64,
    ) -> bool {
        let event_logs = events
            .iter()
            .map(|record| {
                ProductEventLog::from(record)
                    .with_event_type(LogEventType::EntityWrite)
                    .with_write_source(LogWriteSource::ProductCommandService)
                    .with_msg("Persisted product event.")
            })
            .collect::<Vec<_>>();
        match self
            .dynamodb_repository
            .transact_write_product_event_records(events, meta_record, expected_event_version)
            .await
        {
            Ok(()) => {
                for event_log in event_logs {
                    event_log.log();
                }
                true
            }
            Err(err) => {
                warn!(error = ?err, "Failed writing product event transaction. Returning command for retry.");
                false
            }
        }
    }

    async fn load_product(
        &self,
        key: &ProductKey,
    ) -> Result<
        Option<(Product, u64)>,
        aws_sdk_dynamodb::error::SdkError<
            aws_sdk_dynamodb::operation::query::QueryError,
            aws_sdk_dynamodb::config::http::HttpResponse,
        >,
    > {
        let event_records = self
            .dynamodb_repository
            .query_product_event_records(&key.shop_id, &key.shops_product_id)
            .await?;
        if event_records.is_empty() {
            return Ok(None);
        }
        let current_event_count = event_records.len() as u64;
        let events = event_records.into_iter().filter_map(|record| {
            ProductEvent::try_from(record)
                .map_err(|err| error!(error = %err, "Failed mapping ProductEventRecord."))
                .ok()
        });
        let product = match Product::replay(events) {
            Ok(product) => product,
            Err(err) => {
                warn!(error = %err, productKey = %key, "Failed replaying product events.");
                return Ok(None);
            }
        };
        Ok(Some((product, current_event_count)))
    }

    async fn build_created_product(
        &self,
        mut cmd: CreateProductCommand,
    ) -> Option<(Product, ProductDomainEvent)> {
        let resolved = self.enrich_shop_information(&mut cmd).await?;
        self.enrich_price(&mut cmd);
        let event = create_product_event(cmd, resolved);
        let product = match Product::replay(vec![
            event
                .clone()
                .map_payload(ProductEventPayload::ProductDomainEvent),
        ]) {
            Ok(product) => product,
            Err(err) => {
                warn!(error = %err, "Failed replaying created product.");
                return None;
            }
        };
        Some((product, event))
    }

    async fn create_new_product(&self, cmd: CreateProductCommand) -> bool {
        let Some((product, event)) = self.build_created_product(cmd).await else {
            return false;
        };
        let meta = ProductMetaRecord::from_product(&product, 1);
        self.persist_events(
            vec![ProductEventRecord::Domain(ProductDomainEventRecord::from(
                event,
            ))],
            meta,
            0,
        )
        .await
    }

    async fn update_existing_product(
        &self,
        mut product: Product,
        version: u64,
        cmd: UpdateProductCommand,
    ) -> bool {
        let events = determine_update_events(&mut product, cmd, &self.fx_rate);
        if events.is_empty() {
            return true;
        }
        let meta = ProductMetaRecord::from_product(&product, version + events.len() as u64);
        self.persist_events(events, meta, version).await
    }
}

#[async_trait]
impl CommandProductService for CommandProductServiceImpl<'_> {
    async fn create(&self, cmd: CreateProductCommand) -> Option<CreateProductCommand> {
        let key = cmd.key();
        match self.load_product(&key).await {
            Ok(Some(_)) => {
                warn!(shopId = %key.shop_id, shopsProductId = %key.shops_product_id, "Product already exists. Cannot create.");
                None
            }
            Ok(None) => (!self.create_new_product(cmd.clone()).await).then_some(cmd),
            Err(err) => {
                warn!(error = ?err, "Failed loading product before create. Returning command for retry.");
                Some(cmd)
            }
        }
    }

    async fn update(
        &self,
        key: ProductKey,
        cmd: UpdateProductCommand,
    ) -> Option<(ProductKey, UpdateProductCommand)> {
        match self.load_product(&key).await {
            Ok(Some((product, version))) => (!self
                .update_existing_product(product, version, cmd.clone())
                .await)
                .then_some((key, cmd)),
            Ok(None) => {
                warn!(shopId = %key.shop_id, shopsProductId = %key.shops_product_id, "Product not found. Cannot update.");
                Some((key, cmd))
            }
            Err(err) => {
                warn!(error = ?err, "Failed loading product before update. Returning command for retry.");
                Some((key, cmd))
            }
        }
    }

    async fn upsert(&self, cmd: UpsertProductCommand) -> Option<UpsertProductCommand> {
        let key = cmd.key();
        match self.load_product(&key).await {
            Ok(Some((product, version))) => {
                let update_cmd = UpdateProductCommand::from(&cmd);
                (!self
                    .update_existing_product(product, version, update_cmd)
                    .await)
                    .then_some(cmd)
            }
            Ok(None) => {
                let create_cmd = CreateProductCommand::from(cmd.clone());
                (!self.create_new_product(create_cmd).await).then_some(cmd)
            }
            Err(err) => {
                warn!(error = ?err, "Failed loading product before upsert. Returning command for retry.");
                Some(cmd)
            }
        }
    }
}

fn determine_update_events(
    product: &mut Product,
    cmd: UpdateProductCommand,
    fx_rate: &impl FxRate,
) -> Vec<ProductEventRecord> {
    // From<ProductRecord> for Product enriches product.url with UTM params.
    // Strip them here so URL-equality comparisons are done against the
    // canonical (raw) URL, and URL-changed events store the raw URL rather
    // than the already-enriched one.
    product.url = strip_utm_params(product.url.clone());

    let mut events = Vec::new();
    if let Some(price_event) = product.new_price(cmd.native_price, fx_rate) {
        events.push(ProductEventRecord::Domain(ProductDomainEventRecord::from(
            price_event,
        )));
    }
    if let Some(new_state) = cmd.state
        && let Some(state_event) = product.change_state(new_state)
    {
        events.push(ProductEventRecord::Domain(ProductDomainEventRecord::from(
            state_event,
        )));
    }
    if let Some(event) = product.change_estimate_price(
        cmd.native_price_estimate_min,
        cmd.native_price_estimate_max,
        fx_rate,
    ) {
        events.push(ProductEventRecord::Domain(ProductDomainEventRecord::from(
            event,
        )));
    }
    if let Some(url) = cmd.url
        && let Some(event) = product.change_url(url)
    {
        events.push(ProductEventRecord::Domain(ProductDomainEventRecord::from(
            event,
        )));
    }
    if let Some(images) = cmd.images {
        for event in product.change_images(images) {
            events.push(ProductEventRecord::from(event));
        }
    }
    if (cmd.auction_start.is_some() || cmd.auction_end.is_some())
        && let Some(event) = product.change_auction_time(cmd.auction_start, cmd.auction_end)
    {
        events.push(ProductEventRecord::Domain(ProductDomainEventRecord::from(
            event,
        )));
    }

    events
}

fn create_product_event(
    cmd: CreateProductCommand,
    resolved: ResolvedShopInformation,
) -> ProductDomainEvent {
    Product::create(
        cmd.shop_id,
        resolved.seller_id,
        cmd.shops_product_id.clone(),
        resolved.shop_name,
        resolved.seller_name,
        resolved.shop_type,
        cmd.structured_address,
        cmd.geo_address,
        cmd.native_title,
        cmd.native_description,
        cmd.native_price,
        cmd.other_price,
        cmd.native_price_estimate_min,
        cmd.other_price_estimate_min,
        cmd.native_price_estimate_max,
        cmd.other_price_estimate_max,
        cmd.state,
        cmd.url,
        cmd.images,
        cmd.auction_start,
        cmd.auction_end,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use common::price::domain::{FixedFxRate, Price};
    use common::product_state::domain::ProductState;
    use fake::{Fake, Faker};
    use fxrate::service::MockFxRateService;
    use shop::core::shop::Shop;
    use shop::service::get_service::MockGetShopService;
    use shop::service::seller_service::MockSellerService;

    fn update_cmd_for(product: &Product) -> UpdateProductCommand {
        UpdateProductCommand {
            native_price: product.native_price,
            state: Some(product.state),
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
        }
    }

    #[test]
    fn should_determine_no_update_events_when_values_do_not_change() {
        let mut product: Product = Faker.fake();
        let cmd = update_cmd_for(&product);

        let actual = determine_update_events(&mut product, cmd, &FixedFxRate());

        assert!(actual.is_empty());
    }

    #[test]
    fn should_determine_price_changed_event_when_price_changes() {
        let mut product: Product = Faker.fake();
        let mut cmd = update_cmd_for(&product);
        cmd.native_price = Some(Price::new(
            12345_u64.into(),
            common::currency::domain::Currency::Eur,
        ));

        let actual = determine_update_events(&mut product, cmd, &FixedFxRate());

        assert!(actual.iter().any(|event| {
            event.event_type() == ProductDomainEventTypeRecord::DomainPriceChanged.as_str()
        }));
    }

    #[test]
    fn should_determine_state_changed_event_when_state_changes() {
        let mut product: Product = Faker.fake();
        let mut cmd = update_cmd_for(&product);
        cmd.state = Some(if matches!(product.state, ProductState::Available) {
            ProductState::Removed
        } else {
            ProductState::Available
        });

        let actual = determine_update_events(&mut product, cmd, &FixedFxRate());

        assert!(actual.iter().any(|event| {
            event.event_type() == ProductDomainEventTypeRecord::DomainStateChanged.as_str()
        }));
    }

    fn fx_rate_service() -> MockFxRateService {
        let mut fx_rate_service = MockFxRateService::new();
        fx_rate_service
            .expect_get_current()
            .return_once(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
        fx_rate_service
    }

    fn shop_service() -> MockGetShopService {
        let mut shop_service = MockGetShopService::default();
        shop_service.expect_find_shop().returning(|shop_id| {
            let mut shop: Shop = Faker.fake();
            shop.shop_id = *shop_id;
            shop.shop_type = ShopType::AuctionHouse;
            Box::pin(async move { Ok(shop) })
        });
        shop_service
    }

    async fn command_service<'a>(
        repository: &'a (dyn ProductDynamoDbRepository + Sync),
        fx_rate_service: &'a MockFxRateService,
        shop_service: &'a MockGetShopService,
        seller_service: &'a MockSellerService,
    ) -> CommandProductServiceImpl<'a> {
        CommandProductServiceImpl::new(repository, fx_rate_service, shop_service, seller_service)
            .await
            .unwrap()
    }

    fn event_record_from_command(cmd: &CreateProductCommand) -> ProductEventRecord {
        let event = create_product_event(
            cmd.clone(),
            ResolvedShopInformation {
                seller_id: cmd.shop_id,
                seller_name: Faker.fake(),
                shop_name: Faker.fake(),
                shop_type: ShopType::AuctionHouse,
            },
        );
        ProductEventRecord::Domain(ProductDomainEventRecord::from(event))
    }

    #[tokio::test]
    async fn should_create_product_when_product_does_not_exist() {
        let command: CreateProductCommand = Faker.fake();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));
        repository
            .expect_transact_write_product_event_records()
            .return_once(|events, _, expected_version| {
                assert_eq!(1, events.len());
                assert_eq!(0, expected_version);
                Box::pin(async { Ok(()) })
            });
        let fx_rate_service = fx_rate_service();
        let shop_service = shop_service();
        let seller_service = MockSellerService::default();
        let service = command_service(
            &repository,
            &fx_rate_service,
            &shop_service,
            &seller_service,
        )
        .await;

        let actual = service.create(command).await;

        assert!(actual.is_none());
    }

    #[tokio::test]
    async fn should_return_create_command_when_product_create_write_fails() {
        let command: CreateProductCommand = Faker.fake();
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(|_, _| Box::pin(async { Ok(vec![]) }));
        repository
            .expect_transact_write_product_event_records()
            .return_once(|_, _, _| {
                Box::pin(async {
                    Err(aws_sdk_dynamodb::error::SdkError::construction_failure(
                        "boom",
                    ))
                })
            });
        let fx_rate_service = fx_rate_service();
        let shop_service = shop_service();
        let seller_service = MockSellerService::default();
        let service = command_service(
            &repository,
            &fx_rate_service,
            &shop_service,
            &seller_service,
        )
        .await;

        let actual = service.create(command.clone()).await;

        assert_eq!(Some(command), actual);
    }

    #[tokio::test]
    async fn should_update_product_when_existing_product_changes() {
        let create_command: CreateProductCommand = Faker.fake();
        let update_command = UpdateProductCommand {
            native_price: create_command.native_price,
            state: Some(if matches!(create_command.state, ProductState::Available) {
                ProductState::Removed
            } else {
                ProductState::Available
            }),
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
        };
        let key = create_command.key();
        let existing_record = event_record_from_command(&create_command);
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![existing_record]) }));
        repository
            .expect_transact_write_product_event_records()
            .return_once(|events, _, expected_version| {
                assert_eq!(1, events.len());
                assert_eq!(1, expected_version);
                Box::pin(async { Ok(()) })
            });
        let fx_rate_service = fx_rate_service();
        let shop_service = shop_service();
        let seller_service = MockSellerService::default();
        let service = command_service(
            &repository,
            &fx_rate_service,
            &shop_service,
            &seller_service,
        )
        .await;

        let actual = service.update(key, update_command).await;

        assert!(actual.is_none());
    }

    #[tokio::test]
    async fn should_upsert_existing_product_as_update() {
        let create_command: CreateProductCommand = Faker.fake();
        let key = create_command.key();
        let existing_record = event_record_from_command(&create_command);
        let upsert_command = UpsertProductCommand {
            shop_id: key.shop_id,
            shops_product_id: key.shops_product_id.clone(),
            seller_name_raw: create_command.seller_name_raw.clone(),
            structured_address: create_command.structured_address.clone(),
            geo_address: create_command.geo_address,
            native_title: Some(create_command.native_title.clone()),
            native_description: create_command.native_description.clone(),
            native_price: create_command.native_price,
            native_price_estimate_min: create_command.native_price_estimate_min,
            native_price_estimate_max: create_command.native_price_estimate_max,
            state: Some(if matches!(create_command.state, ProductState::Available) {
                ProductState::Removed
            } else {
                ProductState::Available
            }),
            url: Some(create_command.url.clone()),
            images: create_command.images.clone(),
            auction_start: create_command.auction_start,
            auction_end: create_command.auction_end,
        };
        let mut repository = MockProductDynamoDbRepository::default();
        repository
            .expect_query_product_event_records()
            .return_once(move |_, _| Box::pin(async move { Ok(vec![existing_record]) }));
        repository
            .expect_transact_write_product_event_records()
            .return_once(|events, _, expected_version| {
                assert!(!events.is_empty());
                assert_eq!(1, expected_version);
                Box::pin(async { Ok(()) })
            });
        let fx_rate_service = fx_rate_service();
        let shop_service = shop_service();
        let seller_service = MockSellerService::default();
        let service = command_service(
            &repository,
            &fx_rate_service,
            &shop_service,
            &seller_service,
        )
        .await;

        let actual = service.upsert(upsert_command).await;

        assert!(actual.is_none());
    }
}
