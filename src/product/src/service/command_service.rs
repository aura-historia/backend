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
use std::collections::{HashMap, HashSet};
use tracing::{error, warn};

#[async_trait]
#[mockall::automock]
pub trait CommandProductService {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand>;
    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand>;
    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand>;
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

    async fn persist_events<C>(
        &self,
        events: Vec<ProductEventRecord>,
        meta_record: ProductMetaRecord,
        expected_event_version: u64,
        key_cmds: &mut HashMap<ProductKey, C>,
    ) -> Option<(ProductKey, C)> {
        let product_key = meta_record.key();
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
                None
            }
            Err(err) => {
                warn!(error = ?err, "Failed writing product event transaction. Returning command for retry.");
                key_cmds.remove(&product_key).map(|cmd| (product_key, cmd))
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
}

#[async_trait]
impl CommandProductService for CommandProductServiceImpl<'_> {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand> {
        let mut failures = Vec::new();
        let mut seen = HashSet::new();

        for mut cmd in cmds {
            let key = cmd.key();
            if !seen.insert(key.clone()) {
                warn!(shopId = %key.shop_id, shopsProductId = %key.shops_product_id, "Duplicate create command in batch.");
                failures.push(cmd);
                continue;
            }
            match self.load_product(&key).await {
                Ok(Some(_)) => {
                    warn!(shopId = %key.shop_id, shopsProductId = %key.shops_product_id, "Product already exists. Cannot create.");
                }
                Ok(None) => {
                    let Some(resolved) = self.enrich_shop_information(&mut cmd).await else {
                        failures.push(cmd);
                        continue;
                    };
                    self.enrich_price(&mut cmd);
                    let event = create_product_event(cmd.clone(), resolved);
                    let product = match Product::replay(vec![
                        event
                            .clone()
                            .map_payload(ProductEventPayload::ProductDomainEvent),
                    ]) {
                        Ok(product) => product,
                        Err(err) => {
                            warn!(error = %err, "Failed replaying created product.");
                            failures.push(cmd);
                            continue;
                        }
                    };
                    let events = vec![ProductEventRecord::Domain(ProductDomainEventRecord::from(
                        event,
                    ))];
                    let meta = ProductMetaRecord::from_product(&product, 1);
                    let mut key_cmds = HashMap::from([(key.clone(), cmd)]);
                    if let Some((_, cmd)) =
                        self.persist_events(events, meta, 0, &mut key_cmds).await
                    {
                        failures.push(cmd);
                    }
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product before create. Returning command for retry.");
                    failures.push(cmd);
                }
            }
        }

