use crate::core::product::Product;
use crate::core::product_event::ProductEventLog;
use crate::dynamodb::product_event_record::ProductEventRecord;
use crate::dynamodb::product_event_record::domain::ProductDomainEventRecord;
use crate::dynamodb::product_event_record::enrichment::ProductEnrichmentEventRecord;
use crate::dynamodb::product_event_record::lifecycle::ProductLifecycleEventRecord;
use crate::dynamodb::product_record::ProductRecord;
use crate::dynamodb::product_update_record::ProductRecordUpdate;
use crate::dynamodb::repository::ProductDynamoDbRepository;
use crate::dynamodb::utm::append_utm_params;
use crate::service::product_command::{
    CreateProductCommand, Translation, UpdateProductCommand, UpsertProductCommand,
};
use async_trait::async_trait;
use aws_sdk_dynamodb::config::http::HttpResponse;
use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
use common::batch::Batch;
use common::event_id::EventId;
use common::has_key::HasKey;
use common::logging::{LogEventType, LogWriteSource};
use common::mergeable::Mergeable;
use common::price::domain::{FixedFxRate, FxRate};
use common::product_id::ProductKey;
use common::product_lifecycle::domain::ProductLifecycle;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use shop::core::affiliate_configuration::AffiliateConfiguration;
use shop::core::shop_type::ShopType;
use shop::service::get_service::GetShopService;
use std::collections::{HashMap, hash_map::Entry};
use tracing::warn;

#[async_trait]
#[mockall::automock]
pub trait CommandProductService {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand>;
    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand>;
    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand>;
    async fn delete(&self, product_key: &ProductKey) -> Result<(), DeleteProductCommandError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteProductCommandError {
    #[error("Product not found")]
    NotFound,
    #[error("Failed loading product before delete: {0}")]
    Load(String),
    #[error("Failed product delete transaction: {0}")]
    Persist(String),
}

pub struct CommandProductServiceImpl<'a> {
    dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    fx_rate: FixedFxRate,
    get_shop_service: Option<&'a (dyn GetShopService + Sync)>,
}

struct ResolvedShopInformation {
    seller_id: ShopId,
    seller_name: ShopName,
    shop_name: ShopName,
    shop_type: ShopType,
    affiliate_configuration: Option<AffiliateConfiguration>,
}

