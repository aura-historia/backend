//! Service for pushing scraped products to the product backend.
//!
//! # Overview
//!
//! After a product URL is scraped and normalised into a [`NormalizedProduct`], it must be
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
use common::shop_name::ShopName;
use product::core::authenticity::Authenticity;
use product::core::condition::Condition;
use product::core::provenance::Provenance;
use product::core::restoration::Restoration;
use product::service::command_service::CommandProductService;
use product::service::product_command::UpsertProductCommand;
use shop::core::shop_type::ShopType;
use tracing::{error, warn};

use crate::scraper::candidate_service::ScraperCandidate;
use crate::scraper::normalization::product::NormalizedProduct;

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Accepts a batch of [`UpsertProductCommand`]s and forwards them to the backend.
///
/// Failures (commands that could not be persisted) are logged but not propagated —
/// the crawler is designed to retry on the next scraping cycle.
#[async_trait]
#[mockall::automock]
pub trait ProductPushService: Send + Sync {
    async fn push(&self, commands: Vec<UpsertProductCommand>);
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
    async fn push(&self, commands: Vec<UpsertProductCommand>) {
        let count = commands.len();
        let failures = self.command_service.upsert(commands).await;
        if !failures.is_empty() {
            warn!(
                failures = failures.len(),
                total = count,
                "Some products failed to upsert"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// File-based implementation (demo / dev)
// ---------------------------------------------------------------------------

/// Writes upserted commands as pretty-printed JSON to `scraped_products.json`.
///
/// This is used by the demo binary where no DynamoDB/AWS is available.
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

/// Serialisable snapshot of a single upsert command, used only for demo output.
#[derive(serde::Serialize, serde::Deserialize)]
struct UpsertCommandSnapshot {
    shop_id: String,
    seller_id: String,
    shops_product_id: String,
    shop_name: String,
    seller_name: String,
    shop_type: String,
    url: Option<String>,
    state: Option<String>,
}

impl From<&UpsertProductCommand> for UpsertCommandSnapshot {
    fn from(cmd: &UpsertProductCommand) -> Self {
        let shop_id_uuid: uuid::Uuid = cmd.shop_id.into();
        let seller_id_uuid: uuid::Uuid = cmd.seller_id.into();
        Self {
            shop_id: shop_id_uuid.to_string(),
            seller_id: seller_id_uuid.to_string(),
            shops_product_id: cmd.shops_product_id.to_string(),
            shop_name: cmd.shop_name.as_ref().to_string(),
            seller_name: cmd.seller_name.as_ref().to_string(),
            shop_type: format!("{:?}", cmd.shop_type),
            url: cmd.url.as_ref().map(|u| u.to_string()),
            state: cmd.state.as_ref().map(|s| format!("{s:?}")),
        }
    }
}

#[async_trait]
impl ProductPushService for FileProductPushService {
    async fn push(&self, commands: Vec<UpsertProductCommand>) {
        if commands.is_empty() {
            return;
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
                    error!(error = %e, path = %self.output_path.display(), "Failed to write scraped_products.json");
                } else {
                    tracing::info!(
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
    }
}

// ---------------------------------------------------------------------------
// Mapping helper
// ---------------------------------------------------------------------------

/// Maps a [`NormalizedProduct`] together with metadata from its [`ScraperCandidate`] into
/// an [`UpsertProductCommand`].
///
/// # Seller resolution
///
/// For [`ShopType::CommercialDealer`] shops `seller_id == shop_id` and
/// `seller_name == shop_name`. Marketplace / auction-platform seller resolution is out of scope
/// for this implementation.
pub fn normalize_to_upsert(
    product: NormalizedProduct,
    candidate: &ScraperCandidate,
) -> Option<UpsertProductCommand> {
    // Only handle non-marketplace, non-auction-platform shops for now.
    match candidate.shop_type {
        ShopType::CommercialDealer | ShopType::AuctionHouse => {}
        ShopType::Marketplace | ShopType::AuctionPlatform => {
            warn!(
                shop_id = %candidate.shop_id,
                shop_type = ?candidate.shop_type,
                "Skipping product push for marketplace/auction-platform shop — seller resolution not yet implemented"
            );
            return None;
        }
    }

    let shop_name: ShopName = ShopName::from(candidate.shop_name.as_str());
    let seller_name = shop_name.clone();
    let seller_id = candidate.shop_id;

    Some(UpsertProductCommand {
        shop_id: candidate.shop_id,
        seller_id,
        shops_product_id: product.shops_product_id,
        shop_name,
        seller_name,
        shop_type: candidate.shop_type,
        native_title: Some(product.title),
        native_description: product.description,
        native_price: product.price,
        native_price_estimate_min: product.price_estimate_min,
        native_price_estimate_max: product.price_estimate_max,
        state: Some(product.state),
        url: Some(product.url),
        images: product.images,
        auction_start: product.auction_start,
        auction_end: product.auction_end,
        origin_year: None,
        authenticity: Authenticity::default(),
        condition: Condition::default(),
        provenance: Provenance::default(),
        restoration: Restoration::default(),
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
            main_hash: "abc".to_string(),
            last_scraped_hash: None,
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
        assert_eq!(cmd.seller_id, candidate.shop_id); // seller_id == shop_id
        assert_eq!(cmd.shop_name.as_ref(), "Test Shop");
        assert_eq!(cmd.seller_name.as_ref(), "Test Shop"); // seller_name == shop_name
        assert_eq!(cmd.shop_type, ShopType::CommercialDealer);
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
        assert_eq!(cmd.shop_type, ShopType::AuctionHouse);
    }

    #[test]
    fn should_skip_marketplace_products() {
        let candidate = make_candidate(ShopType::Marketplace);
        let product = make_product(&candidate);
        let result = normalize_to_upsert(product, &candidate);
        assert!(result.is_none(), "Marketplace products should be skipped");
    }

    #[test]
    fn should_skip_auction_platform_products() {
        let candidate = make_candidate(ShopType::AuctionPlatform);
        let product = make_product(&candidate);
        let result = normalize_to_upsert(product, &candidate);
        assert!(
            result.is_none(),
            "AuctionPlatform products should be skipped"
        );
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
        assert_eq!(parsed[0]["shop_name"], "Test Shop");

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