        failures
    }

    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand> {
        let mut failures = HashMap::new();

        for (key, cmd) in cmds {
            match self.load_product(&key).await {
                Ok(Some((product, version))) => {
                    let mut working = HashMap::from([(key.clone(), cmd.clone())]);
                    let events =
                        determine_update_events(&mut working, vec![product.clone()], &self.fx_rate);
                    if events.is_empty() {
                        continue;
                    }
                    let mut updated = product;
                    let mut failed_apply = false;
                    for event in events
                        .iter()
                        .cloned()
                        .filter_map(|record| ProductEvent::try_from(record).ok())
                    {
                        if let Err(err) = updated.apply_event(event) {
                            warn!(error = %err, "Failed applying product update event.");
                            failures.insert(key.clone(), cmd.clone());
                            failed_apply = true;
                            break;
                        }
                    }
                    if failed_apply {
                        continue;
                    }
                    let meta =
                        ProductMetaRecord::from_product(&updated, version + events.len() as u64);
                    let mut key_cmds = HashMap::from([(key.clone(), cmd.clone())]);
                    if let Some((key, cmd)) = self
                        .persist_events(events, meta, version, &mut key_cmds)
                        .await
                    {
                        failures.insert(key, cmd);
                    }
                }
                Ok(None) => {
                    warn!(shopId = %key.shop_id, shopsProductId = %key.shops_product_id, "Product not found. Cannot update.");
                    failures.insert(key, cmd);
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product before update. Returning command for retry.");
                    failures.insert(key, cmd);
                }
            }
        }

        failures
    }

    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        let mut failures = Vec::new();

        let mut seen = HashSet::new();
        for cmd in cmds {
            let key = cmd.key();
            if !seen.insert(key.clone()) {
                warn!(shopId = %key.shop_id, shopsProductId = %key.shops_product_id, "Duplicate upsert command in batch.");
                failures.push(cmd);
                continue;
            }
            match self.load_product(&key).await {
                Ok(Some((product, version))) => {
                    let update_cmd = UpdateProductCommand::from(&cmd);
                    let mut working = HashMap::from([(key.clone(), update_cmd)]);
                    let events =
                        determine_update_events(&mut working, vec![product.clone()], &self.fx_rate);
                    if events.is_empty() {
                        continue;
                    }
                    let mut updated = product;
                    let mut failed_apply = false;
                    for event in events
                        .iter()
                        .cloned()
                        .filter_map(|record| ProductEvent::try_from(record).ok())
                    {
                        if let Err(err) = updated.apply_event(event) {
                            warn!(error = %err, "Failed applying product upsert event.");
                            failures.push(cmd.clone());
                            failed_apply = true;
                            break;
                        }
                    }
                    if failed_apply {
                        continue;
                    }
                    let meta =
                        ProductMetaRecord::from_product(&updated, version + events.len() as u64);
                    let mut key_cmds = HashMap::from([(key.clone(), cmd.clone())]);
                    if let Some((_, cmd)) = self
                        .persist_events(events, meta, version, &mut key_cmds)
                        .await
                    {
                        failures.push(cmd);
                    }
                }
                Ok(None) => {
                    let mut create_cmd = CreateProductCommand::from(cmd.clone());
                    let Some(resolved) = self.enrich_shop_information(&mut create_cmd).await else {
                        failures.push(cmd);
                        continue;
                    };
                    self.enrich_price(&mut create_cmd);
                    let event = create_product_event(create_cmd, resolved);
                    let product = match Product::replay(vec![
                        event
                            .clone()
                            .map_payload(ProductEventPayload::ProductDomainEvent),
                    ]) {
                        Ok(product) => product,
                        Err(err) => {
                            warn!(error = %err, "Failed replaying upsert-created product.");
                            failures.push(cmd);
                            continue;
                        }
                    };
                    let events = vec![ProductEventRecord::Domain(ProductDomainEventRecord::from(
                        event,
                    ))];
                    let meta = ProductMetaRecord::from_product(&product, 1);
                    let mut key_cmds = HashMap::from([(key.clone(), cmd)]);
                    if let Some((_, cmd)) =
                        self.persist_events(events, meta, 0, &mut key_cmds).await
                    {
                        failures.push(cmd);
                    }
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product before upsert. Returning command for retry.");
                    failures.push(cmd);
                }
            }
        }

        failures
    }
}

