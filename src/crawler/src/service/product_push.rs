//! Service for pushing scraped products to the canonical product use case.

use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use application::patch_field::PatchField;
use async_trait::async_trait;
use futures::{StreamExt, stream};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::Price;
use product_listing_core::{
    description::Description, listing_availability::ListingAvailability,
    product_listing_id::ProductListingKey, title::Title,
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
    if current.listing_source_id != newer.listing_source_id
        || current.source_listing_id != newer.source_listing_id
    {
        return Err(DuplicateProductListingCommandError::ProductListingKeyMismatch);
    }

    let UpsertProductListingCommand {
        title,
        description,
        price,
        price_estimate_min,
        price_estimate_max,
        availability,
        url,
        images,
        auction_start,
        auction_end,
        ..
    } = newer;

    if let Some(value) = title {
        current.title = Some(value);
    }
    if let Some(value) = description {
        current.description = Some(value);
    }
    merge_latest_explicit_patch(&mut current.price, price);
    merge_latest_explicit_patch(&mut current.price_estimate_min, price_estimate_min);
    merge_latest_explicit_patch(&mut current.price_estimate_max, price_estimate_max);
    merge_latest_explicit_patch(&mut current.availability, availability);
    if let Some(value) = url {
        current.url = Some(value);
    }
    merge_latest_explicit_patch(&mut current.images, images);
    merge_latest_explicit_patch(&mut current.auction_start, auction_start);
    merge_latest_explicit_patch(&mut current.auction_end, auction_end);
    Ok(())
}

