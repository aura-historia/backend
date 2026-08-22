//! Service for pushing scraped products to the canonical product use case.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use indexmap::IndexSet;
use localization::{Language, Localized};
use money::Price;
use product_core::{
    description::Description, product::ProductAddress, product_id::ProductKey,
    product_state::ProductState, title::Title,
};
use product_service::use_cases::commands::upsert_product::{
    UpsertProductCommand, UpsertProductUseCase,
};

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};
use tracing::{debug, error, warn};

use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::normalization::product::NormalizedProduct;

/// Accepts a batch of product commands and reports success for each input position.
///
/// The result order always matches the input order. This lets the crawler mark exactly the
/// successfully persisted URL as scraped, even when product IDs are repeated in a batch.
#[async_trait]
#[mockall::automock]
pub trait ProductPushService: Send + Sync {
    async fn push(&self, products: Vec<ProductPushItem>) -> Vec<bool>;
}

#[derive(Debug, Clone)]
pub struct ProductPushItem {
    pub command: UpsertProductCommand,
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}

/// Pushes each command through the canonical Product upsert use case.
pub struct ProductPushServiceImpl {
    upsert_product: Arc<dyn UpsertProductUseCase>,
}

impl ProductPushServiceImpl {
    pub fn new(upsert_product: Arc<dyn UpsertProductUseCase>) -> Self {
        Self { upsert_product }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum DuplicateProductCommandError {
    #[error("duplicate commands have different Product keys")]
    ProductKeyMismatch,
    #[error("duplicate commands have different seller IDs")]
    SellerMismatch,
}

struct CoalescedProductPush {
    command: UpsertProductCommand,
    input_indices: Vec<usize>,
    valid: bool,
}

fn merge_upsert_command(
    current: &mut UpsertProductCommand,
    newer: UpsertProductCommand,
) -> Result<(), DuplicateProductCommandError> {
    if current.shop_id != newer.shop_id || current.shops_product_id != newer.shops_product_id {
        return Err(DuplicateProductCommandError::ProductKeyMismatch);
    }
    if current.seller_id != newer.seller_id {
        return Err(DuplicateProductCommandError::SellerMismatch);
    }

    let UpsertProductCommand {
        address,
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        state,
        url,
        images,
        auction_start,
        auction_end,
        ..
    } = newer;

    if let Some(value) = address.structured {
        current.address.structured = Some(value);
    }
    if let Some(value) = address.geo {
        current.address.geo = Some(value);
    }
    if let Some(value) = title {
        current.title = Some(value);
    }
    if let Some(value) = description {
        current.description = Some(value);
    }
    if let Some(value) = price {
        current.price = Some(value);
    }
    if let Some(value) = price_estimate_min {
        current.price_estimate_min = Some(value);
    }
    if let Some(value) = price_estimate_max {
        current.price_estimate_max = Some(value);
    }
    if let Some(value) = state {
        current.state = Some(value);
    }
    if let Some(value) = url {
        current.url = Some(value);
    }
    current.images = images;
    if let Some(value) = auction_start {
        current.auction_start = Some(value);
    }
    if let Some(value) = auction_end {
        current.auction_end = Some(value);
    }
    Ok(())
}

#[async_trait]
impl ProductPushService for ProductPushServiceImpl {
    #[tracing::instrument(
        name = "product_push_batch",
        skip(self, products),
        fields(total = products.len())
    )]
    async fn push(&self, products: Vec<ProductPushItem>) -> Vec<bool> {
        let mut results = vec![false; products.len()];
        let mut group_by_key = HashMap::<ProductKey, usize>::new();
        let mut groups = Vec::<CoalescedProductPush>::new();

        for (input_index, product) in products.into_iter().enumerate() {
            let command = product.command;
            let key = ProductKey::new(command.shop_id, command.shops_product_id.clone());
            if let Some(&group_index) = group_by_key.get(&key) {
                let group = &mut groups[group_index];
                group.input_indices.push(input_index);
                if group.valid
                    && let Err(error) = merge_upsert_command(&mut group.command, command)
                {
                    group.valid = false;
                    warn!(
                        shop_id = %group.command.shop_id,
                        shops_product_id = %group.command.shops_product_id,
                        error_kind = %duplicate_product_command_error_kind(&error),
                        "Rejecting conflicting duplicate Product commands"
                    );
                }
            } else {
                group_by_key.insert(key, groups.len());
                groups.push(CoalescedProductPush {
                    command,
                    input_indices: vec![input_index],
                    valid: true,
                });
            }
        }

        for group in groups {
            if !group.valid {
                continue;
            }
            let context = crawler_operation_context(&group.command);
            let succeeded = match self
                .upsert_product
                .execute(&context, group.command.clone())
                .await
            {
                Ok(_) => true,
                Err(error) => {
                    warn!(
                        error = %error,
                        shop_id = %group.command.shop_id,
                        shops_product_id = %group.command.shops_product_id,
                        request_id = %context.request_id,
                        correlation_id = %context.correlation_id,
                        "Product upsert failed; it will be retried on the next scrape cycle"
                    );
                    false
                }
            };
            for input_index in group.input_indices {
                results[input_index] = succeeded;
            }
        }

        results
    }
}

