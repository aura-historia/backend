use crate::core::product::Product;
use crate::core::product_event::ProductEventLog;
use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::repository::{ProductDynamoDbRepository, extract_product_key};
use crate::dynamodb::utm::strip_utm_params;
use crate::service::product_command::{
    CreateProductCommand, UpdateProductCommand, UpsertProductCommand,
};
use async_trait::async_trait;
use common::batch::Batch;
use common::has_key::HasKey;
use common::logging::{LogEventType, LogWriteSource};
use common::price::domain::FxRate;
use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
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

pub struct CommandProductServiceImpl<'a, T: FxRate + Sync> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    fx_rate: &'a T,
    get_shop_service: &'a (dyn GetShopService + Sync),
    seller_service: &'a (dyn SellerService + Sync),
}

struct ResolvedShopInformation {
    seller_id: ShopId,
    seller_name: ShopName,
    shop_name: ShopName,
    shop_type: ShopType,
}

impl<'a, T: FxRate + Sync> CommandProductServiceImpl<'a, T> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        fx_rate: &'a T,
        get_shop_service: &'a (dyn GetShopService + Sync),
        seller_service: &'a (dyn SellerService + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate,
            get_shop_service,
            seller_service,
        }
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
        key_cmds: &mut HashMap<ProductKey, C>,
    ) -> Vec<(ProductKey, C)> {
        let mut failures = Vec::new();
        for batch in Batch::<_, 25>::chunked_from(events.into_iter()) {
            let event_logs = batch
                .iter()
                .map(|record| {
                    ProductEventLog::from(record)
                        .with_event_type(LogEventType::EntityWrite)
                        .with_write_source(LogWriteSource::ProductCommandService)
                        .with_msg("Persisted product event.")
                })
                .collect::<Vec<_>>();
            let product_keys = batch.iter().map(|event| event.key()).collect::<Vec<_>>();
            let res = self
                .dynamodb_repository
                .put_product_event_records(batch)
                .await;
            match res {
                Ok(output) => {
                    let failed_product_keys = output
                        .unprocessed_items
                        .unwrap_or_default()
                        .into_values()
                        .flatten()
                        .map(|req| req.put_request.expect("shouldn't be any other request than 'PutRequest' because events are append-only").item)
                        .map(extract_product_key)
                        .filter_map(|result| match result {
                            Ok(key) => Some(key),
                            Err(err) => {
                                error!(error = %err, "Failed extracting ProductKey.");
                                None
                            }
                        });
                    let failed_product_keys = failed_product_keys.collect::<HashSet<_>>();
                    for failed_product_key in &failed_product_keys {
                        if let Some(cmd) = key_cmds.remove(failed_product_key) {
                            failures.push((failed_product_key.clone(), cmd));
                        }
                    }
                    for event_log in event_logs {
                        if !failed_product_keys.contains(&event_log.key()) {
                            event_log.log();
                        }
                    }
                }
                Err(err) => {
                    warn!(error = ?err, "Failed writing product event batch. Returning commands for retry.");
                    for product_key in product_keys {
                        if let Some(cmd) = key_cmds.remove(&product_key) {
                            failures.push((product_key, cmd));
                        }
                    }
                }
            }
        }
        failures
    }
}