impl<'a> CommandProductServiceImpl<'a> {
    pub fn new(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
        get_shop_service: &'a (dyn GetShopService + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate: FixedFxRate(),
            get_shop_service: Some(get_shop_service),
        }
    }

    pub fn new_delete_only(
        dynamodb_repository: &'a (dyn ProductDynamoDbRepository + Sync),
    ) -> Self {
        Self {
            dynamodb_repository,
            fx_rate: FixedFxRate(),
            get_shop_service: None,
        }
    }

    fn fx_rate(&self) -> &FixedFxRate {
        &self.fx_rate
    }

    fn get_shop_service(&self) -> &(dyn GetShopService + Sync) {
        self.get_shop_service
            .expect("shop service must exist for create and upsert")
    }

    fn enrich_price(&self, cmd: &mut CreateProductCommand) {
        if let Some(price) = &cmd.native_price {
            match self
                .fx_rate()
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
                .fx_rate()
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
                .fx_rate()
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
        let shop = match self.get_shop_service().find_shop(&cmd.shop_id).await {
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

        Some(ResolvedShopInformation {
            seller_id: shop.shop_id,
            seller_name: shop.name.clone(),
            shop_name: shop.name,
            shop_type: shop.shop_type,
            affiliate_configuration: shop.affiliate_configuration,
        })
    }

    async fn persist_create(
        &self,
        event_record: ProductEventRecord,
        cmd: CreateProductCommand,
        failures: &mut Vec<CreateProductCommand>,
    ) {
        let domain_record = match &event_record {
            ProductEventRecord::Domain(d) => d.clone(),
            _ => {
                warn!("Unexpected non-Domain event in persist_create — skipping.");
                failures.push(cmd);
                return;
            }
        };
        let product_record = match ProductRecord::try_from(domain_record) {
            Ok(r) => r,
            Err(err) => {
                warn!(error = ?err, "Failed building ProductRecord for create transaction — skipping.");
                failures.push(cmd);
                return;
            }
        };
        let event_log = ProductEventLog::from(&event_record)
            .with_event_type(LogEventType::EntityWrite)
            .with_write_source(LogWriteSource::ProductCommandService)
            .with_msg("Persisted product create transaction.");
        match self
            .dynamodb_repository
            .transact_write_product_create(event_record, product_record)
            .await
        {
            Ok(_) => {
                event_log.log();
            }
            Err(ref err) if is_transaction_conditional_check_failed(err) => {
                // Product was concurrently created by another execution.
                // Retry so that on the next attempt the product already exists and
                // the command is resolved as an update instead of a create.
                warn!(
                    shopId = %cmd.key().shop_id,
                    shopsProductId = %cmd.key().shops_product_id,
                    "Product was already created concurrently — returning command for retry as update."
                );
                failures.push(cmd);
            }
            Err(err) => {
                warn!(error = ?err, "Failed product create transaction — returning command for retry.");
                failures.push(cmd);
            }
        }
    }

    async fn persist_updates_for_product(
        &self,
        event_records: Vec<ProductEventRecord>,
        expected_event_id: EventId,
        product_key: ProductKey,
        cmd: UpdateProductCommand,
        failures: &mut HashMap<ProductKey, UpdateProductCommand>,
    ) {
        if event_records.is_empty() {
            return;
        }
        let combined_update = build_combined_update(&event_records);
        let event_logs: Vec<_> = event_records
            .iter()
            .map(|record| {
                ProductEventLog::from(record)
                    .with_event_type(LogEventType::EntityWrite)
                    .with_write_source(LogWriteSource::ProductCommandService)
                    .with_msg("Persisted product update transaction.")
            })
            .collect();
        match self
            .dynamodb_repository
            .transact_write_product_update(
                event_records,
                combined_update,
                product_key.clone(),
                expected_event_id,
            )
            .await
        {
            Ok(_) => {
                for log in event_logs {
                    log.log();
                }
            }
            Err(err) => {
                warn!(error = ?err,
                    shopId = %product_key.shop_id,
                    shopsProductId = %product_key.shops_product_id,
                    "Failed product update transaction — returning command for retry.");
                failures.insert(product_key, cmd);
            }
        }
    }
}

#[async_trait]
impl CommandProductService for CommandProductServiceImpl<'_> {
    async fn delete(&self, product_key: &ProductKey) -> Result<(), DeleteProductCommandError> {
        let record = self
            .dynamodb_repository
            .get_product_record(&product_key.shop_id, &product_key.shops_product_id)
            .await
            .map_err(|err| DeleteProductCommandError::Load(format!("{err:?}")))?
            .ok_or(DeleteProductCommandError::NotFound)?;

        if ProductLifecycle::from(record.lifecycle) == ProductLifecycle::Deleted {
            return Ok(());
        }

        let expected_event_id = record.event_id;
        let mut product = Product::from(record);
        let Some(event) = product.delete() else {
            return Ok(());
        };
        let event_record = ProductEventRecord::Lifecycle(ProductLifecycleEventRecord::from(event));
        let update = match &event_record {
            ProductEventRecord::Lifecycle(lifecycle) => {
                ProductRecordUpdate::from(lifecycle.clone())
            }
            _ => ProductRecordUpdate::default(),
        };
        let event_log = ProductEventLog::from(&event_record)
            .with_event_type(LogEventType::EntityWrite)
            .with_write_source(LogWriteSource::ProductCommandService)
            .with_msg("Persisted product delete transaction.");

        self.dynamodb_repository
            .transact_write_product_update(
                vec![event_record],
                update,
                product.key(),
                expected_event_id,
            )
            .await
            .map_err(|err| {
                warn!(error = ?err, "Failed product delete transaction.");
                DeleteProductCommandError::Persist(format!("{err:?}"))
            })?;
        event_log.log();
        Ok(())
    }

    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand> {
        let mut failures = Vec::new();

        for chunk in Batch::<CreateProductCommand, 100>::chunked_from(cmds.into_iter()) {
            let mut key_cmds: HashMap<ProductKey, Vec<CreateProductCommand>> = HashMap::new();
            for cmd in chunk {
                key_cmds.entry(cmd.key()).or_default().push(cmd);
            }
            let mut working: HashMap<ProductKey, CreateProductCommand> = key_cmds
                .iter()
                .map(|(key, commands)| {
                    (
                        key.clone(),
                        commands
                            .last()
                            .cloned()
                            .expect("grouped create commands must be non-empty"),
                    )
                })
                .collect();
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
                            working.remove(&key);
                            if let Some(cmds) = key_cmds.remove(&key) {
                                failures.extend(cmds);
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

                    for mut cmd in working.into_values() {
                        if let Some(resolved) = self.enrich_shop_information(&mut cmd).await {
                            self.enrich_price(&mut cmd);
                            let seller_id = resolved.seller_id;
                            let cmd_clone = cmd.clone();
                            let view_url = resolved
                                .affiliate_configuration
                                .as_ref()
                                .map(|a| a.build_url(&cmd.url))
                                .unwrap_or_else(|| append_utm_params(cmd.url.clone()));
                            let domain_event = Product::create(
                                cmd.shop_id,
                                seller_id,
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
                                view_url.clone(),
                                cmd.images,
                                cmd.auction_start,
                                cmd.auction_end,
                            );
                            let event_record = ProductEventRecord::Domain(
                                ProductDomainEventRecord::from(domain_event),
                            );
                            let mut create_failures = Vec::new();
                            self.persist_create(event_record, cmd_clone, &mut create_failures)
                                .await;
                            for failed_create in create_failures {
                                if let Some(cmds) = key_cmds.remove(&failed_create.key()) {
                                    failures.extend(cmds);
                                } else {
                                    failures.push(failed_create);
                                }
                            }
                        } else {
                            if let Some(cmds) = key_cmds.remove(&cmd.key()) {
                                failures.extend(cmds);
                            } else {
                                failures.push(cmd);
                            }
                        }
                    }
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product batch before create. Returning commands for retry.");
                    failures.extend(key_cmds.into_values().flatten());
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
            let mut key_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();
            for (key, cmd) in chunk {
                match key_cmds.entry(key) {
                    Entry::Occupied(mut entry) => entry.get_mut().merge(cmd),
                    Entry::Vacant(entry) => {
                        entry.insert(cmd);
                    }
                }
            }
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

                    let key_to_old_event_id: HashMap<ProductKey, EventId> = records
                        .items
                        .iter()
                        .map(|r| (r.key(), r.event_id))
                        .collect();

                    let events =
                        determine_update_events(&mut working, records.items, self.fx_rate());

                    // Remaining items in `working` are products not found in DynamoDB —
                    // `determine_update_events` removes matched keys.
                    for (key, cmd) in &working {
                        warn!(
                            shopId = %key.shop_id,
                            shopsProductId = %key.shops_product_id,
                            "Product not found. Cannot update."
                        );
                        failures.insert(key.clone(), cmd.clone());
                    }

                    let mut events_by_key: HashMap<ProductKey, Vec<ProductEventRecord>> =
                        HashMap::new();
                    for event in events {
                        let key = event.key().clone();
                        events_by_key.entry(key).or_default().push(event);
                    }

                    for (key, event_records) in events_by_key {
                        if let (Some(expected_event_id), Some(cmd)) = (
                            key_to_old_event_id.get(&key).copied(),
                            key_cmds.remove(&key),
                        ) {
                            self.persist_updates_for_product(
                                event_records,
                                expected_event_id,
                                key,
                                cmd,
                                &mut failures,
                            )
                            .await;
                        }
                    }
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
            let mut key_cmds: HashMap<ProductKey, UpsertProductCommand> = HashMap::new();
            for cmd in chunk {
                match key_cmds.entry(cmd.key()) {
                    Entry::Occupied(mut entry) => entry.get_mut().merge(cmd),
                    Entry::Vacant(entry) => {
                        entry.insert(cmd);
                    }
                }
            }
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

                    let key_to_old_event_id: HashMap<ProductKey, EventId> = records
                        .items
                        .iter()
                        .map(|r| (r.key(), r.event_id))
                        .collect();

                    // Build update commands for existing products
                    let mut update_cmds: HashMap<ProductKey, UpdateProductCommand> = HashMap::new();
                    for record in &records.items {
                        let key = record.key();
                        if let Some(cmd) = working.remove(&key) {
                            update_cmds.insert(key, UpdateProductCommand::from(&cmd));
                        }
                    }

                    // Determine update events for existing products
                    let update_events =
                        determine_update_events(&mut update_cmds, records.items, self.fx_rate());

                    let mut update_events_by_key: HashMap<ProductKey, Vec<ProductEventRecord>> =
                        HashMap::new();
                    for event in update_events {
                        let key = event.key().clone();
                        update_events_by_key.entry(key).or_default().push(event);
                    }

                    for (key, event_records) in update_events_by_key {
                        if let Some(expected_event_id) = key_to_old_event_id.get(&key).copied() {
                            let update_cmd = key_cmds
                                .get(&key)
                                .map(UpdateProductCommand::from)
                                .expect("cmd must exist for key that had events");
                            let mut update_failures: HashMap<ProductKey, UpdateProductCommand> =
                                HashMap::new();
                            self.persist_updates_for_product(
                                event_records,
                                expected_event_id,
                                key.clone(),
                                update_cmd,
                                &mut update_failures,
                            )
                            .await;
                            for failed_key in update_failures.into_keys() {
                                if let Some(upsert_cmd) = key_cmds.remove(&failed_key) {
                                    failures.push(upsert_cmd);
                                }
                            }
                        }
                    }

                    for cmd in working.into_values() {
                        let mut create_cmd = CreateProductCommand::from(cmd.clone());
                        if let Some(resolved) = self.enrich_shop_information(&mut create_cmd).await
                        {
                            self.enrich_price(&mut create_cmd);
                            let seller_id = resolved.seller_id;
                            let create_cmd_clone = create_cmd.clone();
                            let view_url = resolved
                                .affiliate_configuration
                                .as_ref()
                                .map(|a| a.build_url(&create_cmd.url))
                                .unwrap_or_else(|| append_utm_params(create_cmd.url.clone()));
                            let domain_event = Product::create(
                                create_cmd.shop_id,
                                seller_id,
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
                                view_url.clone(),
                                create_cmd.images,
                                create_cmd.auction_start,
                                create_cmd.auction_end,
                            );
                            let event_record = ProductEventRecord::Domain(
                                ProductDomainEventRecord::from(domain_event),
                            );
                            let mut create_failures: Vec<CreateProductCommand> = Vec::new();
                            self.persist_create(
                                event_record,
                                create_cmd_clone,
                                &mut create_failures,
                            )
                            .await;
                            for failed_create in create_failures {
                                if let Some(upsert_cmd) = key_cmds.remove(&failed_create.key()) {
                                    failures.push(upsert_cmd);
                                }
                            }
                        } else {
                            key_cmds.remove(&cmd.key());
                            failures.push(cmd);
                        }
                    }
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

fn is_transaction_conditional_check_failed(
    err: &SdkError<TransactWriteItemsError, HttpResponse>,
) -> bool {
    if let SdkError::ServiceError(se) = err
        && let TransactWriteItemsError::TransactionCanceledException(e) = se.err()
    {
        return e
            .cancellation_reasons()
            .iter()
            .any(|r| r.code() == Some("ConditionalCheckFailed"));
    }
    false
}

fn build_combined_update(event_records: &[ProductEventRecord]) -> ProductRecordUpdate {
    let mut combined = ProductRecordUpdate {
        event_id: None,
        ..ProductRecordUpdate::default()
    };
    for record in event_records {
        match record {
            ProductEventRecord::Lifecycle(lifecycle) => {
                let upd = ProductRecordUpdate::from(lifecycle.clone());
                combined.updated = upd.updated;
                combined.event_id = upd.event_id.or(combined.event_id);
                combined.lifecycle = upd.lifecycle.or(combined.lifecycle);
            }
            ProductEventRecord::Domain(domain) => {
                let upd = ProductRecordUpdate::from(domain.clone());
                combined.updated = upd.updated;
                combined.event_id = upd.event_id.or(combined.event_id);
                combined.price_native = upd.price_native.or(combined.price_native);
                combined.price_eur = upd.price_eur.or(combined.price_eur);
                combined.price_usd = upd.price_usd.or(combined.price_usd);
                combined.price_gbp = upd.price_gbp.or(combined.price_gbp);
                combined.price_aud = upd.price_aud.or(combined.price_aud);
                combined.price_cad = upd.price_cad.or(combined.price_cad);
                combined.price_nzd = upd.price_nzd.or(combined.price_nzd);
                combined.price_cny = upd.price_cny.or(combined.price_cny);
                combined.price_brl = upd.price_brl.or(combined.price_brl);
                combined.price_pln = upd.price_pln.or(combined.price_pln);
                combined.price_try = upd.price_try.or(combined.price_try);
                combined.price_jpy = upd.price_jpy.or(combined.price_jpy);
                combined.price_czk = upd.price_czk.or(combined.price_czk);
                combined.price_rub = upd.price_rub.or(combined.price_rub);
                combined.price_aed = upd.price_aed.or(combined.price_aed);
                combined.price_sar = upd.price_sar.or(combined.price_sar);
                combined.price_hkd = upd.price_hkd.or(combined.price_hkd);
                combined.price_sgd = upd.price_sgd.or(combined.price_sgd);
                combined.price_chf = upd.price_chf.or(combined.price_chf);
                combined.state = upd.state.or(combined.state);
                combined.lifecycle = upd.lifecycle.or(combined.lifecycle);
                combined.title_de = upd.title_de.or(combined.title_de);
                combined.title_en = upd.title_en.or(combined.title_en);
                combined.title_fr = upd.title_fr.or(combined.title_fr);
                combined.title_es = upd.title_es.or(combined.title_es);
                combined.title_it = upd.title_it.or(combined.title_it);
                combined.images = upd.images.or(combined.images);
                combined.price_estimate_min_native = upd
                    .price_estimate_min_native
                    .or(combined.price_estimate_min_native);
                combined.price_estimate_min_eur = upd
                    .price_estimate_min_eur
                    .or(combined.price_estimate_min_eur);
                combined.price_estimate_min_usd = upd
                    .price_estimate_min_usd
                    .or(combined.price_estimate_min_usd);
                combined.price_estimate_min_gbp = upd
                    .price_estimate_min_gbp
                    .or(combined.price_estimate_min_gbp);
                combined.price_estimate_min_aud = upd
                    .price_estimate_min_aud
                    .or(combined.price_estimate_min_aud);
                combined.price_estimate_min_cad = upd
                    .price_estimate_min_cad
                    .or(combined.price_estimate_min_cad);
                combined.price_estimate_min_nzd = upd
                    .price_estimate_min_nzd
                    .or(combined.price_estimate_min_nzd);
                combined.price_estimate_min_cny = upd
                    .price_estimate_min_cny
                    .or(combined.price_estimate_min_cny);
                combined.price_estimate_min_brl = upd
                    .price_estimate_min_brl
                    .or(combined.price_estimate_min_brl);
                combined.price_estimate_min_pln = upd
                    .price_estimate_min_pln
                    .or(combined.price_estimate_min_pln);
                combined.price_estimate_min_try = upd
                    .price_estimate_min_try
                    .or(combined.price_estimate_min_try);
                combined.price_estimate_min_jpy = upd
                    .price_estimate_min_jpy
                    .or(combined.price_estimate_min_jpy);
                combined.price_estimate_min_czk = upd
                    .price_estimate_min_czk
                    .or(combined.price_estimate_min_czk);
                combined.price_estimate_min_rub = upd
                    .price_estimate_min_rub
                    .or(combined.price_estimate_min_rub);
                combined.price_estimate_min_aed = upd
                    .price_estimate_min_aed
                    .or(combined.price_estimate_min_aed);
                combined.price_estimate_min_sar = upd
                    .price_estimate_min_sar
                    .or(combined.price_estimate_min_sar);
                combined.price_estimate_min_hkd = upd
                    .price_estimate_min_hkd
                    .or(combined.price_estimate_min_hkd);
                combined.price_estimate_min_sgd = upd
                    .price_estimate_min_sgd
                    .or(combined.price_estimate_min_sgd);
                combined.price_estimate_min_chf = upd
                    .price_estimate_min_chf
                    .or(combined.price_estimate_min_chf);
                combined.price_estimate_max_native = upd
                    .price_estimate_max_native
                    .or(combined.price_estimate_max_native);
                combined.price_estimate_max_eur = upd
                    .price_estimate_max_eur
                    .or(combined.price_estimate_max_eur);
                combined.price_estimate_max_usd = upd
                    .price_estimate_max_usd
                    .or(combined.price_estimate_max_usd);
                combined.price_estimate_max_gbp = upd
                    .price_estimate_max_gbp
                    .or(combined.price_estimate_max_gbp);
                combined.price_estimate_max_aud = upd
                    .price_estimate_max_aud
                    .or(combined.price_estimate_max_aud);
                combined.price_estimate_max_cad = upd
                    .price_estimate_max_cad
                    .or(combined.price_estimate_max_cad);
                combined.price_estimate_max_nzd = upd
                    .price_estimate_max_nzd
                    .or(combined.price_estimate_max_nzd);
                combined.price_estimate_max_cny = upd
                    .price_estimate_max_cny
                    .or(combined.price_estimate_max_cny);
                combined.price_estimate_max_brl = upd
                    .price_estimate_max_brl
                    .or(combined.price_estimate_max_brl);
                combined.price_estimate_max_pln = upd
                    .price_estimate_max_pln
                    .or(combined.price_estimate_max_pln);
                combined.price_estimate_max_try = upd
                    .price_estimate_max_try
                    .or(combined.price_estimate_max_try);
                combined.price_estimate_max_jpy = upd
                    .price_estimate_max_jpy
                    .or(combined.price_estimate_max_jpy);
                combined.price_estimate_max_czk = upd
                    .price_estimate_max_czk
                    .or(combined.price_estimate_max_czk);
                combined.price_estimate_max_rub = upd
                    .price_estimate_max_rub
                    .or(combined.price_estimate_max_rub);
                combined.price_estimate_max_aed = upd
                    .price_estimate_max_aed
                    .or(combined.price_estimate_max_aed);
                combined.price_estimate_max_sar = upd
                    .price_estimate_max_sar
                    .or(combined.price_estimate_max_sar);
                combined.price_estimate_max_hkd = upd
                    .price_estimate_max_hkd
                    .or(combined.price_estimate_max_hkd);
                combined.price_estimate_max_sgd = upd
                    .price_estimate_max_sgd
                    .or(combined.price_estimate_max_sgd);
                combined.price_estimate_max_chf = upd
                    .price_estimate_max_chf
                    .or(combined.price_estimate_max_chf);
                combined.url = upd.url.or(combined.url);
                combined.view_url = upd.view_url.or(combined.view_url);
                combined.auction_start = upd.auction_start.or(combined.auction_start);
                combined.auction_end = upd.auction_end.or(combined.auction_end);
                combined.embedding = upd.embedding.or(combined.embedding);
            }
            ProductEventRecord::Enrichment(enrichment) => {
                let upd = ProductRecordUpdate::from(enrichment.clone());
                combined.updated = upd.updated;
                combined.event_id = upd.event_id.or(combined.event_id);
                combined.title_de = upd.title_de.or(combined.title_de);
                combined.title_en = upd.title_en.or(combined.title_en);
                combined.title_fr = upd.title_fr.or(combined.title_fr);
                combined.title_es = upd.title_es.or(combined.title_es);
                combined.title_it = upd.title_it.or(combined.title_it);
                combined.embedding = upd.embedding.or(combined.embedding);
            }
            ProductEventRecord::Policy(_) => {}
        }
    }
    combined
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
            if let Some(new_native_price) = cmd.native_price
                && let Some(price_event) = product.change_price(new_native_price, fx_rate)
            {
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
                && let Some(event) = product.change_url(url.clone(), append_utm_params(url))
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
            if let Some(embedding) = cmd.embedding
                && let Some(event) = product.embed(embedding)
            {
                events.push(ProductEventRecord::Enrichment(
                    ProductEnrichmentEventRecord::from(event),
                ));
            }
            if let Some(Translation { source, targets }) = cmd.translated_titles {
                let source_language = source.localization;
                for (target_language, title) in targets {
                    if let Some(event) =
                        product.translate_title(source_language, target_language, title)
                    {
                        events.push(ProductEventRecord::Enrichment(
                            ProductEnrichmentEventRecord::from(event),
                        ));
                    }
                }
            }
        }
    }

    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::product::Product;
    use crate::dynamodb::repository::MockProductDynamoDbRepository;
    use crate::service::product_command::{CreateProductCommand, UpdateProductCommand};
    use aws_sdk_dynamodb::error::SdkError;
    use common::has_key::HasKey;
    use common::{price::domain::FixedFxRate, product_state::domain::ProductState};
    use fake::{Fake, Faker};
    use rstest;
    use shop::core::shop::Shop;
    use shop::core::shop_type::ShopType;
    use shop::service::get_service::MockGetShopService;

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

    async fn make_command_product_service<'a>(
        repository: &'a (dyn ProductDynamoDbRepository + Sync),
    ) -> CommandProductServiceImpl<'a> {
        let get_shop_service = Box::leak(Box::new(default_shop_service()));
        CommandProductServiceImpl::new(repository, get_shop_service)
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
            product.images = Default::default();
            let new_images = vec![ProductImage {
                url: url::Url::parse("https://img.example.com/new.jpg").unwrap(),
                prohibited_content: ProhibitedContent::None,
            }]
            .into_iter()
            .collect();
            let cmd = UpdateProductCommand {
                native_price: product.native_price,
                state: Some(product.state),
                native_price_estimate_min: None,
                native_price_estimate_max: None,
                url: None,
                images: Some(new_images),
                auction_start: None,
                auction_end: None,
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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
                embedding: None,
                translated_titles: None,
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

        async fn service_for_shop_information<'a>(
            repository: &'a (dyn ProductDynamoDbRepository + Sync),
            get_shop_service: &'a (dyn GetShopService + Sync),
        ) -> CommandProductServiceImpl<'a> {
            CommandProductServiceImpl::new(repository, get_shop_service)
        }

        #[tokio::test]
        async fn should_ignore_raw_seller_name_when_enriching_shop_information() {
            let mut cmd = Faker.fake::<CreateProductCommand>();
            cmd.seller_name_raw = Some("Raw Seller".to_string());

            let mut shop: Shop = Faker.fake();
            shop.shop_id = cmd.shop_id;
            shop.shop_type = ShopType::Marketplace;
            let expected_seller_id = shop.shop_id;
            let expected_seller_name = shop.name.clone();

            let mut get_shop_service = MockGetShopService::default();
            get_shop_service
                .expect_find_shop()
                .return_once(move |_| Box::pin(async move { Ok(shop) }));

            let repository = MockProductDynamoDbRepository::default();
            let service = service_for_shop_information(&repository, &get_shop_service).await;

            let resolved = service
                .enrich_shop_information(&mut cmd)
                .await
                .expect("shop information should resolve");

            assert_eq!(resolved.seller_id, expected_seller_id);
            assert_eq!(resolved.seller_name, expected_seller_name);
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
            let service = service_for_shop_information(&repository, &get_shop_service).await;

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
            let service = service_for_shop_information(&repository, &get_shop_service).await;

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
                .expect_transact_write_product_create()
                .returning(|_, _| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(|_, _| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(|_, _| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
            }]
            .into_iter()
            .collect();
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                .expect_transact_write_product_create()
                .returning(move |event_record, _product_record| {
                    captured_clone.lock().unwrap().push(event_record);
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                embedding: None,
                translated_titles: None,
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
                .expect_transact_write_product_update()
                .returning(|_, _, _, _| {
                    Box::pin(async {
                        Ok(aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsOutput::builder().build())
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
                embedding: None,
                translated_titles: None,
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
            // transact_write_product_update should NOT be called since there are no events
            repository.expect_transact_write_product_update().never();

            let service = make_command_product_service(&repository).await;
            let failures = service.update(HashMap::from([(key, cmd)])).await;

            assert!(failures.is_empty());
        }
    }

    mod concurrent_create {
        use super::*;
        use aws_sdk_dynamodb::operation::transact_write_items::TransactWriteItemsError;
        use aws_sdk_dynamodb::types::CancellationReason;
        use aws_sdk_dynamodb::types::error::TransactionCanceledException;
        use common::batch::dynamodb::BatchGetItemResult;

        fn conditional_check_failed_err()
        -> SdkError<TransactWriteItemsError, aws_sdk_dynamodb::config::http::HttpResponse> {
            let reason = CancellationReason::builder()
                .code("ConditionalCheckFailed")
                .build();
            let inner = TransactionCanceledException::builder()
                .cancellation_reasons(reason)
                .build();
            SdkError::service_error(
                TransactWriteItemsError::TransactionCanceledException(inner),
                aws_sdk_dynamodb::config::http::HttpResponse::new(
                    200u16.try_into().unwrap(),
                    "{}".into(),
                ),
            )
        }

        /// Verifies the core invariant from the issue:
        ///
        /// When two simultaneous upsert commands arrive for the same not-yet-existing product:
        /// 1. Both lambdas read an empty table (batch-get finds nothing → resolve to create).
        /// 2. The first lambda wins the transact_write_product_create and succeeds.
        /// 3. The second lambda's transact_write_product_create fails with ConditionalCheckFailed.
        ///
        /// The second command MUST be returned as a failure so that SQS retries it.
        /// On retry the product already exists, so the command is resolved as an update.
        /// Without this retry the state-change update would be silently dropped.
        #[tokio::test]
        async fn should_return_create_command_as_failure_when_conditional_check_fails() {
            let mut repository = MockProductDynamoDbRepository::default();

            // batch-get returns empty — both lambdas see an empty table simultaneously.
            repository.expect_get_product_records().return_once(|_| {
                Box::pin(async {
                    Ok(BatchGetItemResult {
                        items: vec![],
                        unprocessed: None,
                    })
                })
            });

            // Concurrent create succeeded first → this lambda gets ConditionalCheckFailed.
            repository
                .expect_transact_write_product_create()
                .return_once(|_, _| Box::pin(async { Err(conditional_check_failed_err()) }));

            let service = make_command_product_service(&repository).await;
            let cmd: CreateProductCommand = Faker.fake();
            let failures = service.create(vec![cmd.clone()]).await;

            assert_eq!(
                1,
                failures.len(),
                "command must be returned as failure so SQS retries it as an update"
            );
            assert_eq!(cmd.key(), failures[0].key());
        }
    }
}
