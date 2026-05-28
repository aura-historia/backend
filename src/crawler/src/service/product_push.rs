//! Service for pushing scraped products to the product backend.
//!
//! # Overview
//!
//! After a product URL is scraped and normalized into a [`NormalizedProduct`], it must be
//! forwarded to the product backend so that it can be created or updated in the data store.
//! This module provides:
//!
//! - [`ProductPushService`] — the trait that callers use.
//! - [`ProductPushServiceImpl`] — the production implementation backed by
//!   [`CommandProductService`].
//! - [`FileProductPushService`] — a demo/dev implementation that writes commands as JSON to
//!   a file instead of calling DynamoDB.
//! - [`normalize_to_upsert`] — the pure mapping function from a [`NormalizedProduct`] plus
//!   [`ScraperCandidate`] metadata to a [`UpsertProductCommand`].

use async_trait::async_trait;
use common::language::data::LocalizedTextData;
use common::price::data::PriceData;
use product::data::product_image_data::ProductImageData;
use product::data::product_state_data::ProductStateData;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use shop::core::shop_type::ShopType;
use time::OffsetDateTime;
use tracing::{debug, error, warn};

use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::normalization::product::NormalizedProduct;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Accepts a batch of [`UpsertProductCommand`]s and forwards them to the backend.
///
/// Returns the subset of commands that were **successfully** persisted.
/// Failed commands are logged but not propagated — the crawler retries on the
/// next scraping cycle.  Callers should only call
/// [`ScraperCandidateService::mark_as_scraped`] for the returned (succeeded)
/// commands.
#[async_trait]
#[mockall::automock]
pub trait ProductPushService: Send + Sync {
    async fn push(&self, commands: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand>;
}

// ---------------------------------------------------------------------------
// Production implementation
// ---------------------------------------------------------------------------

/// Pushes commands to [`CommandProductService`] (backed by DynamoDB in production).
pub struct ProductPushServiceImpl {
    command_service: Box<dyn CommandProductService + Send + Sync>,
}

impl ProductPushServiceImpl {
    pub fn new(command_service: Box<dyn CommandProductService + Send + Sync>) -> Self {
        Self { command_service }
    }
}

#[async_trait]
impl ProductPushService for ProductPushServiceImpl {
    #[tracing::instrument(
        name = "product_push_batch",
        skip(self, commands),
        fields(total = commands.len())
    )]
    async fn push(&self, commands: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        let count = commands.len();
        let failed = self.command_service.upsert(commands.clone()).await;
        if !failed.is_empty() {
            warn!(
                failures = failed.len(),
                total = count,
                "Some products failed to upsert"
            );
            for cmd in &failed {
                let shop_id_uuid: uuid::Uuid = cmd.shop_id.into();
                warn!(
                    shop_id = %shop_id_uuid,
                    shops_product_id = %cmd.shops_product_id,
                    url = ?cmd.url.as_ref().map(|u| u.as_str()),
                    "Product failed to upsert — will be retried on next scrape cycle"
                );
            }
        }
        let failed_ids: std::collections::HashSet<_> = failed
            .iter()
            .map(|c| (&c.shop_id, c.shops_product_id.to_string()))
            .collect();
        commands
            .into_iter()
            .filter(|c| !failed_ids.contains(&(&c.shop_id, c.shops_product_id.to_string())))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// File-based implementation (demo / dev)
// ---------------------------------------------------------------------------

/// Writes upserted commands as pretty-printed JSON to the configured output path.
///
/// Used by the demo binary where no DynamoDB/AWS is available.  Each entry in the
/// JSON array is an [`UpsertCommandSnapshot`] containing the **full** product payload —
/// title, description, price, images, state, and auction dates — in addition to the
/// shop identity fields.  This makes the file output a faithful representation of what
/// would be forwarded to DynamoDB in production.
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

/// Serializable snapshot of a single upsert command, used only for demo/file output.
///
/// Captures the full product payload so that the JSON written to disk is a faithful
/// representation of what would be sent to DynamoDB in production.  This includes title,
/// description, price, images, state, and auction dates — not just the identity fields.
#[derive(serde::Serialize, serde::Deserialize)]
struct UpsertCommandSnapshot {
    shop_id: String,
    shops_product_id: String,
    seller_name_raw: Option<String>,
    url: Option<String>,
    state: Option<ProductStateData>,
    title: Option<LocalizedTextData>,
    description: Option<LocalizedTextData>,
    price: Option<PriceData>,
    price_estimate_min: Option<PriceData>,
    price_estimate_max: Option<PriceData>,
    images: Vec<ProductImageData>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_datetime",
        deserialize_with = "deserialize_optional_datetime",
        default
    )]
    auction_start: Option<OffsetDateTime>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_optional_datetime",
        deserialize_with = "deserialize_optional_datetime",
        default
    )]
    auction_end: Option<OffsetDateTime>,
}