#[async_trait]
impl<T: FxRate + Sync> CommandProductService for CommandProductServiceImpl<'_, T> {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand> {
        let mut failures = Vec::new();

        for chunk in Batch::<CreateProductCommand, 100>::chunked_from(cmds.into_iter()) {
            let mut key_cmds: HashMap<ProductKey, CreateProductCommand> =
                chunk.into_iter().map(|cmd| (cmd.key(), cmd)).collect();
            let mut working = key_cmds.clone();
            let keys: Batch<ProductKey, 100> = working
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .try_into()
                .expect("shouldn't fail because keys come from a Batch<_, 100>");

            match self.dynamodb_repository.get_product_records(&keys).await {
                Ok(records) => {
                    if let Some(unprocessed) = records.unprocessed {
                        for key in unprocessed {
                            if let Some(cmd) = working.remove(&key) {
                                failures.push(cmd);
                            }
                        }
                    }

                    for record in records.items {
                        let key = record.key();
                        if working.remove(&key).is_some() {
                            warn!(
                                shopId = %key.shop_id,
                                shopsProductId = %key.shops_product_id,
                                "Product already exists. Cannot create."
                            );
                        }
                    }

                    let mut events: Vec<ProductEventRecord> = Vec::with_capacity(working.len());
                    for mut cmd in working.into_values() {
                        if let Some(resolved) = self.enrich_shop_information(&mut cmd).await {
                            self.enrich_price(&mut cmd);
                            events.push(ProductEventRecord::Domain(
                                ProductDomainEventRecord::from(Product::create(
                                    cmd.shop_id,
                                    resolved.seller_id,
                                    cmd.shops_product_id,
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
                                )),
                            ));
                        } else {
                            key_cmds.remove(&cmd.key());
                            failures.push(cmd);
                        }
                    }

                    let persist_failures = self.persist_events(events, &mut key_cmds).await;
                    failures.extend(persist_failures.into_iter().map(|(_, cmd)| cmd));
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product batch before create. Returning commands for retry.");
                    failures.extend(working.into_values());
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

        for chunk in
            Batch::<(ProductKey, UpdateProductCommand), 100>::chunked_from(cmds.into_iter())
        {
            let mut key_cmds: HashMap<ProductKey, UpdateProductCommand> =
                chunk.into_iter().collect();
            let mut working = key_cmds.clone();
            let keys: Batch<ProductKey, 100> = working
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .try_into()
                .expect("shouldn't fail because keys come from a Batch<_, 100>");

            match self.dynamodb_repository.get_product_records(&keys).await {
                Ok(records) => {
                    if let Some(unprocessed) = records.unprocessed {
                        for key in unprocessed {
                            if let Some(cmd) = working.remove(&key) {
                                failures.insert(key, cmd);
                            }
                        }
                    }

                    let events = determine_update_events(&mut working, records.items, self.fx_rate);
                    let events: Vec<ProductEventRecord> =
                        events.into_iter().map(ProductEventRecord::from).collect();

                    for (key, cmd) in &working {
                        warn!(
                            shopId = %key.shop_id,
                            shopsProductId = %key.shops_product_id,
                            "Product not found. Cannot update."
                        );
                        failures.insert(key.clone(), cmd.clone());
                    }

                    let persist_failures = self.persist_events(events, &mut key_cmds).await;
                    failures.extend(persist_failures);
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product batch before update. Returning commands for retry.");
                    failures.extend(working);
                }
            }
        }

        failures
    }

    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        let mut failures = Vec::new();

        for chunk in Batch::<UpsertProductCommand, 100>::chunked_from(cmds.into_iter()) {
            let mut key_cmds: HashMap<ProductKey, UpsertProductCommand> =
                chunk.into_iter().map(|cmd| (cmd.key(), cmd)).collect();
            let mut working = key_cmds.clone();
            let keys: Batch<ProductKey, 100> = working
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .try_into()
                .expect("shouldn't fail because keys come from a Batch<_, 100>");

            match self.dynamodb_repository.get_product_records(&keys).await {
                Ok(records) => {
                    if let Some(unprocessed) = records.unprocessed {
                        for key in unprocessed {
                            if let Some(cmd) = working.remove(&key) {
                                failures.push(cmd);
                            }
                        }
                    }

                    let mut update_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();
                    for record in &records.items {
                        let key = record.key();
                        if let Some(cmd) = working.remove(&key) {
                            update_cmds.insert(key, UpdateProductCommand::from(&cmd));
                        }
                    }

                    let update_events =
                        determine_update_events(&mut update_cmds, records.items, self.fx_rate);

                    let mut create_events: Vec<ProductEventRecord> = Vec::with_capacity(working.len());
                    for cmd in working.into_values() {
                        let mut create_cmd = CreateProductCommand::from(cmd.clone());
                        if let Some(resolved) = self.enrich_shop_information(&mut create_cmd).await {
                            self.enrich_price(&mut create_cmd);
                            create_events.push(ProductEventRecord::Domain(
                                ProductDomainEventRecord::from(Product::create(
                                    create_cmd.shop_id,
                                    resolved.seller_id,
                                    create_cmd.shops_product_id,
                                    resolved.shop_name,
                                    resolved.seller_name,
                                    resolved.shop_type,
                                    create_cmd.structured_address,
                                    create_cmd.geo_address,
                                    create_cmd.native_title,
                                    create_cmd.native_description,
                                    create_cmd.native_price,
                                    create_cmd.other_price,
                                    create_cmd.native_price_estimate_min,
                                    create_cmd.other_price_estimate_min,
                                    create_cmd.native_price_estimate_max,
                                    create_cmd.other_price_estimate_max,
                                    create_cmd.state,
                                    create_cmd.url,
                                    create_cmd.images,
                                    create_cmd.auction_start,
                                    create_cmd.auction_end,
                                )),
                            ));
                        } else {
                            key_cmds.remove(&cmd.key());
                            failures.push(cmd);
                        }
                    }

                    let all_events: Vec<ProductEventRecord> = update_events
                        .into_iter()
                        .map(ProductEventRecord::from)
                        .chain(create_events)
                        .collect();

                    let persist_failures = self.persist_events(all_events, &mut key_cmds).await;
                    failures.extend(persist_failures.into_iter().map(|(_, cmd)| cmd));
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product batch before upsert. Returning commands for retry.");
                    failures.extend(working.into_values());
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
) -> Vec<ProductDomainEventRecord> {
    let mut events = Vec::new();

    for record in records {
        let key = record.key();
        if let Some(cmd) = working.remove(&key) {
            let mut product: Product = record.into();
            product.url = strip_utm_params(product.url.clone());
            if let Some(price_event) = product.new_price(cmd.native_price, fx_rate) {
                events.push(ProductDomainEventRecord::from(price_event));
            }
            if let Some(new_state) = cmd.state
                && let Some(state_event) = product.change_state(new_state)
            {
                events.push(ProductDomainEventRecord::from(state_event));
            }
            if let Some(event) = product.change_estimate_price(
                cmd.native_price_estimate_min,
                cmd.native_price_estimate_max,
                fx_rate,
            ) {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(url) = cmd.url
                && let Some(event) = product.change_url(url)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if let Some(images) = cmd.images
                && let Some(event) = product.change_images(images)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
            if (cmd.auction_start.is_some() || cmd.auction_end.is_some())
                && let Some(event) = product.change_auction_time(cmd.auction_start, cmd.auction_end)
            {
                events.push(ProductDomainEventRecord::from(event));
            }
        }
    }

    events
}
