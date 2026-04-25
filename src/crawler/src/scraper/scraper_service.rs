use crate::network::policy::{
    NetworkAction, NetworkErrorKind, RetryPolicy, action_for, backoff_delay,
    classify_reqwest_error, retry_cooldown_for,
};
use crate::scraper::candidate_service::{ProductSnapshot, ScraperCandidateService};
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct, ShopsProductSchema,
};
use crate::scraper::css_selector::product_schema_service::{
    ProductSchemaService, ProductSchemaServiceError,
};
use crate::spider::classification::url_metadata::UrlState;

use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::ProductNormalizationService;
use common::shop_id::ShopId;
use scraper::Html;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::sleep;
use tracing::{debug, info, warn};
use url::Url;

const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/135.0.0.0 Safari/537.36";
pub const DEFAULT_SCHEMA_SEED_PAGES: usize = 10;
const MAX_SCHEMA_VARIANTS_PER_SHOP: usize = 5;

// ---------------------------------------------------------------------------
// HtmlFetcher trait — abstracted so it can be mocked in unit tests
// ---------------------------------------------------------------------------

/// Fetches raw HTML from a URL.  The real implementation delegates to
/// [`reqwest::Client`]; tests inject a fake.
#[async_trait::async_trait]
#[mockall::automock]
pub trait HtmlFetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<String, FetchError>;
}

// ---------------------------------------------------------------------------
// Error and reqwest-backed HtmlFetcher
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, thiserror::Error)]
pub enum FetchError {
    #[error("network failure: kind={kind:?}, details={details}")]
    Network {
        kind: NetworkErrorKind,
        details: String,
    },
}

impl FetchError {
    fn kind(&self) -> NetworkErrorKind {
        match self {
            FetchError::Network { kind, .. } => *kind,
        }
    }
}

pub struct ReqwestHtmlFetcher {
    client: reqwest::Client,
    retry_policy: RetryPolicy,
}

impl Default for ReqwestHtmlFetcher {
    fn default() -> Self {
        Self::new()
    }
}

impl ReqwestHtmlFetcher {
    pub fn new() -> Self {
        Self::with_retry_policy(RetryPolicy::default())
    }

    pub fn with_retry_policy(retry_policy: RetryPolicy) -> Self {
        let mut default_headers = reqwest::header::HeaderMap::new();
        default_headers.insert(
            reqwest::header::ACCEPT,
            reqwest::header::HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            ),
        );
        default_headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            reqwest::header::HeaderValue::from_static("de-DE,de;q=0.9,en-US;q=0.8,en;q=0.7"),
        );
        default_headers.insert(
            reqwest::header::CACHE_CONTROL,
            reqwest::header::HeaderValue::from_static("no-cache"),
        );
        default_headers.insert(
            reqwest::header::PRAGMA,
            reqwest::header::HeaderValue::from_static("no-cache"),
        );

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .timeout(std::time::Duration::from_secs(15))
            .user_agent(DEFAULT_USER_AGENT)
            .default_headers(default_headers)
            .build()
            .expect("reqwest client should build");

        Self {
            client,
            retry_policy,
        }
    }

    async fn fetch_once(&self, url: &Url) -> Result<String, FetchError> {
        let response =
            self.client
                .get(url.clone())
                .send()
                .await
                .map_err(|err| FetchError::Network {
                    kind: classify_reqwest_error(&err),
                    details: err.to_string(),
                })?;

        let response = response
            .error_for_status()
            .map_err(|err| FetchError::Network {
                kind: classify_reqwest_error(&err),
                details: err.to_string(),
            })?;

        response.text().await.map_err(|err| FetchError::Network {
            kind: classify_reqwest_error(&err),
            details: err.to_string(),
        })
    }
}