fn merge_latest_explicit_patch<T>(current: &mut PatchField<T>, newer: PatchField<T>) {
    if !matches!(newer, PatchField::Unchanged) {
        *current = newer;
    }
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
            let key = ProductListingKey::new(
                command.listing_source_id,
                command.source_listing_id.clone(),
            );
            if let Some(&group_index) = group_by_key.get(&key) {
                let group = &mut groups[group_index];
                group.input_indices.push(input_index);
                if group.valid
                    && let Err(error) = merge_upsert_command(&mut group.command, command)
                {
                    group.valid = false;
                    warn!(
                        listing_source_id = %group.command.listing_source_id,
                        source_listing_id = %group.command.source_listing_id,
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
                let listing_source_id = command.listing_source_id;
                let source_listing_id = command.source_listing_id.clone();

                let succeeded = match upsert_product.execute(&context, command).await {
                    Ok(_) => true,
                    Err(error) => {
                        warn!(
                            error = %error,
                            listing_source_id = %listing_source_id,
                            source_listing_id = %source_listing_id,
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
    }
}

fn crawler_operation_context(command: &UpsertProductListingCommand) -> OperationContext {
    let product_key = format!(
        "crawler:{}:{}",
        command.listing_source_id, command.source_listing_id
    );

    OperationContext {
        principal: Principal::Service("crawler".to_owned()),
        request_id: RequestId::new(product_key.clone()),
        correlation_id: CorrelationId::new(product_key),
    }
}

/// Writes display-only upsert snapshots to a configured output file.
///
/// These snapshots are not command replay input. They retain every patch field's intent using
/// an explicit tagged representation.
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

/// Display-only record of a command. It deliberately has no deserializer because this file is
/// never an upsert-command replay source.
#[derive(Debug, serde::Serialize)]
struct UpsertCommandSnapshot {
    listing_source_id: String,
    source_listing_id: String,
    title: Option<LocalizedTextSnapshot>,
    description: Option<LocalizedTextSnapshot>,
    price: PricePatchSnapshot,
    price_estimate_min: PricePatchSnapshot,
    price_estimate_max: PricePatchSnapshot,
    availability: AvailabilityPatchSnapshot,
    url: Option<String>,
    images: ImagesPatchSnapshot,
    auction_start: TimestampPatchSnapshot,
    auction_end: TimestampPatchSnapshot,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    raw_attributes: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
enum AvailabilityPatchSnapshot {
    Set { value: String },
    Clear,
    Unchanged,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
enum PricePatchSnapshot {
    Set { value: PriceSnapshot },
    Clear,
    Unchanged,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
enum ImagesPatchSnapshot {
    Set {
        value: Vec<ProductListingImageSnapshot>,
    },
    Clear,
    Unchanged,
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "state", rename_all = "SCREAMING_SNAKE_CASE")]
enum TimestampPatchSnapshot {
    Set { value: String },
    Clear,
    Unchanged,
}

#[derive(Debug, serde::Serialize)]
struct LocalizedTextSnapshot {
    language: String,
    text: String,
}

#[derive(Debug, serde::Serialize)]
struct PriceSnapshot {
    amount: u64,
    currency: String,
}

#[derive(Debug, serde::Serialize)]
struct ProductListingImageSnapshot {
    url: String,
}

impl From<&ProductListingPushItem> for UpsertCommandSnapshot {
    fn from(product: &ProductListingPushItem) -> Self {
        let command = &product.command;
        Self {
            listing_source_id: command.listing_source_id.to_string(),
            source_listing_id: command.source_listing_id.to_string(),
            title: command.title.as_ref().map(snapshot_localized_title),
            description: command
                .description
                .as_ref()
                .map(snapshot_localized_description),
            price: snapshot_price_patch(&command.price),
            price_estimate_min: snapshot_price_patch(&command.price_estimate_min),
            price_estimate_max: snapshot_price_patch(&command.price_estimate_max),
            availability: match command.availability {
                PatchField::Set(availability) => AvailabilityPatchSnapshot::Set {
                    value: availability_name(availability).to_owned(),
                },
                PatchField::Clear => AvailabilityPatchSnapshot::Clear,
                PatchField::Unchanged => AvailabilityPatchSnapshot::Unchanged,
            },
            url: command.url.as_ref().map(ToString::to_string),
            images: snapshot_images_patch(&command.images),
            auction_start: snapshot_timestamp_patch(&command.auction_start),
            auction_end: snapshot_timestamp_patch(&command.auction_end),
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

fn snapshot_price_patch(value: &PatchField<Price>) -> PricePatchSnapshot {
    match value {
        PatchField::Set(price) => PricePatchSnapshot::Set {
            value: snapshot_price(*price),
        },
        PatchField::Clear => PricePatchSnapshot::Clear,
        PatchField::Unchanged => PricePatchSnapshot::Unchanged,
    }
}

fn snapshot_images_patch(
    value: &PatchField<IndexSet<product_listing_core::product_listing_image::ProductListingImage>>,
) -> ImagesPatchSnapshot {
    match value {
        PatchField::Set(images) => ImagesPatchSnapshot::Set {
            value: images
                .iter()
                .map(|image| ProductListingImageSnapshot {
                    url: image.url().to_string(),
                })
                .collect(),
        },
        PatchField::Clear => ImagesPatchSnapshot::Clear,
        PatchField::Unchanged => ImagesPatchSnapshot::Unchanged,
    }
}

fn snapshot_timestamp_patch(value: &PatchField<time::OffsetDateTime>) -> TimestampPatchSnapshot {
    match value {
        PatchField::Set(value) => TimestampPatchSnapshot::Set {
            value: value.to_string(),
        },
        PatchField::Clear => TimestampPatchSnapshot::Clear,
        PatchField::Unchanged => TimestampPatchSnapshot::Unchanged,
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

        // Existing entries are retained for display only. Do not deserialize them as commands:
        // the file is not a replay source and may contain older display formats.
        let mut snapshots: Vec<serde_json::Value> = if self.output_path.exists() {
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

        let new_snapshots = match products
            .iter()
            .map(UpsertCommandSnapshot::from)
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
        {
            Ok(snapshots) => snapshots,
            Err(error) => {
                error!(error = %error, "Failed to serialize scraped product snapshots");
                return vec![false; products.len()];
            }
        };
        snapshots.extend(new_snapshots);

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
        listing_source_id: ListingSourceId::from(uuid::Uuid::from(candidate.listing_source_id)),
        source_listing_id: product.source_listing_id,
        title: Some(product.title),
        description: product.description,
        price: match product.price {
            Some(price) => PatchField::Set(price),
            None => PatchField::Clear,
        },
        price_estimate_min: option_to_patch(product.price_estimate_min),
        price_estimate_max: option_to_patch(product.price_estimate_max),
        availability: match product.availability {
            crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping::Availability(availability) => PatchField::Set(availability),
            crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping::NoAssertion => PatchField::Clear,
            crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping::Ignore => PatchField::Unchanged,
        },
        url: Some(product.url),
        images: PatchField::Set(product.images.into_iter().collect::<IndexSet<_>>()),
        auction_start: option_to_patch(product.auction_start),
        auction_end: option_to_patch(product.auction_end),
    })
}

fn option_to_patch<T>(value: Option<T>) -> PatchField<T> {
    match value {
        Some(value) => PatchField::Set(value),
        None => PatchField::Clear,
    }
}

fn availability_name(value: ListingAvailability) -> &'static str {
    value.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use listing_source_core::ListingSourceId;
    use product_listing_core::{
        product_listing_id::ProductListingId, source_listing_id::SourceListingId,
    };
    use product_listing_service::use_cases::commands::{
        update_product_listing::UpdateProductListingResult,
        upsert_product_listing::{UpsertProductListingError, UpsertProductListingResult},
    };

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
                return Err(UpsertProductListingError::ListingSourceNotFound);
            }

            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    product_listing_id: ProductListingId::new(),
                    event_id: None,
                },
            ))
        }
    }

    fn command() -> Result<UpsertProductListingCommand, url::ParseError> {
        Ok(UpsertProductListingCommand {
            listing_source_id: ListingSourceId::new(),
            source_listing_id: SourceListingId::try_from("prod-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: Some(Localized::new(Language::De, Title::from("Ein Schrank"))),
            description: None,
            price: PatchField::Unchanged,
            price_estimate_min: PatchField::Unchanged,
            price_estimate_max: PatchField::Unchanged,
            availability: PatchField::Set(ListingAvailability::Available),
            url: Some(Url::parse("https://example.com/product/1")?),
            images: PatchField::Unchanged,
            auction_start: PatchField::Unchanged,
            auction_end: PatchField::Unchanged,
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
        item.command.source_listing_id = SourceListingId::try_from(id)
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
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
                    product_listing_id: ProductListingId::new(),
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
            match command.source_listing_id.to_string().as_str() {
                "slow-ok" => sleep(Duration::from_millis(40)).await,
                "fast-fail" => {
                    sleep(Duration::from_millis(1)).await;
                    return Err(UpsertProductListingError::ListingSourceNotFound);
                }
                "medium-ok" => sleep(Duration::from_millis(15)).await,
                other => panic!("unexpected product id: {other}"),
            }

            Ok(UpsertProductListingResult::Updated(
                UpdateProductListingResult {
                    product_listing_id: ProductListingId::new(),
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
            let product_key = format!(
                "crawler:{}:{}",
                command.listing_source_id, command.source_listing_id
            );
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
        first.command.images = PatchField::Set(IndexSet::from([
            product_listing_core::product_listing_image::ProductListingImage::new(Url::parse(
                "https://example.com/image.jpg",
            )?),
        ]));
        let mut second = first.clone();
        second.command.images = PatchField::Set(IndexSet::new());

        assert_eq!(service.push(vec![first, second]).await, vec![true, true]);
        let executed = commands
            .lock()
            .map(|commands| commands.clone())
            .unwrap_or_else(|error| error.into_inner().clone());
        assert!(matches!(
            &executed[0].images,
            PatchField::Set(images) if images.is_empty()
        ));
        Ok(())
    }

    #[tokio::test]
    async fn should_fan_out_group_failure() -> Result<(), url::ParseError> {
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
        Ok(())
    }

    #[test]
    fn should_keep_latest_explicit_patch_when_coalescing() -> Result<(), url::ParseError> {
        let price = Price::new(money::MonetaryAmount::from(120_u64), money::Currency::Eur);
        let mut current = command()?;
        let auction_at = time::OffsetDateTime::UNIX_EPOCH;
        current.price = PatchField::Set(price);
        current.price_estimate_min = PatchField::Set(price);
        current.price_estimate_max = PatchField::Set(price);
        current.availability = PatchField::Set(ListingAvailability::Available);
        current.images = PatchField::Set(IndexSet::new());
        current.auction_start = PatchField::Set(auction_at);
        current.auction_end = PatchField::Set(auction_at);

        let mut clear = current.clone();
        clear.price = PatchField::Clear;
        clear.price_estimate_min = PatchField::Clear;
        clear.price_estimate_max = PatchField::Clear;
        clear.availability = PatchField::Clear;
        clear.images = PatchField::Clear;
        clear.auction_start = PatchField::Clear;
        clear.auction_end = PatchField::Clear;
        assert!(merge_upsert_command(&mut current, clear).is_ok());
        assert!(matches!(current.price, PatchField::Clear));
        assert!(matches!(current.price_estimate_min, PatchField::Clear));
        assert!(matches!(current.price_estimate_max, PatchField::Clear));
        assert!(matches!(current.availability, PatchField::Clear));
        assert!(matches!(current.images, PatchField::Clear));
        assert!(matches!(current.auction_start, PatchField::Clear));
        assert!(matches!(current.auction_end, PatchField::Clear));

        let mut set = current.clone();
        set.price = PatchField::Set(price);
        set.price_estimate_min = PatchField::Set(price);
        set.price_estimate_max = PatchField::Set(price);
        set.availability = PatchField::Set(ListingAvailability::OutOfStock);
        set.images = PatchField::Set(IndexSet::new());
        set.auction_start = PatchField::Set(auction_at);
        set.auction_end = PatchField::Set(auction_at);
        assert!(merge_upsert_command(&mut current, set).is_ok());
        assert!(matches!(current.price, PatchField::Set(value) if value == price));
        assert!(matches!(current.price_estimate_min, PatchField::Set(value) if value == price));
        assert!(matches!(current.price_estimate_max, PatchField::Set(value) if value == price));
        assert!(matches!(
            current.availability,
            PatchField::Set(ListingAvailability::OutOfStock)
        ));
        assert!(matches!(current.images, PatchField::Set(ref images) if images.is_empty()));
        assert!(matches!(current.auction_start, PatchField::Set(value) if value == auction_at));
        assert!(matches!(current.auction_end, PatchField::Set(value) if value == auction_at));

        let mut unchanged = current.clone();
        unchanged.price = PatchField::Unchanged;
        unchanged.price_estimate_min = PatchField::Unchanged;
        unchanged.price_estimate_max = PatchField::Unchanged;
        unchanged.availability = PatchField::Unchanged;
        unchanged.images = PatchField::Unchanged;
        unchanged.auction_start = PatchField::Unchanged;
        unchanged.auction_end = PatchField::Unchanged;
        assert!(merge_upsert_command(&mut current, unchanged).is_ok());
        assert!(matches!(current.price, PatchField::Set(value) if value == price));
        assert!(matches!(current.price_estimate_min, PatchField::Set(value) if value == price));
        assert!(matches!(current.price_estimate_max, PatchField::Set(value) if value == price));
        assert!(matches!(
            current.availability,
            PatchField::Set(ListingAvailability::OutOfStock)
        ));
        assert!(matches!(current.images, PatchField::Set(ref images) if images.is_empty()));
        assert!(matches!(current.auction_start, PatchField::Set(value) if value == auction_at));
        assert!(matches!(current.auction_end, PatchField::Set(value) if value == auction_at));
        Ok(())
    }

    #[test]
    fn should_reject_mismatched_product_listing_keys() -> Result<(), url::ParseError> {
        let mut current = command()?;
        let newer = current.clone();
        assert!(merge_upsert_command(&mut current, newer).is_ok());

        let mut mismatched = current.clone();
        mismatched.source_listing_id = SourceListingId::try_from("other-product")
            .unwrap_or_else(|error| panic!("valid source listing ID: {error}"));
        assert_eq!(
            merge_upsert_command(&mut current, mismatched),
            Err(DuplicateProductListingCommandError::ProductListingKeyMismatch)
        );
        Ok(())
    }

    #[test]
    fn should_map_normalized_product_to_listing_source_handoff() -> Result<(), url::ParseError> {
        let candidate = ScraperCandidate {
            listing_source_id: ListingSourceId::new(),
            listing_source_name: "Test source".to_owned(),
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
            last_scraped_presence: "PRESENT".to_owned(),
            last_scraped_availability: None,
        };
        let product = NormalizedProduct {
            source_listing_id: SourceListingId::try_from("prod-1")
                .unwrap_or_else(|error| panic!("valid source listing ID: {error}")),
            title: Localized::new(Language::De, Title::from("Ein Schrank")),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            availability: crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping::Availability(ListingAvailability::Available),
            url: candidate.url.clone(),
            images: Vec::new(),
            auction_start: None,
            auction_end: None,
            raw_attributes: BTreeMap::new(),
        };

        let command = normalize_to_upsert(product.clone(), &candidate);

        assert_eq!(
            command.as_ref().map(|command| command.listing_source_id),
            Some(ListingSourceId::from(uuid::Uuid::from(
                candidate.listing_source_id
            )))
        );
        assert_eq!(
            command
                .as_ref()
                .map(|command| command.source_listing_id.to_string()),
            Some("prod-1".to_owned())
        );
        assert!(matches!(
            command.as_ref().map(|command| &command.price),
            Some(PatchField::Clear)
        ));
        assert!(matches!(
            command.as_ref().map(|command| &command.price_estimate_min),
            Some(PatchField::Clear)
        ));
        assert!(matches!(
            command.as_ref().map(|command| &command.price_estimate_max),
            Some(PatchField::Clear)
        ));
        assert!(matches!(
            command.as_ref().map(|command| &command.availability),
            Some(PatchField::Set(ListingAvailability::Available))
        ));
        assert!(matches!(
            command.as_ref().map(|command| &command.images),
            Some(PatchField::Set(images)) if images.is_empty()
        ));
        assert!(matches!(
            command.as_ref().map(|command| &command.auction_start),
            Some(PatchField::Clear)
        ));
        assert!(matches!(
            command.as_ref().map(|command| &command.auction_end),
            Some(PatchField::Clear)
        ));

        let no_assertion_command = normalize_to_upsert(
            NormalizedProduct {
                availability: crate::scraper::normalization::listing_availability_mapping::ListingAvailabilityMapping::NoAssertion,
                ..product
            },
            &candidate,
        );
        assert!(matches!(
            no_assertion_command
                .as_ref()
                .map(|command| &command.availability),
            Some(PatchField::Clear)
        ));
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

    #[test]
    fn should_serialize_all_patch_states_without_aliasing() -> Result<(), Box<dyn std::error::Error>>
    {
        let price = Price::new(money::MonetaryAmount::from(100_u64), money::Currency::Eur);
        let auction_at = time::OffsetDateTime::UNIX_EPOCH;
        let mut set = push_item()?;
        set.command.price = PatchField::Set(price);
        set.command.price_estimate_min = PatchField::Set(price);
        set.command.price_estimate_max = PatchField::Set(price);
        set.command.images = PatchField::Set(IndexSet::new());
        set.command.auction_start = PatchField::Set(auction_at);
        set.command.auction_end = PatchField::Set(auction_at);

        let mut clear = set.clone();
        clear.command.price = PatchField::Clear;
        clear.command.price_estimate_min = PatchField::Clear;
        clear.command.price_estimate_max = PatchField::Clear;
        clear.command.availability = PatchField::Clear;
        clear.command.images = PatchField::Clear;
        clear.command.auction_start = PatchField::Clear;
        clear.command.auction_end = PatchField::Clear;

        let mut unchanged = set.clone();
        unchanged.command.price = PatchField::Unchanged;
        unchanged.command.price_estimate_min = PatchField::Unchanged;
        unchanged.command.price_estimate_max = PatchField::Unchanged;
        unchanged.command.availability = PatchField::Unchanged;
        unchanged.command.images = PatchField::Unchanged;
        unchanged.command.auction_start = PatchField::Unchanged;
        unchanged.command.auction_end = PatchField::Unchanged;

        let set_snapshot = serde_json::to_value(UpsertCommandSnapshot::from(&set))?;
        assert_eq!(
            set_snapshot["price"],
            serde_json::json!({
                "state": "SET",
                "value": { "amount": 100, "currency": "EUR" }
            })
        );
        assert_eq!(set_snapshot["price_estimate_min"], set_snapshot["price"]);
        assert_eq!(set_snapshot["price_estimate_max"], set_snapshot["price"]);
        assert_eq!(
            set_snapshot["availability"],
            serde_json::json!({ "state": "SET", "value": "AVAILABLE" })
        );
        assert_eq!(
            set_snapshot["images"],
            serde_json::json!({ "state": "SET", "value": [] })
        );
        assert_eq!(
            set_snapshot["auction_start"],
            serde_json::json!({ "state": "SET", "value": auction_at.to_string() })
        );
        assert_eq!(set_snapshot["auction_end"], set_snapshot["auction_start"]);

        for item in [&clear, &unchanged] {
            let expected_state = if std::ptr::eq(item, &clear) {
                "CLEAR"
            } else {
                "UNCHANGED"
            };
            let snapshot = serde_json::to_value(UpsertCommandSnapshot::from(item))?;
            for field in [
                "price",
                "price_estimate_min",
                "price_estimate_max",
                "availability",
                "images",
                "auction_start",
                "auction_end",
            ] {
                assert_eq!(
                    snapshot[field],
                    serde_json::json!({ "state": expected_state })
                );
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn should_write_display_only_snapshot() -> Result<(), url::ParseError> {
        let path = std::env::temp_dir().join(format!("product_push_{}.json", uuid::Uuid::new_v4()));
        let service = FileProductListingPushService::new(path.clone());

        let succeeded = service.push(vec![push_item()?]).await;
        assert_eq!(succeeded, vec![true]);

        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(error) => panic!("failed to read product snapshot: {error}"),
        };
        assert!(content.contains("\"listing_source_id\""));
        assert!(content.contains("\"source_listing_id\""));
        assert!(content.contains("\"state\": \"SET\""));
        assert!(content.contains("\"value\": \"AVAILABLE\""));

        let _ = std::fs::remove_file(path);
        Ok(())
    }
}