// `time::serde::rfc3339` only works on `OffsetDateTime` directly, not `Option<OffsetDateTime>`.
// These thin wrappers adapt it for optional fields.
fn serialize_optional_datetime<S>(
    value: &Option<OffsetDateTime>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(dt) => time::serde::rfc3339::serialize(dt, serializer),
        None => serializer.serialize_none(),
    }
}

fn deserialize_optional_datetime<'de, D>(
    deserializer: D,
) -> Result<Option<OffsetDateTime>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    time::serde::rfc3339::option::deserialize(deserializer)
}

impl From<&UpsertProductCommand> for UpsertCommandSnapshot {
    fn from(cmd: &UpsertProductCommand) -> Self {
        let shop_id_uuid: uuid::Uuid = cmd.shop_id.into();
        Self {
            shop_id: shop_id_uuid.to_string(),
            shops_product_id: cmd.shops_product_id.to_string(),
            seller_name_raw: cmd.seller_name_raw.clone(),
            url: cmd.url.as_ref().map(|u| u.to_string()),
            state: cmd.state.as_ref().map(|s| ProductStateData::from(*s)),
            title: cmd.native_title.as_ref().map(|t| t.clone().into()),
            description: cmd.native_description.as_ref().map(|d| d.clone().into()),
            price: cmd.native_price.map(PriceData::from),
            price_estimate_min: cmd.native_price_estimate_min.map(PriceData::from),
            price_estimate_max: cmd.native_price_estimate_max.map(PriceData::from),
            images: cmd
                .images
                .iter()
                .map(|i| ProductImageData::from_with_consent(i.clone(), true))
                .collect(),
            auction_start: cmd.auction_start,
            auction_end: cmd.auction_end,
        }
    }
}

#[async_trait]
impl ProductPushService for FileProductPushService {
    #[tracing::instrument(
        name = "file_product_push_batch",
        skip(self, commands),
        fields(total = commands.len())
    )]
    async fn push(&self, commands: Vec<UpsertProductCommand>) -> Vec<UpsertProductCommand> {
        if commands.is_empty() {
            return commands;
        }

        // Load any previously written products so we append rather than overwrite.
        let mut existing: Vec<UpsertCommandSnapshot> = if self.output_path.exists() {
            match std::fs::read_to_string(&self.output_path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
                Err(_) => vec![],
            }
        } else {
            vec![]
        };

        let new_snapshots: Vec<UpsertCommandSnapshot> =
            commands.iter().map(UpsertCommandSnapshot::from).collect();
        let count = new_snapshots.len();
        existing.extend(new_snapshots);

        match serde_json::to_string_pretty(&existing) {
            Ok(json) => {
                if let Err(e) = std::fs::write(&self.output_path, json) {
                    error!(
                        error = %e,
                        path = %self.output_path.display(),
                        "Failed to write scraped_products.json"
                    );
                } else {
                    debug!(
                        count,
                        path = %self.output_path.display(),
                        "Wrote scraped products to file"
                    );
                }
            }
            Err(e) => {
                error!(error = %e, "Failed to serialise upsert commands to JSON");
            }
        }
        // File push always "succeeds" — return all commands as succeeded.
        commands
    }
}

// ---------------------------------------------------------------------------
// Mapping helper
// ---------------------------------------------------------------------------