#[async_trait::async_trait]
impl HtmlFetcher for ReqwestHtmlFetcher {
    async fn fetch(&self, url: &Url) -> Result<String, FetchError> {
        for attempt in 1..=self.retry_policy.max_attempts {
            match self.fetch_once(url).await {
                Ok(html) => return Ok(html),
                Err(err) => {
                    let action = action_for(err.kind());
                    if action != NetworkAction::Retry || attempt >= self.retry_policy.max_attempts {
                        return Err(err);
                    }
                    // For status codes with a dedicated cooldown (e.g. 429 → 1 sec),
                    // use that cooldown as the retry delay so the policy's max_delay
                    // cap doesn't truncate it to a much smaller value.
                    let cooldown = retry_cooldown_for(err.kind());
                    let backoff = backoff_delay(self.retry_policy, attempt);
                    let delay = cooldown.max(backoff);
                    if !delay.is_zero() {
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(FetchError::Network {
            kind: NetworkErrorKind::Unknown,
            details: "retry loop terminated unexpectedly".to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ScraperError {
    #[error("HTTP error while fetching '{url}': {details}")]
    HttpError {
        url: Url,
        kind: NetworkErrorKind,
        details: String,
    },

    #[error("Product URL removed while fetching '{url}': {details}")]
    ProductRemoved { url: Url, details: String },

    #[error("URL has no host: {url}")]
    NoHost { url: Url },

    #[error("Schema service error: {0}")]
    SchemaServiceError(#[from] ProductSchemaServiceError),

    #[error(
        "Schema regeneration exhausted after {attempts} attempts for '{url}' (last error: {last_error})"
    )]
    SchemaRegenerationExhausted {
        url: Url,
        attempts: u32,
        last_error: ApplySchemaError,
    },

    #[error("Normalization error: {0}")]
    NormalizationError(#[from] NormalizationError),
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

/// Result of a successful scrape — the normalized product together with the
/// metadata needed to mark the URL as scraped *after* the push has been
/// confirmed.
#[derive(Debug)]
pub struct ScrapedProduct {
    pub product: NormalizedProduct,
    /// SHA-256 of the page's `<main>` fragment (or full HTML) that was used to
    /// detect whether the page had changed.
    pub hash: String,
    /// Snapshot of the normalized product's tracked fields, serialized to the
    /// same TEXT representation used in the database.
    pub snapshot: ProductSnapshot,
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ScraperService: Send + Sync {
    /// Fetch the product page at `url`, extract structured data using the CSS
    /// selector schema for `shop_id`, normalise the raw data, and return a
    /// [`ScrapedProduct`].  The caller is responsible for calling
    /// [`ScraperCandidateService::mark_as_scraped`] once the product has been
    /// successfully pushed to the backend.
    async fn scrape(
        &self,
        shop_id: &ShopId,
        url: &Url,
        last_scraped_hash: Option<&str>,
    ) -> Result<Option<ScrapedProduct>, ScraperError>;
}

// ---------------------------------------------------------------------------
// ScraperServiceImpl
// ---------------------------------------------------------------------------

pub struct ScraperServiceImpl {
    html_fetcher: Box<dyn HtmlFetcher>,
    schema_service: Box<dyn ProductSchemaService + Send + Sync>,
    normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
    candidate_service: Arc<dyn ScraperCandidateService>,
    /// Maximum number of regenerate-and-retry attempts when no cached schema
    /// variant applies. This keeps the old config slot semantics while removing
    /// fix-mode behavior.
    max_schema_fix_attempts: u32,
    /// Number of HTML pages to seed first-time schema generation with.
    /// `1` means current page only; values >1 trigger best-effort sampling/fetch.
    schema_seed_pages: usize,
}

impl ScraperServiceImpl {
    pub fn new(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        max_schema_fix_attempts: u32,
    ) -> Self {
        Self::new_with_schema_seed_pages(
            html_fetcher,
            schema_service,
            normalization_service,
            candidate_service,
            max_schema_fix_attempts,
            DEFAULT_SCHEMA_SEED_PAGES,
        )
    }

    pub fn new_with_schema_seed_pages(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        max_schema_fix_attempts: u32,
        schema_seed_pages: usize,
    ) -> Self {
        Self {
            html_fetcher,
            schema_service,
            normalization_service,
            candidate_service,
            max_schema_fix_attempts,
            schema_seed_pages: schema_seed_pages.max(1),
        }
    }
}

impl ScraperServiceImpl {
    async fn mark_product_removed_best_effort(&self, shop_id: &ShopId, url: &Url) {
        if let Err(err) = self
            .candidate_service
            .set_state(shop_id, url, UrlState::Removed)
            .await
        {
            warn!(error = %err, url = %url, "Failed to mark product as REMOVED");
        }
    }

    async fn persist_scraped_state_best_effort(
        &self,
        shop_id: &ShopId,
        url: &Url,
        state: UrlState,
    ) {
        if let Err(err) = self.candidate_service.set_state(shop_id, url, state).await {
            warn!(
                error = %err,
                url = %url,
                state = %state,
                "Failed to persist scraped URL state"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Extracted helper methods for the scrape pipeline
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    async fn collect_schema_seed_pages(
        &self,
        shop_id: &ShopId,
        url: &Url,
        primary_html: &str,
    ) -> Vec<String> {
        let mut pages = vec![primary_html.to_string()];
        if self.schema_seed_pages <= 1 {
            return pages;
        }

        let extra_limit = (self.schema_seed_pages - 1) as i64;
        let sample_urls = match self
            .candidate_service
            .get_random_product_urls_for_schema_seed(shop_id, url, extra_limit)
            .await
        {
            Ok(urls) => urls,
            Err(err) => {
                warn!(
                    error = %err,
                    shop_id = %shop_id,
                    url = %url,
                    "Failed to load random schema-seed URLs; falling back to current page only"
                );
                return pages;
            }
        };

        // Keep this exclusion keying aligned with the DB query in
        // `get_random_product_urls_for_schema_seed`: both currently operate on
        // raw URL strings. If URL canonicalization is introduced, update both
        // places together to avoid duplicate samples slipping through.
        let mut seen_urls = HashSet::new();
        seen_urls.insert(url.as_str().to_string());
        for sample_url in sample_urls {
            if pages.len() >= self.schema_seed_pages {
                break;
            }
            let sample_url_key = sample_url.as_str().to_string();
            if !seen_urls.insert(sample_url_key) {
                continue;
            }
            match self.html_fetcher.fetch(&sample_url).await {
                Ok(sample_html) => pages.push(sample_html),
                Err(err) => {
                    warn!(
                        error = %err,
                        sample_url = %sample_url,
                        "Failed to fetch sampled schema-seed page; continuing with available samples"
                    );
                }
            }
        }

        pages
    }

    /// Obtains product CSS selector schemas for `shop_id`, loading them from
    /// the DB or generating them via the LLM if they do not yet exist.
    ///
    /// The dispatcher guarantees at most one in-flight scrape per domain at a
    /// time, so no additional locking is required here.
    async fn obtain_schemas(
        &self,
        shop_id: &ShopId,
        domain: &str,
        url: &Url,
        html: &str,
    ) -> Result<ShopsProductSchema, ScraperError> {
        debug!(domain, url = %url, "Obtaining product CSS selector schemas");
        if let Some(existing) = self.schema_service.find_product_schema(shop_id).await? {
            debug!(domain, url = %url, "Schema found in DB");
            Ok(existing)
        } else {
            let seed_pages = self.collect_schema_seed_pages(shop_id, url, html).await;
            #[cfg(not(test))]
            if let Err(err) = self
                .candidate_service
                .increment_shop_llm_calls(shop_id, 1)
                .await
            {
                warn!(shop_id = %shop_id, error = %err, "Failed to increment shop LLM call counter");
            }
            let schemas = self
                .schema_service
                .create_product_schemas(&seed_pages)
                .await?;
            Ok(self
                .schema_service
                .save_product_schemas(shop_id, domain, schemas)
                .await?)
        }
    }

    /// Applies `schema` to `html` synchronously (scraper::Html is !Send).
    fn apply_schema(
        schema: &ProductCssSelectorSchema,
        html: &str,
    ) -> Result<RawExtractedProduct, ApplySchemaError> {
        let parsed_html = Html::parse_document(html);
        schema.apply(&parsed_html)
    }

    fn try_apply_schemas(
        schemas: &[ProductCssSelectorSchema],
        html: &str,
    ) -> Result<(ProductCssSelectorSchema, RawExtractedProduct), ApplySchemaError> {
        let mut last_error: Option<ApplySchemaError> = None;
        for schema in schemas {
            match Self::apply_schema(schema, html) {
                Ok(raw) => return Ok((schema.clone(), raw)),
                Err(err) => {
                    last_error = Some(err);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            ApplySchemaError::Title(ExtractionError::NoElementMatched {
                selector: "title".to_string(),
            })
        }))
    }

    async fn append_and_reapply_with_retry(
        &self,
        shop_id: &ShopId,
        domain: &str,
        url: &Url,
        html: &str,
    ) -> Result<(ProductCssSelectorSchema, RawExtractedProduct), ScraperError> {
        let attempts = self.max_schema_fix_attempts.max(1);
        let mut last_error: Option<ApplySchemaError> = None;

        for attempt in 1..=attempts {
            #[cfg(not(test))]
            if let Err(err) = self
                .candidate_service
                .increment_shop_llm_calls(shop_id, 1)
                .await
            {
                warn!(shop_id = %shop_id, error = %err, "Failed to increment shop LLM call counter");
            }

            let candidate = self
                .schema_service
                .append_single_schema(shop_id, domain, html)
                .await?;

            match Self::try_apply_schemas(&candidate.product_schemas, html) {
                Ok((selected_schema, raw)) => {
                    let persisted_schemas = Self::dedupe_and_cap_schemas(candidate.product_schemas);
                    self.schema_service
                        .save_product_schemas(shop_id, domain, persisted_schemas)
                        .await?;
                    info!(domain, url = %url, attempt, "Generated schema appended and applied");
                    return Ok((selected_schema, raw));
                }
                Err(err) => {
                    last_error = Some(err.clone());
                    warn!(
                        domain,
                        url = %url,
                        attempt,
                        max_attempts = attempts,
                        error = %err,
                        "Generated schema did not apply; discarding and retrying"
                    );
                }
            }
        }

        Err(ScraperError::SchemaRegenerationExhausted {
            url: url.clone(),
            attempts,
            last_error: last_error.unwrap_or_else(|| {
                ApplySchemaError::Title(ExtractionError::NoElementMatched {
                    selector: "title".to_string(),
                })
            }),
        })
    }

    fn dedupe_and_cap_schemas(
        schemas: Vec<ProductCssSelectorSchema>,
    ) -> Vec<ProductCssSelectorSchema> {
        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::with_capacity(schemas.len());

        for schema in schemas {
            let key = match serde_json::to_string(&schema) {
                Ok(serialized) => serialized,
                Err(_) => {
                    deduped.push(schema);
                    continue;
                }
            };
            if seen.insert(key) {
                deduped.push(schema);
            }
        }

        if deduped.len() <= MAX_SCHEMA_VARIANTS_PER_SHOP {
            return deduped;
        }

        // Keep the newest variants (append path adds new candidates at the end).
        let start = deduped.len() - MAX_SCHEMA_VARIANTS_PER_SHOP;
        deduped.into_iter().skip(start).collect()
    }
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    async fn scrape(
        &self,
        shop_id: &ShopId,
        url: &Url,
        last_scraped_hash: Option<&str>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let domain = url
            .host_str()
            .ok_or_else(|| ScraperError::NoHost { url: url.clone() })?;

        // 1. Fetch HTML --------------------------------------------------
        debug!(domain, url = %url, "Fetching product page HTML");
        let html = match self.html_fetcher.fetch(url).await {
            Ok(html) => html,
            Err(FetchError::Network {
                kind: NetworkErrorKind::HttpStatus(404 | 410),
                details,
            }) => {
                self.mark_product_removed_best_effort(shop_id, url).await;
                return Err(ScraperError::ProductRemoved {
                    url: url.clone(),
                    details,
                });
            }
            Err(FetchError::Network { kind, details }) => {
                return Err(ScraperError::HttpError {
                    url: url.clone(),
                    kind,
                    details,
                });
            }
        };

        let has_main = extract_main_fragment(&html).is_some();
        let current_hash = hash_main_fragment(&html).unwrap_or_else(|| hash_html(&html));

        if has_main && last_scraped_hash == Some(current_hash.as_str()) {
            debug!(url = %url, "Hash matches last scraped hash, skipping extraction.");
            if let Err(e) = self
                .candidate_service
                .touch_scraped(shop_id, url, &current_hash)
                .await
            {
                warn!(error = %e, "Failed to touch url as scraped after hash-match skip");
            }
            return Ok(None);
        }

        // 2. Obtain schemas (from DB or freshly created by LLM) -----------
        let shops_product_schema = self.obtain_schemas(shop_id, domain, url, &html).await?;

        // 3. Apply one schema that fits this page -------------------------
        let (selected_schema, raw) =
            match Self::try_apply_schemas(&shops_product_schema.product_schemas, &html) {
                Ok(result) => result,
                Err(err) => {
                    warn!(
                        domain,
                        url = %url,
                        schemas = shops_product_schema.product_schemas.len(),
                        error = %err,
                        "No cached schema applied; generating new schema candidates"
                    );
                    self.append_and_reapply_with_retry(shop_id, domain, url, &html)
                        .await?
                }
            };

        {
            debug!(
                domain,
                url = %url,
                shops_product_id = %raw.shops_product_id,
                title = %raw.title,
                state = %raw.state,
                price = ?raw.price,
                price_estimate_min = ?raw.price_estimate_min,
                price_estimate_max = ?raw.price_estimate_max,
                images_count = raw.images.len(),
                has_description = !raw.description.is_empty(),
                has_auction_start = raw.auction_start.is_some(),
                has_auction_end = raw.auction_end.is_some(),
                "Schema applied successfully"
            );
        }

        // 4. Normalise --------------------------------------------------
        debug!(domain, url = %url, "Normalizing extracted product data");
        let final_product = self
            .normalization_service
            .normalize(
                raw,
                url.clone(),
                selected_schema
                    .default_currency
                    .map(common::currency::domain::Currency::from),
            )
            .await?;

        // 5. Bookkeeping ------------------------------------------------
        self.persist_scraped_state_best_effort(shop_id, url, UrlState::from(final_product.state))
            .await;

        // `mark_as_scraped` is intentionally NOT called here.  The caller
        // (cron pipeline) must call it only after the push to the product
        // backend has been confirmed, so that a failed push is retried on
        // the next cycle.
        let snapshot = ProductSnapshot::from_normalized(&final_product);

        debug!(
            domain,
            shops_product_id = %final_product.shops_product_id,
            url = %url,
            "Scraping complete"
        );
        Ok(Some(ScrapedProduct {
            product: final_product,
            hash: current_hash,
            snapshot,
        }))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn hash_main_fragment(html: &str) -> Option<String> {
    let content = extract_main_fragment(html)?;
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn hash_html(html: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(html.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn find_case_insensitive(text: &str, search: &str) -> Option<usize> {
    let search_bytes = search.as_bytes();
    if search_bytes.is_empty() {
        return Some(0);
    }
    text.as_bytes()
        .windows(search_bytes.len())
        .position(|window| window.eq_ignore_ascii_case(search_bytes))
}

fn extract_main_fragment(html: &str) -> Option<&str> {
    let main_start = find_case_insensitive(html, "<main")?;
    let tag_end_rel = html[main_start..].find('>')?;
    let content_start = main_start + tag_end_rel + 1;
    let main_end_rel = find_case_insensitive(&html[content_start..], "</main>")?;
    let content_end = content_start + main_end_rel;
    Some(&html[content_start..content_end])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::css_selector::product_schema::{
        ProductCssSelectorSchema, ShopsProductSchema,
    };
    use crate::scraper::css_selector::product_schema_service::MockProductSchemaService;
    use crate::scraper::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::scraper::normalization::product_normalization_service::MockProductNormalizationService;
    use common::language::domain::Language;
    use common::localized::Localized;
    use common::product_state::domain::ProductState;
    use common::shop_id::ShopId;
    use common::shops_product_id::ShopsProductId;
    use product::core::title::Title;
    use time::OffsetDateTime;
    use url::Url;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn shop_id() -> ShopId {
        ShopId::new()
    }

    fn product_url() -> Url {
        Url::parse("https://example.com/products/123").unwrap()
    }

    fn sample_html() -> String {
        r#"<!DOCTYPE html>
        <html>
        <body>
          <main>
            <span id="product-id">SKU-42</span>
            <h1>Biedermeier Chair</h1>
            <span id="state">In Stock</span>
            <img src="/images/chair.jpg">
          </main>
        </body>
        </html>"#
            .to_string()
    }

    fn minimal_schema() -> ProductCssSelectorSchema {
        let text_rule = |selector: &str| ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        };
        let attr_rule_all = |selector: &str, attr: &str| ExtractionRule {
            selector: CssSelector::from(selector),
            additional_selectors: vec![],
            extract: ExtractionKind::Attribute { name: attr.into() },
            cardinality: ExtractionCardinality::All,
        };

        ProductCssSelectorSchema {
            shops_product_id: text_rule("#product-id"),
            title: text_rule("h1"),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: text_rule("#state"),
            images: attr_rule_all("img", "src"),
            auction_start: None,
            auction_end: None,
            default_currency: None,
        }
    }

    fn shops_product_schema(shop_id: ShopId) -> ShopsProductSchema {
        let schema = minimal_schema();
        ShopsProductSchema {
            shop_id,
            product_schema: schema.clone(),
            product_schemas: vec![schema],
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    fn normalized_product(url: Url) -> NormalizedProduct {
        let title: Title = "Biedermeier Chair".into();
        NormalizedProduct {
            shops_product_id: ShopsProductId::from("SKU-42"),
            title: Localized::new(Language::De, title),
            description: None,
            price: None,
            price_estimate_min: None,
            price_estimate_max: None,
            state: ProductState::Available,
            url,
            images: vec![],
            auction_start: None,
            auction_end: None,
        }
    }

    fn expect_successful_bookkeeping(
        cand_svc: &mut MockScraperCandidateService,
        shop_id: ShopId,
        url: Url,
        state: UrlState,
    ) {
        let url_for_set_state = url.clone();
        cand_svc
            .expect_set_state()
            .once()
            .withf(move |received_shop_id, received_url, received_state| {
                *received_shop_id == shop_id
                    && received_url == &url_for_set_state
                    && *received_state == state
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));
        // `mark_as_scraped` is no longer called inside `scrape()` — it is
        // called by the cron pipeline after a successful push.
    }

    // -----------------------------------------------------------------------
    // Happy path
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_normalized_product_when_schema_exists_and_applies_cleanly() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .once()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let schema_for_create = schema.product_schemas.first().cloned().unwrap();
        let schema_for_save = schema.clone();
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        schema_svc
            .expect_create_product_schemas()
            .once()
            .returning(move |_| {
                let s = vec![schema_for_create.clone()];
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_save.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .once()
            .returning(move |_, _, _| {
                let n = expected.clone();
                Box::pin(async move { Ok(n) })
            });

        let mut cand_svc = MockScraperCandidateService::new();
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            1,
        );

        let result = service.scrape(&id, &url, None).await.unwrap().unwrap();

        assert_eq!(
            result.product.shops_product_id,
            ShopsProductId::from("SKU-42")
        );
        assert_eq!(result.product.state, ProductState::Available);
        assert_eq!(result.product.url, url);
    }

    #[tokio::test]
    async fn should_return_normalized_product_with_all_fields_when_normalization_produces_full_data()
     {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let schema_for_create = schema.product_schemas.first().cloned().unwrap();
        let schema_for_save = schema.clone();
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        schema_svc
            .expect_create_product_schemas()
            .once()
            .returning(move |_| {
                let s = vec![schema_for_create.clone()];
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_save.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let norm_clone = norm.clone();
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(move |_, _, _| {
            let n = norm_clone.clone();
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            1,
        );

        let result = service.scrape(&id, &url, None).await.unwrap().unwrap();

        assert_eq!(result.product, norm);
    }

    #[tokio::test]
    async fn should_seed_schema_generation_with_additional_sample_pages_on_cache_miss() {
        let id = shop_id();
        let url = product_url();
        let primary_html = sample_html();

        let mut fetcher = MockHtmlFetcher::new();
        let primary_html_for_fetch = primary_html.clone();
        fetcher.expect_fetch().once().returning(move |_| {
            let html = primary_html_for_fetch.clone();
            Box::pin(async move { Ok(html) })
        });

        let initial_schema = {
            // Create a schema that won't match the sample_html
            let text_rule = |selector: &str| ExtractionRule {
                selector: CssSelector::from(selector),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            };
            let attr_rule_all = |selector: &str, attr: &str| ExtractionRule {
                selector: CssSelector::from(selector),
                additional_selectors: vec![],
                extract: ExtractionKind::Attribute { name: attr.into() },
                cardinality: ExtractionCardinality::All,
            };
            ProductCssSelectorSchema {
                shops_product_id: text_rule("non-existent-id"),
                title: text_rule("non-existent-title"),
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: text_rule("non-existent-state"),
                images: attr_rule_all("img", "src"),
                auction_start: None,
                auction_end: None,
                default_currency: None,
            }
        };

        let schema = shops_product_schema(id);
        let final_schema_for_append = schema.clone();

        let mut schema_svc = MockProductSchemaService::new();
        let initial_schema_for_find = initial_schema.clone();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(move |_| {
                let s = ShopsProductSchema {
                    shop_id: shop_id(),
                    product_schema: initial_schema_for_find.clone(),
                    product_schemas: vec![initial_schema_for_find.clone()],
                    created: OffsetDateTime::now_utc(),
                    updated: OffsetDateTime::now_utc(),
                };
                Box::pin(async move { Ok(Some(s)) })
            });
        schema_svc
            .expect_append_single_schema()
            .once()
            .returning(move |_, _, _| {
                let s = final_schema_for_append.clone();
                Box::pin(async move { Ok(s) })
            });
        let schema_for_persist = schema.clone();
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_persist.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .once()
            .returning(move |_, _, _| {
                let n = expected.clone();
                Box::pin(async move { Ok(n) })
            });

        let mut cand_svc = MockScraperCandidateService::new();
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            3,
        );

        let result = service.scrape(&id, &url, None).await.unwrap().unwrap();
        assert_eq!(
            result.product.shops_product_id,
            ShopsProductId::from("SKU-42")
        );
    }

    #[tokio::test]
    async fn should_fallback_to_primary_page_when_schema_seed_sampling_query_fails() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .once()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let schema_for_create = schema.product_schemas.first().cloned().unwrap();
        let schema_for_save = schema.clone();
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        schema_svc
            .expect_create_product_schemas()
            .once()
            .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
            .returning(move |_| {
                let s = vec![schema_for_create.clone()];
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_save.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .once()
            .returning(move |_, _, _| {
                let n = expected.clone();
                Box::pin(async move { Ok(n) })
            });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_get_random_product_urls_for_schema_seed()
            .once()
            .returning(|_, _, _| Box::pin(async { Err(sqlx::Error::RowNotFound) }));
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            3,
        );

        let result = service.scrape(&id, &url, None).await.unwrap().unwrap();
        assert_eq!(result.product.state, ProductState::Available);
    }

    #[tokio::test]
    async fn should_keep_primary_only_when_extra_schema_seed_fetch_fails() {
        let id = shop_id();
        let url = product_url();
        let sample_seed_url = Url::parse("https://example.com/product/seed-fail").unwrap();
        let expected_primary_url = url.clone();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .times(2)
            .returning(move |requested_url| {
                let requested_url = requested_url.clone();
                let expected_primary_url = expected_primary_url.clone();
                Box::pin(async move {
                    if requested_url == expected_primary_url {
                        Ok(sample_html())
                    } else {
                        Err(FetchError::Network {
                            kind: NetworkErrorKind::Timeout,
                            details: "timeout".to_string(),
                        })
                    }
                })
            });

        let schema = shops_product_schema(id);
        let schema_for_create = schema.product_schemas.first().cloned().unwrap();
        let schema_for_save = schema.clone();
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        schema_svc
            .expect_create_product_schemas()
            .once()
            .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
            .returning(move |_| {
                let s = vec![schema_for_create.clone()];
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_save.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .once()
            .returning(move |_, _, _| {
                let n = expected.clone();
                Box::pin(async move { Ok(n) })
            });

        let mut cand_svc = MockScraperCandidateService::new();
        let sample_seed_url_clone = sample_seed_url.clone();
        cand_svc
            .expect_get_random_product_urls_for_schema_seed()
            .once()
            .returning(move |_, _, _| {
                let sampled = vec![sample_seed_url_clone.clone()];
                Box::pin(async move { Ok(sampled) })
            });
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            3,
        );

        let result = service.scrape(&id, &url, None).await.unwrap().unwrap();
        assert_eq!(result.product.state, ProductState::Available);
    }

    #[tokio::test]
    async fn should_not_query_seed_urls_when_schema_seed_pages_is_one() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .once()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let schema_for_create = schema.product_schemas.first().cloned().unwrap();
        let schema_for_save = schema.clone();
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        schema_svc
            .expect_create_product_schemas()
            .once()
            .withf(|html_pages| html_pages.len() == 1 && html_pages[0] == sample_html())
            .returning(move |_| {
                let s = vec![schema_for_create.clone()];
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_save.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .once()
            .returning(move |_, _, _| {
                let n = expected.clone();
                Box::pin(async move { Ok(n) })
            });

        let mut cand_svc = MockScraperCandidateService::new();
        expect_successful_bookkeeping(&mut cand_svc, id, url.clone(), UrlState::Available);

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            1,
        );

        let result = service.scrape(&id, &url, None).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[tokio::test]
    async fn should_skip_fetching_and_return_none_when_hashes_match() {
        let id = shop_id();
        let url = product_url();
        let html = sample_html();
        let matching_hash = hash_main_fragment(&html).unwrap_or_else(|| hash_html(&html));

        let mut fetcher = MockHtmlFetcher::new();
        fetcher.expect_fetch().once().returning(move |_| {
            let html = html.clone();
            Box::pin(async move { Ok(html) })
        });

        let schema_svc = MockProductSchemaService::new();
        let norm_svc = MockProductNormalizationService::new();
        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_touch_scraped()
            .once()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            1,
        );

        let result = service
            .scrape(&id, &url, Some(&matching_hash))
            .await
            .unwrap();

        assert!(result.is_none());
    }

    #[test]
    fn should_hash_main_fragment_when_main_tag_exists() {
        let html = "<html><body><main><h1>Hello</h1></main></body></html>";
        let hash = hash_main_fragment(html).expect("should find <main> tag");

        let mut hasher = Sha256::new();
        hasher.update("<h1>Hello</h1>".as_bytes());
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        assert_eq!(hash, expected);
    }

    #[test]
    fn should_return_none_from_hash_main_fragment_when_main_tag_missing() {
        let html = "<html><body><section>No main</section></body></html>";
        assert!(hash_main_fragment(html).is_none());
    }

    #[test]
    fn should_hash_full_html_when_main_tag_missing() {
        let html = "<html><body><section>No main</section></body></html>";
        let hash = hash_html(html);

        let mut hasher = Sha256::new();
        hasher.update(html.as_bytes());
        let expected: String = hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();

        assert_eq!(hash, expected);
    }

    // -----------------------------------------------------------------------
    #[tokio::test]
    async fn should_persist_scraped_state_before_marking_url_as_scraped() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .once()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let schema_for_create = schema.product_schemas.first().cloned().unwrap();
        let schema_for_save = schema.clone();
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_find_product_schema()
            .once()
            .returning(|_| Box::pin(async { Ok(None) }));
        schema_svc
            .expect_create_product_schemas()
            .once()
            .returning(move |_| {
                let s = vec![schema_for_create.clone()];
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_save_product_schemas()
            .once()
            .returning(move |_, _, _| {
                let s = schema_for_save.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut expected = normalized_product(url.clone());
        expected.state = ProductState::Sold;
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .once()
            .returning(move |_, _, _| {
                let n = expected.clone();
                Box::pin(async move { Ok(n) })
            });

        // mark_as_scraped is no longer called inside scrape() — it is the
        // caller's responsibility (cron flush_batch) after push succeeds.
        let mut cand_svc = MockScraperCandidateService::new();
        let url_for_set_state = url.clone();
        cand_svc
            .expect_set_state()
            .once()
            .withf(move |received_shop_id, received_url, received_state| {
                *received_shop_id == id
                    && received_url == &url_for_set_state
                    && *received_state == UrlState::Sold
            })
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new_with_schema_seed_pages(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
            1,
        );

        let result = service.scrape(&id, &url, None).await.unwrap().unwrap();

        assert_eq!(result.product.state, ProductState::Sold);
        // The snapshot must carry the persisted state so flush_batch can call
        // mark_as_scraped with the correct data.
        assert!(!result.snapshot.state.is_empty());
    }
}