fn duplicate_product_command_error_kind(error: &DuplicateProductCommandError) -> &'static str {
    match error {
        DuplicateProductCommandError::ProductKeyMismatch => "product_key_mismatch",
        DuplicateProductCommandError::SellerMismatch => "seller_mismatch",
    }
}

fn crawler_operation_context(command: &UpsertProductCommand) -> OperationContext {
    let product_key = format!("crawler:{}:{}", command.shop_id, command.shops_product_id);

    OperationContext {
        principal: Principal::Service("crawler".to_owned()),
        request_id: RequestId::new(product_key.clone()),
        correlation_id: CorrelationId::new(product_key),
    }
}

/// Writes upsert commands as primitive JSON snapshots to a configured output file.
pub struct FileProductPushService {
    output_path: std::path::PathBuf,
}

impl FileProductPushService {
    pub fn new(output_path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            output_path: output_path.into(),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct UpsertCommandSnapshot {
    shop_id: String,
    seller_id: String,
    shops_product_id: String,
    address: ProductAddressSnapshot,
    title: Option<LocalizedTextSnapshot>,
    description: Option<LocalizedTextSnapshot>,
    price: Option<PriceSnapshot>,
    price_estimate_min: Option<PriceSnapshot>,
    price_estimate_max: Option<PriceSnapshot>,
    state: Option<String>,
    url: Option<String>,
    images: Vec<ProductImageSnapshot>,
    auction_start: Option<String>,
    auction_end: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    raw_attributes: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ProductAddressSnapshot {
    structured: Option<String>,
    geo: Option<String>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct LocalizedTextSnapshot {
    language: String,
    text: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct PriceSnapshot {
    amount: u64,
    currency: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ProductImageSnapshot {
    url: String,
    prohibited_content: String,
}

impl From<&ProductPushItem> for UpsertCommandSnapshot {
    fn from(product: &ProductPushItem) -> Self {
        let command = &product.command;
        Self {
            shop_id: command.shop_id.to_string(),
            seller_id: command.seller_id.to_string(),
            shops_product_id: command.shops_product_id.to_string(),
            address: ProductAddressSnapshot::default(),
            title: command.title.as_ref().map(snapshot_localized_title),
            description: command
                .description
                .as_ref()
                .map(snapshot_localized_description),
            price: command.price.map(snapshot_price),
            price_estimate_min: command.price_estimate_min.map(snapshot_price),
            price_estimate_max: command.price_estimate_max.map(snapshot_price),
            state: command.state.map(product_state_name).map(str::to_owned),
            url: command.url.as_ref().map(ToString::to_string),
            images: command
                .images
                .iter()
                .map(|image| ProductImageSnapshot {
                    url: image.url.to_string(),
                    prohibited_content: image.prohibited_content.as_str().to_owned(),
                })
                .collect(),
            auction_start: command.auction_start.map(|value| value.to_string()),
            auction_end: command.auction_end.map(|value| value.to_string()),
            raw_attributes: product.raw_attributes.clone(),
        }
    }
}

fn snapshot_localized_title(value: &Localized<Language, Title>) -> LocalizedTextSnapshot {
    LocalizedTextSnapshot {
        language: value.localization.as_str().to_owned(),
        text: value.payload.as_ref().to_owned(),
    }
}

fn snapshot_localized_description(
    value: &Localized<Language, Description>,
) -> LocalizedTextSnapshot {
    LocalizedTextSnapshot {
        language: value.localization.as_str().to_owned(),
        text: value.payload.as_ref().to_owned(),
    }
}

fn snapshot_price(value: Price) -> PriceSnapshot {
    PriceSnapshot {
        amount: value.monetary_amount.into(),
        currency: value.currency.as_str().to_owned(),
    }
}

#[async_trait]
impl ProductPushService for FileProductPushService {
    #[tracing::instrument(
        name = "file_product_push_batch",
        skip(self, products),
        fields(total = products.len())
    )]
    async fn push(&self, products: Vec<ProductPushItem>) -> Vec<bool> {
        if products.is_empty() {
            return Vec::new();
        }

        let mut snapshots = if self.output_path.exists() {
            match std::fs::read_to_string(&self.output_path) {
                Ok(content) => match serde_json::from_str(&content) {
                    Ok(snapshots) => snapshots,
                    Err(error) => {
                        error!(
                            error = %error,
                            path = %self.output_path.display(),
                            "Failed to parse existing scraped product snapshots"
                        );
                        return vec![false; products.len()];
                    }
                },
                Err(error) => {
                    error!(
                        error = %error,
                        path = %self.output_path.display(),
                        "Failed to read existing scraped product snapshots"
                    );
                    return vec![false; products.len()];
                }
            }
        } else {
            Vec::new()
        };

        snapshots.extend(products.iter().map(UpsertCommandSnapshot::from));

        let json = match serde_json::to_string_pretty(&snapshots) {
            Ok(json) => json,
            Err(error) => {
                error!(error = %error, "Failed to serialize scraped product snapshots");
                return Vec::new();
            }
        };

        match std::fs::write(&self.output_path, json) {
            Ok(()) => {
                debug!(
                    count = products.len(),
                    path = %self.output_path.display(),
                    "Wrote scraped product snapshots to file"
                );
                vec![true; products.len()]
            }
            Err(error) => {
                error!(
                    error = %error,
                    path = %self.output_path.display(),
                    "Failed to write scraped product snapshots"
                );
                vec![false; products.len()]
            }
        }
    }
}

/// Maps normalized crawler output into the canonical Product upsert command.
pub fn normalize_to_upsert(
    product: NormalizedProduct,
    candidate: &ScraperCandidate,
) -> Option<UpsertProductCommand> {
    Some(UpsertProductCommand {
        shop_id: candidate.shop_id,
        seller_id: candidate.shop_id,
        shops_product_id: product.shops_product_id,
        address: ProductAddress::default(),
        title: Some(product.title),
        description: product.description,
        price: product.price,
        price_estimate_min: product.price_estimate_min,
        price_estimate_max: product.price_estimate_max,
        state: Some(product.state),
        url: Some(product.url),
        images: product.images.into_iter().collect::<IndexSet<_>>(),
        auction_start: product.auction_start,
        auction_end: product.auction_end,
    })
}

fn product_state_name(value: ProductState) -> &'static str {
    match value {
        ProductState::Listed => "LISTED",
        ProductState::Available => "AVAILABLE",
        ProductState::Reserved => "RESERVED",
        ProductState::Sold => "SOLD",
        ProductState::Removed => "REMOVED",
        ProductState::Unknown => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use product_core::{product_id::ProductId, shops_product_id::ShopsProductId};
    use product_service::use_cases::commands::{
        update_product::UpdateProductResult,
        upsert_product::{UpsertProductError, UpsertProductResult},
    };
    use shop_core::{shop_id::ShopId, shop_type::ShopType};
    use std::sync::{Arc, Mutex};
    use url::Url;

    #[derive(Default)]
    struct FakeUpsertProductUseCase {
        commands: Arc<Mutex<Vec<UpsertProductCommand>>>,
        contexts: Arc<Mutex<Vec<OperationContext>>>,
        fail: bool,
    }

    #[async_trait]
    impl UpsertProductUseCase for FakeUpsertProductUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: UpsertProductCommand,
        ) -> Result<UpsertProductResult, UpsertProductError> {
            match self.commands.lock() {
                Ok(mut commands) => commands.push(command),
                Err(error) => error.into_inner().push(command),
            }
            match self.contexts.lock() {
                Ok(mut contexts) => contexts.push(context.clone()),
                Err(error) => error.into_inner().push(context.clone()),
            }

            if self.fail {
                return Err(UpsertProductError::ShopNotFound);
            }

            Ok(UpsertProductResult::Updated(UpdateProductResult {
                product_id: ProductId::new(),
                event_id: None,
            }))
        }
    }

    fn command() -> Result<UpsertProductCommand, url::ParseError> {
        Ok(UpsertProductCommand {
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shops_product_id: ShopsProductId::from("prod-1"),
            address: ProductAddress::default(),
            title: Some(Localized::new(Language::De, Title::from("Ein Schrank"))),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: Some(ProductState::Available),
            url: Some(Url::parse("https://example.com/product/1")?),
            images: Default::default(),
            auction_start: None,
            auction_end: None,
        })
    }

    fn push_item() -> Result<ProductPushItem, url::ParseError> {
        Ok(ProductPushItem {
            command: command()?,
            raw_attributes: BTreeMap::new(),
        })
    }

    #[tokio::test]
    async fn should_return_ordered_success_results_with_deterministic_crawler_context()
    -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let contexts = Arc::clone(&use_case.contexts);
        let service = ProductPushServiceImpl::new(use_case);
        let succeeded = service.push(vec![push_item()?, push_item()?]).await;

        assert_eq!(succeeded, vec![true, true]);
        let executed_commands = match commands.lock() {
            Ok(commands) => commands.clone(),
            Err(error) => error.into_inner().clone(),
        };
        let executed_contexts = match contexts.lock() {
            Ok(contexts) => contexts.clone(),
            Err(error) => error.into_inner().clone(),
        };
        assert_eq!(executed_commands.len(), 2);
        assert_eq!(executed_contexts.len(), 2);
        for (context, command) in executed_contexts.iter().zip(executed_commands) {
            let product_key = format!("crawler:{}:{}", command.shop_id, command.shops_product_id);
            assert_eq!(context.principal, Principal::Service("crawler".to_owned()));
            assert_eq!(context.request_id.as_str(), product_key);
            assert_eq!(context.correlation_id.as_str(), product_key);
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_coalesce_duplicate_product_keys_and_fan_out_success()
    -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductPushServiceImpl::new(use_case);
        let item = push_item()?;

        assert_eq!(
            service.push(vec![item.clone(), item]).await,
            vec![true, true]
        );
        let executed = commands
            .lock()
            .map(|commands| commands.clone())
            .unwrap_or_else(|error| error.into_inner().clone());
        assert_eq!(executed.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn should_merge_later_optional_values_without_erasing_with_none()
    -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductPushServiceImpl::new(use_case);
        let mut first = push_item()?;
        first.command.description = Some(Localized::new(
            Language::De,
            Description::from("Earlier description"),
        ));
        let mut second = first.clone();
        second.command.title = Some(Localized::new(Language::En, Title::from("Later title")));
        second.command.description = None;

        assert_eq!(service.push(vec![first, second]).await, vec![true, true]);
        let executed = commands
            .lock()
            .map(|commands| commands.clone())
            .unwrap_or_else(|error| error.into_inner().clone());
        assert_eq!(executed.len(), 1);
        assert_eq!(
            executed[0]
                .title
                .as_ref()
                .map(|title| title.payload.as_ref()),
            Some("Later title")
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_replace_images_with_newest_snapshot_including_empty_set()
    -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductPushServiceImpl::new(use_case);
        let mut first = push_item()?;
        first
            .command
            .images
            .insert(product_core::product_image::ProductImage {
                url: Url::parse("https://example.com/image.jpg")?,
                prohibited_content: product_core::prohibited_content::ProhibitedContent::None,
            });
        let mut second = first.clone();
        second.command.images.clear();

        assert_eq!(service.push(vec![first, second]).await, vec![true, true]);
        let executed = commands
            .lock()
            .map(|commands| commands.clone())
            .unwrap_or_else(|error| error.into_inner().clone());
        assert!(executed[0].images.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn should_fan_out_group_failure_and_reject_seller_mismatch() -> Result<(), url::ParseError>
    {
        let use_case = Arc::new(FakeUpsertProductUseCase {
            fail: true,
            ..Default::default()
        });
        let commands = Arc::clone(&use_case.commands);
        let service = ProductPushServiceImpl::new(use_case);
        let first = push_item()?;
        let second = first.clone();

        assert_eq!(service.push(vec![first, second]).await, vec![false, false]);
        assert_eq!(
            commands.lock().map(|commands| commands.len()).unwrap_or(0),
            1
        );

        let use_case = Arc::new(FakeUpsertProductUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductPushServiceImpl::new(use_case);
        let first = push_item()?;
        let mut second = first.clone();
        second.command.seller_id = ShopId::new();
        assert_eq!(service.push(vec![first, second]).await, vec![false, false]);
        assert_eq!(
            commands.lock().map(|commands| commands.len()).unwrap_or(0),
            0
        );
        Ok(())
    }

    #[test]
    fn should_merge_matching_product_commands_with_explicit_invariants()
    -> Result<(), url::ParseError> {
        let mut current = command()?;
        let newer = current.clone();
        assert!(merge_upsert_command(&mut current, newer).is_ok());

        let mut mismatched = current.clone();
        mismatched.seller_id = ShopId::new();
        assert_eq!(
            merge_upsert_command(&mut current, mismatched),
            Err(DuplicateProductCommandError::SellerMismatch)
        );
        Ok(())
    }

    #[test]
    fn should_map_normalized_product_with_candidate_as_seller() -> Result<(), url::ParseError> {
        let candidate = ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: "Test Shop".to_owned(),
            shop_type: ShopType::CommercialDealer,
            url_pattern: None,
            url: Url::parse("https://example.com/product/1")?,
            last_scraped_hash: None,
            last_scraped_price: None,
            last_scraped_price_estimate_min: None,
            last_scraped_price_estimate_max: None,
            last_scraped_url: None,
            last_scraped_images_hash: None,
            last_scraped_auction_start: None,
            last_scraped_auction_end: None,
            last_scraped_state: None,
        };
        let product = NormalizedProduct {
            shops_product_id: ShopsProductId::from("prod-1"),
            title: Localized::new(Language::De, Title::from("Ein Schrank")),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: ProductState::Available,
            url: candidate.url.clone(),
            images: Vec::new(),
            auction_start: None,
            auction_end: None,
            raw_attributes: BTreeMap::new(),
        };

        let command = normalize_to_upsert(product, &candidate);

        assert_eq!(
            command.as_ref().map(|command| command.shop_id),
            Some(candidate.shop_id)
        );
        assert_eq!(
            command.as_ref().map(|command| command.seller_id),
            Some(candidate.shop_id)
        );
        assert_eq!(
            command.as_ref().map(|command| command.address.clone()),
            Some(ProductAddress::default())
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_omit_failed_commands_from_successful_batch() -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductUseCase {
            fail: true,
            ..Default::default()
        });
        let service = ProductPushServiceImpl::new(use_case);

        assert_eq!(service.push(vec![push_item()?]).await, vec![false]);
        Ok(())
    }

    #[tokio::test]
    async fn should_write_primitive_canonical_snapshot() -> Result<(), url::ParseError> {
        let path = std::env::temp_dir().join(format!("product_push_{}.json", uuid::Uuid::new_v4()));
        let service = FileProductPushService::new(path.clone());

        let succeeded = service.push(vec![push_item()?]).await;
        assert_eq!(succeeded, vec![true]);

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => panic!("failed to read product snapshot: {error}"),
        };
        assert!(content.contains("\"seller_id\""));
        assert!(content.contains("\"state\": \"AVAILABLE\""));

        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