/// Maps a [`NormalizedProduct`] together with metadata from its [`ScraperCandidate`] into
/// an [`UpsertProductCommand`].
///
pub fn normalize_to_upsert(
    product: NormalizedProduct,
    candidate: &ScraperCandidate,
) -> Option<UpsertProductCommand> {
    let seller_name_raw = match candidate.shop_type {
        ShopType::CommercialDealer | ShopType::AuctionHouse => None,
        ShopType::Marketplace | ShopType::AuctionPlatform => product
            .seller_name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string),
    };

    Some(UpsertProductCommand {
        shop_id: candidate.shop_id,
        shops_product_id: product.shops_product_id,
        seller_name_raw,
        structured_address: None,
        geo_address: None,
        native_title: Some(product.title),
        native_description: product.description,
        native_price: product.price,
        native_price_estimate_min: product.price_estimate_min,
        native_price_estimate_max: product.price_estimate_max,
        state: Some(product.state),
        url: Some(product.url),
        images: product.images.into_iter().collect(),
        auction_start: product.auction_start,
        auction_end: product.auction_end,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use product::core::title::Title;
    use url::Url;

    fn make_candidate(shop_type: ShopType) -> ScraperCandidate {
        ScraperCandidate {
            shop_id: ShopId::new(),
            shop_name: "Test Shop".to_string(),
            shop_type,
            url: Url::parse("https://example.com/product/1").unwrap(),
            last_scraped_hash: None,
            last_scraped_price: None,
            last_scraped_price_estimate_min: None,
            last_scraped_price_estimate_max: None,
            last_scraped_url: None,
            last_scraped_images_hash: None,
            last_scraped_auction_start: None,
            last_scraped_auction_end: None,
            last_scraped_state: None,
        }
    }

    fn make_product(candidate: &ScraperCandidate) -> NormalizedProduct {
        NormalizedProduct {
            shops_product_id: ShopsProductId::from("prod-1".to_string()),
            title: Localized::new(Language::De, Title::from("Ein Schrank")),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            seller_name: None,
            state: ProductState::Available,
            url: candidate.url.clone(),
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    #[test]
    fn should_map_commercial_dealer_product_to_upsert_command() {
        let candidate = make_candidate(ShopType::CommercialDealer);
        let product = make_product(&candidate);

        let cmd = normalize_to_upsert(product, &candidate)
            .expect("should produce a command for CommercialDealer");

        assert_eq!(cmd.shop_id, candidate.shop_id);
        assert_eq!(cmd.seller_name_raw, None);
        assert_eq!(cmd.state, Some(ProductState::Available));
        assert_eq!(
            cmd.url.as_ref().map(|u| u.as_str()),
            Some("https://example.com/product/1")
        );
    }

    #[test]
    fn should_map_auction_house_product_to_upsert_command() {
        let candidate = make_candidate(ShopType::AuctionHouse);
        let product = make_product(&candidate);

        let cmd = normalize_to_upsert(product, &candidate)
            .expect("should produce a command for AuctionHouse");

        assert_eq!(cmd.shop_id, candidate.shop_id);
        assert_eq!(cmd.seller_name_raw, None);
    }

    #[test]
    fn should_map_marketplace_product_when_seller_name_is_present() {
        let candidate = make_candidate(ShopType::Marketplace);
        let mut product = make_product(&candidate);
        product.seller_name = Some("Marketplace Seller".to_string());

        let cmd = normalize_to_upsert(product, &candidate)
            .expect("should produce a command for Marketplace when seller is known");

        assert_eq!(cmd.seller_name_raw.as_deref(), Some("Marketplace Seller"));
    }

    #[test]
    fn should_map_auction_platform_product_when_seller_name_is_present() {
        let candidate = make_candidate(ShopType::AuctionPlatform);
        let mut product = make_product(&candidate);
        product.seller_name = Some("Auction Platform Seller".to_string());

        let cmd = normalize_to_upsert(product, &candidate)
            .expect("should produce a command for AuctionPlatform when seller is known");

        assert_eq!(
            cmd.seller_name_raw.as_deref(),
            Some("Auction Platform Seller")
        );
    }

    #[test]
    fn should_map_marketplace_product_when_seller_name_is_missing() {
        let candidate = make_candidate(ShopType::Marketplace);
        let product = make_product(&candidate);
        let cmd = normalize_to_upsert(product, &candidate)
            .expect("should produce a command for Marketplace without seller_name");
        assert_eq!(cmd.seller_name_raw, None);
    }

    #[test]
    fn should_map_auction_platform_product_when_seller_name_is_blank() {
        let candidate = make_candidate(ShopType::AuctionPlatform);
        let mut product = make_product(&candidate);
        product.seller_name = Some("   ".to_string());
        let cmd = normalize_to_upsert(product, &candidate)
            .expect("should produce a command for AuctionPlatform with blank seller_name");
        assert_eq!(cmd.seller_name_raw, None);
    }

    fn temp_output_path(suffix: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("product_push_test_{suffix}.json"))
    }

    #[tokio::test]
    async fn file_push_service_creates_output_file() {
        let path = temp_output_path("creates");
        let _ = std::fs::remove_file(&path); // clean up from any previous run

        let service = FileProductPushService::new(path.clone());

        let candidate = make_candidate(ShopType::CommercialDealer);
        let product = make_product(&candidate);
        let cmd = normalize_to_upsert(product, &candidate).unwrap();

        service.push(vec![cmd]).await;

        assert!(path.exists(), "output file should be created");
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.len(), 1);
        // Derived identity fields are intentionally omitted from the command snapshot.
        assert!(parsed[0].get("seller_name_raw").is_some());
        assert_eq!(parsed[0]["state"], "AVAILABLE");
        // Rich product fields are present in the output
        assert!(
            parsed[0].get("title").is_some(),
            "title should be serialised"
        );
        assert!(
            parsed[0].get("images").is_some(),
            "images should be serialised"
        );
        assert!(
            parsed[0].get("price").is_some(),
            "price key should be present"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn file_push_service_appends_on_subsequent_calls() {
        let path = temp_output_path("appends");
        let _ = std::fs::remove_file(&path);

        let service = FileProductPushService::new(path.clone());

        let candidate = make_candidate(ShopType::CommercialDealer);
        let product1 = make_product(&candidate);
        let cmd1 = normalize_to_upsert(product1, &candidate).unwrap();
        service.push(vec![cmd1]).await;

        let product2 = make_product(&candidate);
        let cmd2 = normalize_to_upsert(product2, &candidate).unwrap();
        service.push(vec![cmd2]).await;

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.len(), 2, "both commands should be in the file");

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn file_push_service_noop_on_empty_commands() {
        let path = temp_output_path("noop");
        let _ = std::fs::remove_file(&path);

        let service = FileProductPushService::new(path.clone());
        service.push(vec![]).await;

        assert!(!path.exists(), "file should not be created for empty push");
    }

    #[tokio::test]
    async fn production_push_service_logs_failures() {
        let mut mock = product::service::command_service::MockCommandProductService::new();

        let candidate = make_candidate(ShopType::CommercialDealer);
        let product = make_product(&candidate);
        let cmd = normalize_to_upsert(product, &candidate).unwrap();
        let cmd_clone = cmd.clone();

        mock.expect_upsert().times(1).returning(move |cmds| {
            let failures = cmds.clone();
            Box::pin(async move { failures })
        });

        let service = ProductPushServiceImpl::new(Box::new(mock));
        service.push(vec![cmd_clone]).await;
        // No panic = pass; failures are logged as warnings, not propagated.
    }

    #[tokio::test]
    async fn production_push_service_succeeds_when_no_failures() {
        let mut mock = product::service::command_service::MockCommandProductService::new();

        mock.expect_upsert()
            .times(1)
            .returning(|_| Box::pin(async { vec![] }));

        let candidate = make_candidate(ShopType::CommercialDealer);
        let product = make_product(&candidate);
        let cmd = normalize_to_upsert(product, &candidate).unwrap();

        let service = ProductPushServiceImpl::new(Box::new(mock));
        service.push(vec![cmd]).await;
    }
}
