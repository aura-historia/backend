use crate::core::product::Product;
use crate::core::product_event::{
    ProductDomainEvent, ProductEnrichmentEvent, ProductEvent, ProductEventPayload,
    ProductLifecycleEvent, ProductPolicyEvent,
};
use crate::postgres::{ProductPostgresRepository, ProductPostgresRepositoryError};
use crate::service::product_command::{
    CreateProductCommand, Translation, UpdateProductCommand, UpsertProductCommand,
};
use async_trait::async_trait;
use common::has_key::HasKey;
use common::mergeable::Mergeable;
use common::price::domain::FxRate;
use common::product_id::ProductKey;
use common::shop_id::ShopId;
use common::shop_name::ShopName;
use common::utm::append_utm_params;
use fxrate::dynamodb::record::FxRatesRecord;
use fxrate::service::{FxRateService, FxRateServiceError};
use shop::core::affiliate_configuration::AffiliateConfiguration;
use shop::core::shop_type::ShopType;
use shop::service::get_service::GetShopService;
use std::collections::HashMap;
use tracing::warn;

#[async_trait]
pub trait ProductService {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand>;
    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand>;
    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand>;
    async fn delete(&self, product_key: &ProductKey) -> Result<(), DeleteProductError>;
}

#[derive(Debug, thiserror::Error)]
pub enum DeleteProductError {
    #[error("Product not found")]
    NotFound,
    #[error("Failed loading product before delete: {0}")]
    Load(String),
    #[error("Failed product delete transaction: {0}")]
    Persist(String),
}

pub struct ProductServiceImpl<'a> {
    postgres_repository: &'a ProductPostgresRepository,
    fx_rate: FxRatesRecord,
    get_shop_service: &'a (dyn GetShopService + Sync),
}

struct ResolvedShopInformation {
    seller_id: ShopId,
    seller_name: ShopName,
    shop_name: ShopName,
    shop_type: ShopType,
    affiliate_configuration: Option<AffiliateConfiguration>,
}

impl<'a> ProductServiceImpl<'a> {
    pub async fn new(
        postgres_repository: &'a ProductPostgresRepository,
        fx_rate_service: &(dyn FxRateService + Sync),
        get_shop_service: &'a (dyn GetShopService + Sync),
    ) -> Result<Self, FxRateServiceError> {
        let fx_rate = fx_rate_service.get_current().await?;
        Ok(Self {
            postgres_repository,
            fx_rate,
            get_shop_service,
        })
    }

    pub fn new_with_fx_rate(
        postgres_repository: &'a ProductPostgresRepository,
        fx_rate: FxRatesRecord,
        get_shop_service: &'a (dyn GetShopService + Sync),
    ) -> Self {
        Self {
            postgres_repository,
            fx_rate,
            get_shop_service,
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

        Some(ResolvedShopInformation {
            seller_id: shop.shop_id,
            seller_name: shop.name.clone(),
            shop_name: shop.name,
            shop_type: shop.shop_type,
            affiliate_configuration: shop.affiliate_configuration,
        })
    }

    async fn create_one(&self, mut cmd: CreateProductCommand) -> Result<(), CreateProductCommand> {
        match self.postgres_repository.get_product(&cmd.key()).await {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {}
            Err(err) => {
                warn!(error = ?err, "Failed loading product before create.");
                return Err(cmd);
            }
        }

        let Some(resolved) = self.enrich_shop_information(&mut cmd).await else {
            return Err(cmd);
        };
        self.enrich_price(&mut cmd);
        let view_url = resolved
            .affiliate_configuration
            .as_ref()
            .map(|configuration| configuration.build_url(&cmd.url))
            .unwrap_or_else(|| append_utm_params(cmd.url.clone()));
        let failure_cmd = cmd.clone();
        let create_event = Product::create(
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
            view_url,
            cmd.images,
            cmd.auction_start,
            cmd.auction_end,
        );

        match self
            .postgres_repository
            .insert_created_product(create_event)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(error = ?err, "Failed product create transaction.");
                Err(failure_cmd)
            }
        }
    }

    async fn update_one(
        &self,
        key: ProductKey,
        cmd: UpdateProductCommand,
    ) -> Result<(), (ProductKey, UpdateProductCommand)> {
        let mut product = match self.postgres_repository.get_product(&key).await {
            Ok(Some(product)) => product,
            Ok(None) => return Err((key, cmd)),
            Err(err) => {
                warn!(error = ?err, "Failed loading product before update.");
                return Err((key, cmd));
            }
        };
        let expected_event_id = product.event_id;
        let events = determine_update_events(&mut product, cmd.clone(), &self.fx_rate);
        if events.is_empty() {
            return Ok(());
        }
        match self
            .postgres_repository
            .update_product_with_events(product, events, expected_event_id)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) => {
                warn!(error = ?err, "Failed product update transaction.");
                Err((key, cmd))
            }
        }
    }
}

