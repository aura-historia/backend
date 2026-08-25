//! Service for pushing scraped products to the canonical product use case.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use async_trait::async_trait;
use futures::{StreamExt, stream};
use indexmap::IndexSet;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::{
    description::Description, product_listing::ProductListingAddress,
    product_listing_id::ProductListingKey, product_state::ProductState, title::Title,
};
use product_listing_service::use_cases::commands::upsert_product_listing::{
    UpsertProductListingCommand, UpsertProductListingUseCase,
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
pub trait ProductListingPushService: Send + Sync {
    async fn push(&self, products: Vec<ProductListingPushItem>) -> Vec<bool>;
}

#[derive(Debug, Clone)]
pub struct ProductListingPushItem {
    pub command: UpsertProductListingCommand,
    pub raw_attributes: BTreeMap<String, Vec<String>>,
}

/// Pushes each command through the canonical ProductListing upsert use case.
pub struct ProductListingPushServiceImpl {
    upsert_product: Arc<dyn UpsertProductListingUseCase>,
    max_concurrent_upserts: usize,
}

impl ProductListingPushServiceImpl {
    /// Maximum concurrent canonical product transactions.
    /// Must remain below authoritative business database capacity.
    /// Startup configuration validation enforces this boundary.
    pub fn new(
        upsert_product: Arc<dyn UpsertProductListingUseCase>,
        max_concurrent_upserts: usize,
    ) -> Self {
        Self {
            upsert_product,
            max_concurrent_upserts: max_concurrent_upserts.max(1),
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
enum DuplicateProductListingCommandError {
    #[error("duplicate commands have different ProductListing keys")]
    ProductListingKeyMismatch,
    #[error("duplicate commands have different seller IDs")]
    SellerMismatch,
}

struct CoalescedProductListingPush {
    command: UpsertProductListingCommand,
    input_indices: Vec<usize>,
    valid: bool,
}

fn merge_upsert_command(
    current: &mut UpsertProductListingCommand,
    newer: UpsertProductListingCommand,
) -> Result<(), DuplicateProductListingCommandError> {
    if current.shop_id != newer.shop_id || current.shop_listing_id != newer.shop_listing_id {
        return Err(DuplicateProductListingCommandError::ProductListingKeyMismatch);
    }
    if current.seller_id != newer.seller_id {
        return Err(DuplicateProductListingCommandError::SellerMismatch);
    }

    let UpsertProductListingCommand {
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
impl ProductListingPushService for ProductListingPushServiceImpl {
    #[tracing::instrument(
        name = "product_push_batch",
        skip(self, products),
        fields(total = products.len())
    )]
    async fn push(&self, products: Vec<ProductListingPushItem>) -> Vec<bool> {
        let mut results = vec![false; products.len()];
        let mut group_by_key = HashMap::<ProductListingKey, usize>::new();
        let mut groups = Vec::<CoalescedProductListingPush>::new();

        for (input_index, product) in products.into_iter().enumerate() {
            let command = product.command;
            let key = ProductListingKey::new(command.shop_id, command.shop_listing_id.clone());
            if let Some(&group_index) = group_by_key.get(&key) {
                let group = &mut groups[group_index];
                group.input_indices.push(input_index);
                if group.valid
                    && let Err(error) = merge_upsert_command(&mut group.command, command)
                {
                    group.valid = false;
                    warn!(
                        shop_id = %group.command.shop_id,
                        shop_listing_id = %group.command.shop_listing_id,
                        error_kind = %duplicate_product_command_error_kind(&error),
                        "Rejecting conflicting duplicate ProductListing commands"
                    );
                }
            } else {
                group_by_key.insert(key, groups.len());
                groups.push(CoalescedProductListingPush {
                    command,
                    input_indices: vec![input_index],
                    valid: true,
                });
            }
        }

        let upsert_product = Arc::clone(&self.upsert_product);

        let outcomes = stream::iter(groups.into_iter().filter(|group| group.valid).map(|group| {
            let upsert_product = Arc::clone(&upsert_product);

            async move {
                let CoalescedProductListingPush {
                    command,
                    input_indices,
                    ..
                } = group;

                let context = crawler_operation_context(&command);
                let shop_id = command.shop_id;
                let shop_listing_id = command.shop_listing_id.clone();

                let succeeded = match upsert_product.execute(&context, command).await {
                    Ok(_) => true,
                    Err(error) => {
                        warn!(
                            error = %error,
                            shop_id = %shop_id,
                            shop_listing_id = %shop_listing_id,
                            request_id = %context.request_id,
                            correlation_id = %context.correlation_id,
                            "ProductListing upsert failed; it will be retried on the next scrape cycle"
                        );
                        false
                    }
                };

                (input_indices, succeeded)
            }
        }))
        .buffer_unordered(self.max_concurrent_upserts)
        .collect::<Vec<_>>()
        .await;

        for (input_indices, succeeded) in outcomes {
            for input_index in input_indices {
                results[input_index] = succeeded;
            }
        }

        results
    }
}

fn duplicate_product_command_error_kind(
    error: &DuplicateProductListingCommandError,
) -> &'static str {
    match error {
        DuplicateProductListingCommandError::ProductListingKeyMismatch => "product_key_mismatch",
        DuplicateProductListingCommandError::SellerMismatch => "seller_mismatch",
    }
}

fn crawler_operation_context(command: &UpsertProductListingCommand) -> OperationContext {
    let product_key = format!("crawler:{}:{}", command.shop_id, command.shop_listing_id);

    OperationContext {
        principal: Principal::Service("crawler".to_owned()),
        request_id: RequestId::new(product_key.clone()),
        correlation_id: CorrelationId::new(product_key),
    }
}

/// Writes upsert commands as primitive JSON snapshots to a configured output file.
pub struct FileProductListingPushService {
    output_path: std::path::PathBuf,
}

impl FileProductListingPushService {
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
    shop_listing_id: String,
    address: ProductListingAddressSnapshot,
    title: Option<LocalizedTextSnapshot>,
    description: Option<LocalizedTextSnapshot>,
    price: Option<PriceSnapshot>,
    price_estimate_min: Option<PriceSnapshot>,
    price_estimate_max: Option<PriceSnapshot>,
    state: Option<String>,
    url: Option<String>,
    images: Vec<ProductListingImageSnapshot>,
    auction_start: Option<String>,
    auction_end: Option<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    raw_attributes: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct ProductListingAddressSnapshot {
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
struct ProductListingImageSnapshot {
    url: String,
    prohibited_content: String,
}

impl From<&ProductListingPushItem> for UpsertCommandSnapshot {
    fn from(product: &ProductListingPushItem) -> Self {
        let command = &product.command;
        Self {
            shop_id: command.shop_id.to_string(),
            seller_id: command.seller_id.to_string(),
            shop_listing_id: command.shop_listing_id.to_string(),
            address: ProductListingAddressSnapshot::default(),
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
                .map(|image| ProductListingImageSnapshot {
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
impl ProductListingPushService for FileProductListingPushService {
    #[tracing::instrument(
        name = "file_product_push_batch",
        skip(self, products),
        fields(total = products.len())
    )]
    async fn push(&self, products: Vec<ProductListingPushItem>) -> Vec<bool> {
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

/// Maps normalized crawler output into the canonical ProductListing upsert command.
pub fn normalize_to_upsert(
    product: NormalizedProduct,
    candidate: &ScraperCandidate,
) -> Option<UpsertProductListingCommand> {
    Some(UpsertProductListingCommand {
        shop_id: candidate.shop_id,
        seller_id: candidate.shop_id,
        shop_listing_id: product.shop_listing_id,
        address: ProductListingAddress::default(),
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
    use product_listing_core::{
        product_listing_id::ProductListingId, shop_listing_id::ShopListingId,
    };
    use product_listing_service::use_cases::commands::{
        update_product_listing::UpdateProductListingResult,
        upsert_product_listing::{UpsertProductListingError, UpsertProductListingResult},
    };
    use shop_core::{shop_id::ShopId, shop_type::ShopType};
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tokio::time::{Duration, sleep, timeout};
    use url::Url;

    #[derive(Default)]
    struct FakeUpsertProductListingUseCase {
        commands: Arc<Mutex<Vec<UpsertProductListingCommand>>>,
        contexts: Arc<Mutex<Vec<OperationContext>>>,
        fail: bool,
    }

    #[async_trait]
    impl UpsertProductListingUseCase for FakeUpsertProductListingUseCase {
        async fn execute(
            &self,
            context: &OperationContext,
            command: UpsertProductListingCommand,
        ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
            match self.commands.lock() {
                Ok(mut commands) => commands.push(command),
                Err(error) => error.into_inner().push(command),
            }
            match self.contexts.lock() {
                Ok(mut contexts) => contexts.push(context.clone()),
                Err(error) => error.into_inner().push(context.clone()),
            }

            if self.fail {
                return Err(UpsertProductListingError::ShopNotFound);
            }

            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    product_id: ProductListingId::new(),
                    event_id: None,
                },
            ))
        }
    }

    fn command() -> Result<UpsertProductListingCommand, url::ParseError> {
        Ok(UpsertProductListingCommand {
            shop_id: ShopId::new(),
            seller_id: ShopId::new(),
            shop_listing_id: ShopListingId::from("prod-1"),
            address: ProductListingAddress::default(),
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

    fn push_item() -> Result<ProductListingPushItem, url::ParseError> {
        Ok(ProductListingPushItem {
            command: command()?,
            raw_attributes: BTreeMap::new(),
        })
    }

    fn push_item_with_id(id: &str) -> Result<ProductListingPushItem, url::ParseError> {
        let mut item = push_item()?;
        item.command.shop_listing_id = ShopListingId::from(id);
        Ok(item)
    }

    struct ConcurrencyTrackingUpsert {
        active: Arc<AtomicUsize>,
        max_active: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl UpsertProductListingUseCase for ConcurrencyTrackingUpsert {
        async fn execute(
            &self,
            _: &OperationContext,
            _: UpsertProductListingCommand,
        ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);

            sleep(Duration::from_millis(25)).await;

            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    product_id: ProductListingId::new(),
                    event_id: None,
                },
            ))
        }
    }

    struct DelayedSelectiveUpsert;

    #[async_trait]
    impl UpsertProductListingUseCase for DelayedSelectiveUpsert {
        async fn execute(
            &self,
            _: &OperationContext,
            command: UpsertProductListingCommand,
        ) -> Result<UpsertProductListingResult, UpsertProductListingError> {
            match command.shop_listing_id.to_string().as_str() {
                "slow-ok" => sleep(Duration::from_millis(40)).await,
                "fast-fail" => {
                    sleep(Duration::from_millis(1)).await;
                    return Err(UpsertProductListingError::ShopNotFound);
                }
                "medium-ok" => sleep(Duration::from_millis(15)).await,
                other => panic!("unexpected product id: {other}"),
            }

            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    product_id: ProductListingId::new(),
                    event_id: None,
                },
            ))
        }
    }

    #[tokio::test]
    async fn should_bound_concurrent_product_upserts() -> Result<(), url::ParseError> {
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let use_case = Arc::new(ConcurrencyTrackingUpsert {
            active: Arc::clone(&active),
            max_active: Arc::clone(&max_active),
        });
        let service = ProductListingPushServiceImpl::new(use_case, 2);

        let products = (0..8)
            .map(|index| push_item_with_id(&format!("prod-{index}")))
            .collect::<Result<Vec<_>, _>>()?;

        let result = timeout(Duration::from_secs(2), service.push(products))
            .await
            .expect("bounded upserts must complete");

        assert_eq!(result, vec![true; 8]);
        assert_eq!(max_active.load(Ordering::SeqCst), 2);
        Ok(())
    }

    #[tokio::test]
    async fn should_restore_result_order_after_out_of_order_product_upserts()
    -> Result<(), url::ParseError> {
        let service = ProductListingPushServiceImpl::new(Arc::new(DelayedSelectiveUpsert), 3);
        let products = vec![
            push_item_with_id("slow-ok")?,
            push_item_with_id("fast-fail")?,
            push_item_with_id("medium-ok")?,
        ];

        let result = service.push(products).await;

        assert_eq!(result, vec![true, false, true]);
        Ok(())
    }

    #[tokio::test]
    async fn should_return_ordered_success_results_with_deterministic_crawler_context()
    -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductListingUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let contexts = Arc::clone(&use_case.contexts);
        let service = ProductListingPushServiceImpl::new(use_case, 1);
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
            let product_key = format!("crawler:{}:{}", command.shop_id, command.shop_listing_id);
            assert_eq!(context.principal, Principal::Service("crawler".to_owned()));
            assert_eq!(context.request_id.as_str(), product_key);
            assert_eq!(context.correlation_id.as_str(), product_key);
        }
        Ok(())
    }

    #[tokio::test]
    async fn should_coalesce_duplicate_product_keys_and_fan_out_success()
    -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductListingUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductListingPushServiceImpl::new(use_case, 1);
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
        let use_case = Arc::new(FakeUpsertProductListingUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductListingPushServiceImpl::new(use_case, 1);
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
        let use_case = Arc::new(FakeUpsertProductListingUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductListingPushServiceImpl::new(use_case, 1);
        let mut first = push_item()?;
        first.command.images.insert(
            product_listing_core::product_listing_image::ProductListingImage {
                url: Url::parse("https://example.com/image.jpg")?,
                prohibited_content:
                    product_listing_core::prohibited_content::ProhibitedContent::None,
            },
        );
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
        let use_case = Arc::new(FakeUpsertProductListingUseCase {
            fail: true,
            ..Default::default()
        });
        let commands = Arc::clone(&use_case.commands);
        let service = ProductListingPushServiceImpl::new(use_case, 1);
        let first = push_item()?;
        let second = first.clone();

        assert_eq!(service.push(vec![first, second]).await, vec![false, false]);
        assert_eq!(
            commands.lock().map(|commands| commands.len()).unwrap_or(0),
            1
        );

        let use_case = Arc::new(FakeUpsertProductListingUseCase::default());
        let commands = Arc::clone(&use_case.commands);
        let service = ProductListingPushServiceImpl::new(use_case, 1);
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
            Err(DuplicateProductListingCommandError::SellerMismatch)
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
            shop_listing_id: ShopListingId::from("prod-1"),
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
            Some(ProductListingAddress::default())
        );
        Ok(())
    }

    #[tokio::test]
    async fn should_omit_failed_commands_from_successful_batch() -> Result<(), url::ParseError> {
        let use_case = Arc::new(FakeUpsertProductListingUseCase {
            fail: true,
            ..Default::default()
        });
        let service = ProductListingPushServiceImpl::new(use_case, 1);

        assert_eq!(service.push(vec![push_item()?]).await, vec![false]);
        Ok(())
    }

    #[tokio::test]
    async fn should_write_primitive_canonical_snapshot() -> Result<(), url::ParseError> {
        let path = std::env::temp_dir().join(format!("product_push_{}.json", uuid::Uuid::new_v4()));
        let service = FileProductListingPushService::new(path.clone());

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
