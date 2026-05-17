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