#[async_trait]
impl ProductService for ProductServiceImpl<'_> {
    async fn create(&self, cmds: Vec<CreateProductCommand>) -> Vec<CreateProductCommand> {
        let mut by_key = HashMap::<ProductKey, CreateProductCommand>::new();
        for cmd in cmds {
            by_key.insert(cmd.key(), cmd);
        }

        let mut failures = Vec::new();
        for cmd in by_key.into_values() {
            if let Err(cmd) = self.create_one(cmd).await {
                failures.push(cmd);
            }
        }
        failures
    }

    async fn update(
        &self,
        cmds: HashMap<ProductKey, UpdateProductCommand>,
    ) -> HashMap<ProductKey, UpdateProductCommand> {
        let mut merged = HashMap::<ProductKey, UpdateProductCommand>::new();
        for (key, cmd) in cmds {
            match merged.entry(key) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().merge(cmd)
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(cmd);
                }
            }
        }

        let mut failures = HashMap::new();
        for (key, cmd) in merged {
            if let Err((key, cmd)) = self.update_one(key, cmd).await {
                failures.insert(key, cmd);
            }
        }
        failures
    }

    async fn upsert(&self, cmds: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        let mut merged = HashMap::<ProductKey, UpsertProductCommand>::new();
        for cmd in cmds {
            match merged.entry(cmd.key()) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    entry.get_mut().merge(cmd)
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(cmd);
                }
            }
        }

        let mut failures = Vec::new();
        for (key, cmd) in merged {
            match self.postgres_repository.get_product(&key).await {
                Ok(Some(_)) => {
                    if self
                        .update_one(key, UpdateProductCommand::from(&cmd))
                        .await
                        .is_err()
                    {
                        failures.push(cmd);
                    }
                }
                Ok(None) => {
                    if self
                        .create_one(CreateProductCommand::from(cmd.clone()))
                        .await
                        .is_err()
                    {
                        failures.push(cmd);
                    }
                }
                Err(err) => {
                    warn!(error = ?err, "Failed loading product before upsert.");
                    failures.push(cmd);
                }
            }
        }
        failures
    }

    async fn delete(&self, product_key: &ProductKey) -> Result<(), DeleteProductError> {
        let mut product = self
            .postgres_repository
            .get_product(product_key)
            .await
            .map_err(|err| DeleteProductError::Load(err.to_string()))?
            .ok_or(DeleteProductError::NotFound)?;
        let expected_event_id = product.event_id;
        let Some(event) = product.delete() else {
            return Ok(());
        };
        self.postgres_repository
            .update_product_with_events(product, vec![lifecycle_event(event)], expected_event_id)
            .await
            .map_err(|err| DeleteProductError::Persist(err.to_string()))
    }
}

fn determine_update_events(
    product: &mut Product,
    cmd: UpdateProductCommand,
    fx_rate: &impl FxRate,
) -> Vec<ProductEvent> {
    let mut events = Vec::new();
    if let Some(new_native_price) = cmd.native_price
        && let Some(event) = product.change_price(new_native_price, fx_rate)
    {
        events.push(domain_event(event));
    }
    if let Some(new_state) = cmd.state
        && let Some(event) = product.change_state(new_state)
    {
        events.push(domain_event(event));
    }
    if let Some(event) = product.change_estimate_price(
        cmd.native_price_estimate_min,
        cmd.native_price_estimate_max,
        fx_rate,
    ) {
        events.push(domain_event(event));
    }
    if let Some(url) = cmd.url
        && let Some(event) = product.change_url(url.clone(), append_utm_params(url))
    {
        events.push(domain_event(event));
    }
    if let Some(images) = cmd.images {
        events.extend(product.change_images(images));
    }
    if (cmd.auction_start.is_some() || cmd.auction_end.is_some())
        && let Some(event) = product.change_auction_time(cmd.auction_start, cmd.auction_end)
    {
        events.push(domain_event(event));
    }
    if let Some(embedding) = cmd.embedding
        && let Some(event) = product.embed(embedding)
    {
        events.push(enrichment_event(event));
    }
    if let Some(Translation { source, targets }) = cmd.translated_titles {
        let source_language = source.localization;
        for (target_language, title) in targets {
            if let Some(event) = product.translate_title(source_language, target_language, title) {
                events.push(enrichment_event(event));
            }
        }
    }
    events
}

fn domain_event(event: ProductDomainEvent) -> ProductEvent {
    ProductEvent {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload: ProductEventPayload::ProductDomainEvent(event.payload),
    }
}

fn enrichment_event(event: ProductEnrichmentEvent) -> ProductEvent {
    ProductEvent {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload: ProductEventPayload::ProductEnrichmentEvent(event.payload),
    }
}

#[allow(dead_code)]
fn policy_event(event: ProductPolicyEvent) -> ProductEvent {
    ProductEvent {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload: ProductEventPayload::ProductPolicyEvent(event.payload),
    }
}

fn lifecycle_event(event: ProductLifecycleEvent) -> ProductEvent {
    ProductEvent {
        aggregate_id: event.aggregate_id,
        event_id: event.event_id,
        timestamp: event.timestamp,
        payload: ProductEventPayload::ProductLifecycleEvent(event.payload),
    }
}

impl From<ProductPostgresRepositoryError> for DeleteProductError {
    fn from(value: ProductPostgresRepositoryError) -> Self {
        Self::Persist(value.to_string())
    }
}