fn determine_update_events(
    working: &mut HashMap<ProductKey, UpdateProductCommand>,
    records: Vec<impl HasKey<Key = ProductKey> + Into<Product>>,
    fx_rate: &impl FxRate,
) -> Vec<ProductEventRecord> {
    let mut events = Vec::new();

    for record in records {
        let key = record.key();
        if let Some(cmd) = working.remove(&key) {
            let mut product: Product = record.into();
            // From<ProductRecord> for Product enriches product.url with UTM params.
            // Strip them here so URL-equality comparisons are done against the
            // canonical (raw) URL, and URL-changed events store the raw URL rather
            // than the already-enriched one.
            product.url = strip_utm_params(product.url.clone());
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
        }
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

#[cfg(any())]
mod tests {
    use super::*;
    use crate::core::product::Product;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use crate::service::product_command::{CreateProductCommand, UpdateProductCommand};
    use aws_sdk_dynamodb::error::SdkError;
    use common::has_key::HasKey;
    use common::{price::domain::FixedFxRate, product_state::domain::ProductState};
    use fake::{Fake, Faker};
    use fxrate::dynamodb::record::FxRatesRecord;
    use fxrate::service::MockFxRateService;
    use rstest;
    use shop::core::shop::Shop;
    use shop::core::shop_type::ShopType;
    use shop::service::get_service::MockGetShopService;
    use shop::service::seller_service::MockSellerService;

    fn default_shop_service() -> MockGetShopService {
        let mut service = MockGetShopService::default();
        service.expect_find_shop().returning(|shop_id| {
            let mut shop: Shop = Faker.fake();
            shop.shop_id = *shop_id;
            shop.shop_type = ShopType::AuctionHouse;
            Box::pin(async move { Ok(shop) })
        });
        service
    }

    fn default_seller_service() -> MockSellerService {
        MockSellerService::default()
    }

    fn default_fx_rate_service() -> MockFxRateService {
        let mut service = MockFxRateService::new();
        service
            .expect_get_current()
            .returning(|| Box::pin(async { Ok(FxRatesRecord::from(FixedFxRate())) }));
        service
    }

    async fn make_command_product_service<'a>(
        repository: &'a (dyn ProductDynamoDbRepository + Sync),
    ) -> CommandProductServiceImpl<'a> {
        let get_shop_service = Box::leak(Box::new(default_shop_service()));
        let seller_service = Box::leak(Box::new(default_seller_service()));
        let fx_rate_service = default_fx_rate_service();
        CommandProductServiceImpl::new(
            repository,
            &fx_rate_service,
            get_shop_service,
            seller_service,
        )
        .await
        .expect("failed to create CommandProductServiceImpl in test")
    }

    mod determine_update_events {
        use super::*;
        use crate::dynamodb::product_event_type_record::domain::ProductDomainEventTypeRecord;
        use crate::dynamodb::product_record::ProductRecord;
        use common::price::domain::Price;

        #[test]
        fn should_determine_no_update_events_when_only_skipped() {
            let record1 = Faker.fake::<ProductRecord>();
            let product1 = Product::from(record1.clone());
            let cmd1 = UpdateProductCommand {
                native_price: product1.native_price,
                state: Some(product1.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let record2 = Faker.fake::<ProductRecord>();
            let product2 = Product::from(record2.clone());
            let cmd2 = UpdateProductCommand {
                native_price: product2.native_price,
                state: Some(product2.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let mut working = HashMap::from([(product1.key(), cmd1), (product2.key(), cmd2)]);

            let actual =
                determine_update_events(&mut working, vec![record1, record2], &FixedFxRate());
            assert!(working.is_empty());
            assert!(actual.is_empty());
        }

        #[test]
        fn should_determine_update_events_when_none_skipped() {
            let record1 = Faker.fake::<ProductRecord>();
            let product1 = Product::from(record1.clone());
            let cmd1 = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(product1.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let record2 = Faker.fake::<ProductRecord>();
            let product2 = Product::from(record2.clone());
            let cmd2 = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(if matches!(product2.state, ProductState::Available) {
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

            let mut working = HashMap::from([(product1.key(), cmd1), (product2.key(), cmd2)]);

            let actual =
                determine_update_events(&mut working, vec![record1, record2], &FixedFxRate());
            assert!(working.is_empty());
            assert_eq!(3, actual.len());
        }

        #[test]
        fn should_determine_update_events_when_some_skipped() {
            let record1 = Faker.fake::<ProductRecord>();
            let product1 = Product::from(record1.clone());
            let cmd1 = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(product1.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let record2 = Faker.fake::<ProductRecord>();
            let product2 = Product::from(record2.clone());
            let cmd2 = UpdateProductCommand {
                native_price: product2.native_price,
                state: Some(product2.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let mut working = HashMap::from([(product1.key(), cmd1), (product2.key(), cmd2)]);

            let actual =
                determine_update_events(&mut working, vec![record1, record2], &FixedFxRate());
            assert!(working.is_empty());
            assert_eq!(1, actual.len());
        }

        #[test]
        fn should_leave_unmatched_keys_in_working() {
            let product = Faker.fake::<Product>();
            let cmd = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(ProductState::Available),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let mut working = HashMap::from([(product.key(), cmd.clone())]);

            let actual =
                determine_update_events(&mut working, Vec::<ProductRecord>::new(), &FixedFxRate());
            assert!(actual.is_empty());
            assert_eq!(1, working.len());
            assert_eq!(Some(&cmd), working.get(&product.key()));
        }

        #[test]
        fn should_generate_estimate_price_changed_event_when_estimate_price_changes() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.native_price_estimate_min = None;
            product.native_price_estimate_max = None;
            let new_min = Some(Price::new(
                100u64.into(),
                common::currency::domain::Currency::Eur,
            ));
            let new_max = Some(Price::new(
                500u64.into(),
                common::currency::domain::Currency::Eur,
            ));
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: new_min,
                native_price_estimate_max: new_max,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            let expected = ProductDomainEventTypeRecord::DomainEstimatePriceChanged.as_str();
            assert!(events.iter().any(|e| e.event_type() == expected));
        }

        #[test]
        fn should_generate_url_changed_event_when_url_changes() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.url = url::Url::parse("https://original.example.com").unwrap();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: Some(url::Url::parse("https://definitely-different.example.com").unwrap()),
                images: None,
                auction_start: None,
                auction_end: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            let expected = ProductDomainEventTypeRecord::DomainUrlChanged.as_str();
            assert!(events.iter().any(|e| e.event_type() == expected));
        }

        #[test]
        fn should_generate_images_changed_event_when_images_change() {
            use crate::core::product_image::ProductImage;
            use crate::core::prohibited_content::ProhibitedContent;

            let mut product: Product = Faker.fake();
            let key = product.key();
            product.images = vec![];
            let new_images = vec![ProductImage {
                url: url::Url::parse("https://img.example.com/new.jpg").unwrap(),
                prohibited_content: ProhibitedContent::None,
            }];
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: Some(new_images),
                auction_start: None,
                auction_end: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            let expected = ProductDomainEventTypeRecord::DomainImagesChanged.as_str();
            assert!(events.iter().any(|e| e.event_type() == expected));
        }

        #[test]
        fn should_generate_auction_time_changed_event_when_auction_time_changes() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.auction_start = None;
            product.auction_end = None;
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: Some(time::OffsetDateTime::now_utc() + time::Duration::days(30)),
                auction_end: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            let expected = ProductDomainEventTypeRecord::DomainAuctionTimeChanged.as_str();
            assert!(events.iter().any(|e| e.event_type() == expected));
        }

        #[test]
        fn should_generate_no_events_when_no_fields_change() {
            let product: Product = Faker.fake();
            let key = product.key();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(events.is_empty());
        }

        #[test]
        fn should_generate_multiple_events_when_multiple_fields_change() {
            let mut product: Product = Faker.fake();
            let key = product.key();
            product.url = url::Url::parse("https://original.example.com").unwrap();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: Some(url::Url::parse("https://different.example.com").unwrap()),
                images: None,
                auction_start: Some(time::OffsetDateTime::now_utc() + time::Duration::days(30)),
                auction_end: None,
            };
            let mut working = HashMap::from([(key.clone(), cmd)]);
            let events = determine_update_events(&mut working, vec![product], &FixedFxRate());
            assert!(events.len() >= 2);
            let url_changed = ProductDomainEventTypeRecord::DomainUrlChanged.as_str();
            let auction_changed = ProductDomainEventTypeRecord::DomainAuctionTimeChanged.as_str();
            assert!(events.iter().any(|e| e.event_type() == url_changed));
            assert!(events.iter().any(|e| e.event_type() == auction_changed));
        }
    }

    mod create {
        use super::*;
        use crate::dynamodb::product_record::ProductRecord;
        use common::batch::dynamodb::BatchGetItemResult;
        use common::slug_id::SlugId;

        async fn service_for_shop_information<'a>(
            repository: &'a (dyn ProductDynamoDbRepository + Sync),
            get_shop_service: &'a (dyn GetShopService + Sync),
            seller_service: &'a (dyn SellerService + Sync),
        ) -> CommandProductServiceImpl<'a> {
            let fx_rate_service = default_fx_rate_service();
            CommandProductServiceImpl::new(
                repository,
                &fx_rate_service,
                get_shop_service,
                seller_service,
            )
            .await
            .expect("failed to create CommandProductServiceImpl in test")
        }

        #[tokio::test]
        async fn should_resolve_seller_from_raw_name_when_platform_for_shop_information() {
            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.seller_name_raw = Some("Raw Seller".to_string());

            let mut shop: Shop = Faker.fake();
            shop.shop_id = cmd.shop_id;
            shop.shop_type = ShopType::Marketplace;

            let resolved_seller_id = ShopId::new();
            let resolved_seller_name = ShopName::from("Resolved Seller");
            let mut get_shop_service = MockGetShopService::default();
            get_shop_service
                .expect_find_shop()
                .return_once(move |_| Box::pin(async move { Ok(shop) }));
            let mut seller_service = MockSellerService::default();
            let expected_seller_name = resolved_seller_name.clone();
            seller_service
                .expect_get_seller_shop_details()
                .return_once(move |raw_name| {
                    assert_eq!(raw_name.as_ref(), "Raw Seller");
                    Box::pin(async move {
                        Ok((
                            resolved_seller_id,
                            SlugId::from("resolved-seller"),
                            expected_seller_name,
                        ))
                    })
                });

            let repository = MockProductDynamoDbRepository::default();
            let service =
                service_for_shop_information(&repository, &get_shop_service, &seller_service).await;

            let resolved = service
                .enrich_shop_information(&mut cmd)
                .await
                .expect("shop information should resolve");

            assert_eq!(resolved.seller_id, resolved_seller_id);
            assert_eq!(resolved.seller_name, resolved_seller_name);
        }

        #[tokio::test]
        async fn should_use_shop_addresses_when_product_addresses_missing_for_shop_information() {
            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.seller_name_raw = None;
            cmd.structured_address = None;
            cmd.geo_address = None;

            let mut shop: Shop = Faker.fake();
            shop.shop_id = cmd.shop_id;
            shop.shop_type = ShopType::AuctionHouse;
            shop.structured_address = Some(Faker.fake());
            shop.geo_address = Some(Faker.fake());
            let expected_structured_address = shop.structured_address.clone();
            let expected_geo_address = shop.geo_address;

            let mut get_shop_service = MockGetShopService::default();
            get_shop_service
                .expect_find_shop()
                .return_once(move |_| Box::pin(async move { Ok(shop) }));

            let repository = MockProductDynamoDbRepository::default();
            let seller_service = MockSellerService::default();
            let service =
                service_for_shop_information(&repository, &get_shop_service, &seller_service).await;

            service
                .enrich_shop_information(&mut cmd)
                .await
                .expect("shop information should resolve");

            assert_eq!(cmd.structured_address, expected_structured_address);
            assert_eq!(cmd.geo_address, expected_geo_address);
        }

        #[tokio::test]
        async fn should_keep_product_addresses_when_either_product_address_exists_for_shop_information()
         {
            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.seller_name_raw = None;
            cmd.structured_address = Some(Faker.fake());
            cmd.geo_address = None;
            let expected_structured_address = cmd.structured_address.clone();

            let mut shop: Shop = Faker.fake();
            shop.shop_id = cmd.shop_id;
            shop.shop_type = ShopType::AuctionHouse;
            shop.structured_address = Some(Faker.fake());
            shop.geo_address = Some(Faker.fake());

            let mut get_shop_service = MockGetShopService::default();
            get_shop_service
                .expect_find_shop()
                .return_once(move |_| Box::pin(async move { Ok(shop) }));

            let repository = MockProductDynamoDbRepository::default();
            let seller_service = MockSellerService::default();
            let service =
                service_for_shop_information(&repository, &get_shop_service, &seller_service).await;

            service
                .enrich_shop_information(&mut cmd)
                .await
                .expect("shop information should resolve");

            assert_eq!(cmd.structured_address, expected_structured_address);
            assert_eq!(cmd.geo_address, None);
        }

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(aws_sdk_dynamodb::error::ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            aws_sdk_dynamodb::config::http::HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
            aws_sdk_dynamodb::config::http::HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_fail_entire_chunk_when_batch_get_entirely_fails(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_records()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let service = make_command_product_service(&repository).await;

            let mut expected = fake::vec![CreateProductCommand; 89];
            let mut actual = service.create(expected.clone()).await;

            expected.sort_by_key(|l| l.key());
            actual.sort_by_key(|l| l.key());

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_create_products_when_none_exist() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let cmds = fake::vec![CreateProductCommand; 5];
            let failures = service.create(cmds).await;

            assert!(failures.is_empty());
        }

        #[tokio::test]
        async fn should_skip_products_that_already_exist() {
            let existing_record = Faker.fake::<ProductRecord>();
            let existing_key = existing_record.key();

            let mut existing_cmd = Faker.fake::<CreateProductCommand>();
            existing_cmd.shop_id = existing_key.shop_id;
            existing_cmd.shops_product_id = existing_key.shops_product_id;

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let record = existing_record.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![record],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;

            let mut cmds = fake::vec![CreateProductCommand; 3];
            cmds.push(existing_cmd);
            let failures = service.create(cmds).await;

            assert!(failures.is_empty());
        }

        #[tokio::test]
        async fn should_return_unprocessed_as_failures() {
            let cmds = fake::vec![CreateProductCommand; 3];
            let unprocessed_keys: Vec<ProductKey> = cmds.iter().map(|c| c.key()).collect();

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let keys = unprocessed_keys.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: Some(keys.try_into().unwrap()),
                    })
                })
            });

            let service = make_command_product_service(&repository).await;

            let mut expected = cmds.clone();
            let mut actual = service.create(cmds).await;

            expected.sort_by_key(|l| l.key());
            actual.sort_by_key(|l| l.key());

            assert_eq!(expected, actual);
        }

        #[tokio::test]
        async fn should_enrich_prices_for_created_products() {
            use common::currency::domain::Currency;
            use strum::EnumCount;

            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.native_price = Some(Faker.fake());
            cmd.other_price.clear();
            cmd.native_price_estimate_min = Some(Faker.fake());
            cmd.other_price_estimate_min.clear();
            cmd.native_price_estimate_max = Some(Faker.fake());
            cmd.other_price_estimate_max.clear();

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;

            // Verify enrich_price directly
            let mut test_cmd = cmd.clone();
            service.enrich_price(&mut test_cmd);
            assert_eq!(Currency::COUNT, test_cmd.other_price.len());
            assert_eq!(Currency::COUNT, test_cmd.other_price_estimate_min.len());
            assert_eq!(Currency::COUNT, test_cmd.other_price_estimate_max.len());

            let failures = service.create(vec![cmd]).await;
            assert!(failures.is_empty());
        }

        /// Helper: build a `CreateProductCommand` with a given title and no description.
        fn cmd_with_title(title: &str) -> CreateProductCommand {
            use crate::core::product_image::ProductImage;
            use crate::core::title::Title;
            use common::language::domain::Language;
            use common::localized::Localized;

            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.native_title = Localized::new(Language::De, Title::from(title));
            cmd.native_description = None;
            cmd.images = vec![ProductImage {
                url: url::Url::parse("https://img.example.com/item.jpg").unwrap(),
                prohibited_content: crate::core::prohibited_content::ProhibitedContent::Unknown,
            }];
            cmd
        }

        fn empty_items_repository() -> MockProductDynamoDbRepository {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });
            repository
        }

        #[tokio::test]
        async fn should_create_domain_event_with_nazi_classification_when_title_contains_drittes_reich()
         {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Orden aus dem Dritten Reich 1940");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            // With the new design, classification is embedded in the domain event;
            // no separate policy event is created for creates.
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event should be created for create operations"
            );
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain create event to be created"
            );
        }

        #[tokio::test]
        async fn should_create_domain_event_when_title_contains_nsdap() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("NSDAP Abzeichen 1935 Original");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain create event to be created for NSDAP listing"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event for create – classification is embedded in the domain event"
            );
        }

        #[tokio::test]
        async fn should_create_domain_event_when_title_contains_waffen_ss() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Waffen-SS Feldmütze WWII Original");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain create event to be created for Waffen-SS listing"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event for create – classification is embedded in the domain event"
            );
        }

        #[tokio::test]
        async fn should_create_domain_event_when_english_nazi_germany_title() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Badge from Nazi Germany WWII collection");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain create event for 'Nazi Germany' title"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event for create – classification is embedded in images"
            );
        }

        #[tokio::test]
        async fn should_create_domain_event_for_hitlerjugend_listing() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Hitlerjugend Messer HJ M1937 mit Scheide");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain create event for Hitlerjugend listing"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event for create – classification is embedded in images"
            );
        }

        #[tokio::test]
        async fn should_create_domain_event_for_swastika_listing() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Bronze pendant with Swastika symbol WWII original");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain create event for Swastika listing"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event for create – classification is embedded in images"
            );
        }

        #[tokio::test]
        async fn should_create_only_domain_event_for_nazi_listing() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Hakenkreuz Wanddeko Porzellan 1938");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            // Classification is embedded in the Created domain event's images;
            // no separate policy event is created for creates.
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Domain(_))),
                "expected a domain event"
            );
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "no policy event should be created for create – classification is in the domain event"
            );
        }

        #[tokio::test]
        async fn should_not_create_policy_event_when_title_is_benign() {
            use std::sync::{Arc, Mutex};

            let cmd = cmd_with_title("Biedermeier Sekretär 19. Jahrhundert Mahagoni");

            let captured: Arc<Mutex<Vec<ProductEventRecord>>> = Arc::new(Mutex::new(Vec::new()));
            let captured_clone = captured.clone();

            let mut repository = empty_items_repository();
            repository
                .expect_put_product_event_records()
                .returning(move |batch| {
                    captured_clone.lock().unwrap().extend(batch.into_iter());
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.create(vec![cmd]).await;

            assert!(failures.is_empty());
            let events = captured.lock().unwrap();
            assert!(
                !events
                    .iter()
                    .any(|e| matches!(e, ProductEventRecord::Policy(_))),
                "expected no policy event for a benign antique furniture listing"
            );
        }
    }

    mod update {
        use super::*;
        use crate::dynamodb::product_record::ProductRecord;
        use common::batch::dynamodb::BatchGetItemResult;

        #[tokio::test]
        #[rstest::rstest]
        #[case::construction_failure(SdkError::construction_failure("Something went wrong"))]
        #[case::timeout(SdkError::timeout_error("Something went wrong"))]
        #[case::dispatch_failure(SdkError::dispatch_failure(aws_sdk_dynamodb::error::ConnectorError::user("Something went wrong".into())))]
        #[case::response_error(SdkError::response_error(
            "Something went wrong",
            aws_sdk_dynamodb::config::http::HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[case::service_error(SdkError::service_error(
            aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError::unhandled("Something went wrong"),
            aws_sdk_dynamodb::config::http::HttpResponse::new(500u16.try_into().unwrap(), "{}".into())
        ))]
        #[trace]
        async fn should_fail_entire_chunk_when_batch_get_entirely_fails(
            #[case] expected: SdkError<
                aws_sdk_dynamodb::operation::batch_get_item::BatchGetItemError,
                aws_sdk_dynamodb::config::http::HttpResponse,
            >,
        ) {
            let mut repository = MockProductDynamoDbRepository::default();
            repository
                .expect_get_product_records()
                .return_once(|_| Box::pin(async { Err(expected) }));

            let service = make_command_product_service(&repository).await;

            let cmds: HashMap<ProductKey, UpdateProductCommand> = (0..5)
                .map(|_| {
                    let product = Faker.fake::<Product>();
                    (product.key(), Faker.fake())
                })
                .collect();
            let expected_keys: Vec<ProductKey> = cmds.keys().cloned().collect();
            let actual = service.update(cmds).await;

            let mut actual_keys: Vec<ProductKey> = actual.keys().cloned().collect();
            let mut expected_sorted = expected_keys;
            expected_sorted.sort();
            actual_keys.sort();

            assert_eq!(expected_sorted, actual_keys);
        }

        #[tokio::test]
        async fn should_update_products_when_all_exist() {
            let record = Faker.fake::<ProductRecord>();
            let product = Product::from(record.clone());
            let key = product.key();
            let cmd = UpdateProductCommand {
                native_price: Some(Faker.fake()),
                state: Some(if matches!(product.state, ProductState::Available) {
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

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let r = record.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![r],
                        unprocessed: None,
                    })
                })
            });
            repository
                .expect_put_product_event_records()
                .returning(|_| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::batch_write_item::BatchWriteItemOutput::builder().build())
                    })
                });

            let service = make_command_product_service(&repository).await;
            let failures = service.update(HashMap::from([(key, cmd)])).await;

            assert!(failures.is_empty());
        }

        #[tokio::test]
        async fn should_return_not_found_products_as_failures() {
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });

            let service = make_command_product_service(&repository).await;

            let cmds: HashMap<ProductKey, UpdateProductCommand> = (0..3)
                .map(|_| {
                    let product = Faker.fake::<Product>();
                    (product.key(), Faker.fake())
                })
                .collect();
            let expected_keys: Vec<ProductKey> = cmds.keys().cloned().collect();
            let actual = service.update(cmds).await;

            let mut actual_keys: Vec<ProductKey> = actual.keys().cloned().collect();
            let mut expected_sorted = expected_keys;
            expected_sorted.sort();
            actual_keys.sort();

            assert_eq!(expected_sorted, actual_keys);
        }

        #[tokio::test]
        async fn should_return_unprocessed_as_failures() {
            let product = Faker.fake::<Product>();
            let key = product.key();
            let cmd: UpdateProductCommand = Faker.fake();

            let unprocessed_key = key.clone();
            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let k = unprocessed_key.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: Some(vec![k].try_into().unwrap()),
                    })
                })
            });

            let service = make_command_product_service(&repository).await;

            let actual = service
                .update(HashMap::from([(key.clone(), cmd.clone())]))
                .await;

            assert_eq!(1, actual.len());
            assert_eq!(Some(&cmd), actual.get(&key));
        }

        #[tokio::test]
        async fn should_skip_updates_when_no_changes() {
            let record = Faker.fake::<ProductRecord>();
            let product = Product::from(record.clone());
            let key = product.key();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: None,
                auction_start: None,
                auction_end: None,
            };

            let mut repository = MockProductDynamoDbRepository::default();
            repository.expect_get_product_records().returning(move |_| {
                let r = record.clone();
                Box::pin(async move {
                    Ok(BatchGetItemResult {
                        items: vec![r],
                        unprocessed: None,
                    })
                })
            });
            // put_product_event_records should NOT be called since there are no events
            repository.expect_put_product_event_records().never();

            let service = make_command_product_service(&repository).await;
            let failures = service.update(HashMap::from([(key, cmd)])).await;

            assert!(failures.is_empty());
        }
    }
}
