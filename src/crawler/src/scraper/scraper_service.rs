use crate::scraper::candidate_service::ScraperCandidateService;
use crate::scraper::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use crate::scraper::css_selector::product_schema_service::{
    ProductSchemaService, ProductSchemaServiceError,
};

use crate::scraper::css_selector::rule::ExtractionError;
use crate::scraper::normalization::error::NormalizationError;
use crate::scraper::normalization::product::NormalizedProduct;
use crate::scraper::normalization::product_normalization_service::ProductNormalizationService;
use common::shop_id::ShopId;
use scraper::Html;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};
use url::Url;

// ---------------------------------------------------------------------------
// HtmlFetcher trait — abstracted so it can be mocked in unit tests
// ---------------------------------------------------------------------------

/// Fetches raw HTML from a URL.  The real implementation delegates to
/// [`reqwest::Client`]; tests inject a fake.
#[async_trait::async_trait]
#[mockall::automock]
pub trait HtmlFetcher: Send + Sync {
    async fn fetch(&self, url: &Url) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// Real HtmlFetcher backed by spider
// ---------------------------------------------------------------------------

use spider::website::Website;

#[derive(Default)]
pub struct SpiderHtmlFetcher {}

impl SpiderHtmlFetcher {
    pub fn new() -> Self {
        Self {}
    }
}

#[async_trait::async_trait]
impl HtmlFetcher for SpiderHtmlFetcher {
    async fn fetch(&self, url: &Url) -> Result<String, String> {
        let mut website = Website::new(url.as_str());

        let mut hashbrown_budget = spider::hashbrown::HashMap::new();
        hashbrown_budget.insert("*", 1);
        website.with_budget(Some(hashbrown_budget));

        let mut rx = website
            .subscribe(16)
            .ok_or("Failed to subscribe to spider channel")?;

        website.scrape().await;
        drop(website);

        // Read the page from the channel (now that scraping is done and website dropped)
        if let Ok(page) = rx.try_recv() {
            let html = page.get_html();
            if !html.is_empty() {
                return Ok(html);
            }
        }

        Err(format!("Spider could not fetch HTML for URL: {}", url))
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ScraperError {
    #[error("HTTP error while fetching '{url}': {details}")]
    HttpError { url: Url, details: String },

    #[error("Schema service error: {0}")]
    SchemaServiceError(#[from] ProductSchemaServiceError),

    /// The LLM produced a schema, the schema was applied, it failed, and the
    /// fix attempt also failed.  We surface both the original apply error and
    /// the fix error so callers have the full picture.
    #[error(
        "Schema application failed (apply: {apply_error}) and fix attempt also failed: {fix_error}"
    )]
    SchemaFixFailed {
        apply_error: ApplySchemaError,
        fix_error: ProductSchemaServiceError,
    },

    /// The maximum number of schema-fix attempts for this domain has been
    /// reached.  The domain is skipped to avoid repeated LLM calls that
    /// consistently produce non-working schemas.
    #[error("Schema fix attempts exhausted for domain '{domain}', skipping")]
    SchemaFixAttemptsExhausted { domain: String },

    #[error("Normalization error: {0}")]
    NormalizationError(#[from] NormalizationError),
}

// ---------------------------------------------------------------------------
// ScraperService trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
#[mockall::automock]
pub trait ScraperService: Send + Sync {
    /// Fetch the product page at `url`, extract structured data using the CSS
    /// selector schema for `shop_id`, normalise the raw data, and return a
    /// [`NormalizedProduct`].
    async fn scrape(
        &self,
        shop_id: &ShopId,
        url: &Url,
        current_hash: &str,
        last_scraped_hash: Option<&str>,
    ) -> Result<Option<NormalizedProduct>, ScraperError>;
}

// ---------------------------------------------------------------------------
// ScraperServiceImpl
// ---------------------------------------------------------------------------

pub struct ScraperServiceImpl {
    html_fetcher: Box<dyn HtmlFetcher>,
    schema_service: Box<dyn ProductSchemaService + Send + Sync>,
    normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
    candidate_service: Arc<dyn ScraperCandidateService>,
    /// Per-domain mutex map — ensures only one LLM schema-fix attempt runs at a
    /// time for each domain.  Concurrent URLs for the same domain queue up,
    /// then re-fetch the (hopefully now-fixed) schema from the DB before
    /// deciding whether to call the LLM again.
    schema_fix_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    /// Tracks how many times a LLM schema-fix attempt was made for each domain
    /// and resulted in a schema that still failed.  Persists across batches
    /// for the lifetime of the service.  Once a domain reaches
    /// `max_schema_fix_attempts` failed attempts it is skipped entirely.
    schema_fix_attempts: Arc<Mutex<HashMap<String, u32>>>,
    /// Maximum number of failed schema-fix attempts before a domain is skipped.
    max_schema_fix_attempts: u32,
}

impl ScraperServiceImpl {
    pub fn new(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
        candidate_service: Arc<dyn ScraperCandidateService>,
        max_schema_fix_attempts: u32,
    ) -> Self {
        Self {
            html_fetcher,
            schema_service,
            normalization_service,
            candidate_service,
            schema_fix_locks: Arc::new(Mutex::new(HashMap::new())),
            schema_fix_attempts: Arc::new(Mutex::new(HashMap::new())),
            max_schema_fix_attempts,
        }
    }
}

// ---------------------------------------------------------------------------
// Per-domain fix-attempt tracking helpers
// ---------------------------------------------------------------------------

impl ScraperServiceImpl {
    /// Atomically checks whether `domain` has budget remaining and, if so,
    /// pre-increments the counter before the LLM call is made.  Must be called
    /// while holding the per-domain fix lock so that concurrent tasks waiting
    /// for the lock will see the already-incremented counter and bail out
    /// without making a redundant LLM call.
    ///
    /// Returns `Err(SchemaFixAttemptsExhausted)` when the budget is already
    /// exhausted; otherwise increments and returns `Ok(())`.
    async fn increment_fix_attempts(&self, domain: &str) -> Result<(), ScraperError> {
        let mut map = self.schema_fix_attempts.lock().await;
        let count = map.entry(domain.to_string()).or_insert(0);
        if *count >= self.max_schema_fix_attempts {
            warn!(
                domain,
                attempts = *count,
                max = self.max_schema_fix_attempts,
                "Schema fix attempts exhausted, skipping domain"
            );
            return Err(ScraperError::SchemaFixAttemptsExhausted {
                domain: domain.to_string(),
            });
        }
        *count += 1;
        Ok(())
    }

    /// Resets the failed schema-fix attempt counter for `domain` (called when
    /// a fix attempt succeeds end-to-end including normalization).
    async fn reset_fix_attempts(&self, domain: &str) {
        let mut map = self.schema_fix_attempts.lock().await;
        map.remove(domain);
    }
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    async fn scrape(
        &self,
        shop_id: &ShopId,
        url: &Url,
        current_hash: &str,
        last_scraped_hash: Option<&str>,
    ) -> Result<Option<NormalizedProduct>, ScraperError> {
        if last_scraped_hash == Some(current_hash) {
            debug!(
                url = %url,
                "Hash matches last scraped hash, skipping fetch."
            );
            if let Err(e) = self
                .candidate_service
                .mark_as_scraped(shop_id, url, current_hash)
                .await
            {
                warn!(error = %e, "Failed to mark url as scraped after skip");
            }
            return Ok(None);
        }

        let domain = url.host_str().unwrap_or("unknown");

        // Tracks whether a schema fix was applied during this scrape.  Used
        // to reset the per-domain failure counter on a fully successful scrape
        // (schema fixed, extracted, and normalized without errors).
        let mut schema_was_fixed = false;

        // 1. Fetch HTML --------------------------------------------------
        debug!(domain, url = %url, "Fetching product page HTML");
        let html =
            self.html_fetcher
                .fetch(url)
                .await
                .map_err(|details| ScraperError::HttpError {
                    url: url.clone(),
                    details,
                })?;

        // 2. Obtain schema (from DB or freshly created by LLM) -----------
        debug!(domain, url = %url, "Obtaining product CSS selector schema");
        let shops_product_schema = self
            .schema_service
            .get_product_schema(shop_id, domain, &html)
            .await?;

        // 3. Apply schema → RawExtractedProduct -------------------------
        // Parse HTML and apply the schema synchronously before any await
        // boundary — scraper::Html is !Send so it must not be held across awaits.
        let schema: &ProductCssSelectorSchema = &shops_product_schema.product_schema;

        enum ApplyOutcome {
            Ok(RawExtractedProduct),
            NeedsFix { apply_error: ApplySchemaError },
        }

        let outcome = {
            let parsed_html = Html::parse_document(&html);
            match schema.apply(&parsed_html) {
                Ok(raw) => {
                    debug!(domain, url = %url, "Schema applied successfully");
                    ApplyOutcome::Ok(raw)
                }
                Err(apply_error) => {
                    warn!(
                        domain,
                        url = %url,
                        error = %apply_error,
                        "Schema application failed, attempting LLM-based fix"
                    );
                    ApplyOutcome::NeedsFix { apply_error }
                }
            }
        };

        let raw = match outcome {
            ApplyOutcome::Ok(raw) => raw,
            ApplyOutcome::NeedsFix { apply_error } => {
                // 3a. Schema failed — serialise fix attempts per domain so that
                // concurrent URLs for the same shop don't each trigger a
                // separate LLM call and overwrite each other's result.
                //
                // Algorithm:
                //   1. Acquire the per-domain mutex (creating it lazily).
                //   2. While holding the lock, re-fetch the schema from the DB
                //      — a concurrent goroutine may have already fixed it.
                //   3. Try applying the refreshed schema.  If it succeeds,
                //      we're done (no LLM call needed).
                //   4. Only if the refreshed schema still fails do we call the
                //      LLM, persist the fix, and re-apply.

                // Step 1: get (or create) the per-domain lock.
                let domain_lock = {
                    let mut map = self.schema_fix_locks.lock().await;
                    map.entry(domain.to_string())
                        .or_insert_with(|| Arc::new(Mutex::new(())))
                        .clone()
                };
                let _domain_guard = domain_lock.lock().await;

                // Step 2: re-fetch the schema — it may have been fixed while
                // we were waiting for the lock.
                debug!(domain, url = %url, "Re-fetching schema after fix-lock acquired");
                let refreshed_schema = self
                    .schema_service
                    .get_product_schema(shop_id, domain, &html)
                    .await?;

                // Step 3: try the refreshed schema first.
                let refreshed_apply = {
                    let parsed_html = Html::parse_document(&html);
                    refreshed_schema.product_schema.apply(&parsed_html)
                };

                match refreshed_apply {
                    Ok(raw) => {
                        debug!(domain, url = %url, "Refreshed schema applied successfully (no LLM fix needed)");
                        raw
                    }
                    Err(refreshed_error) => {
                        // Step 4: still broken — pre-increment the attempt
                        // counter before calling the LLM so that concurrent
                        // tasks waiting on this lock will see the incremented
                        // value and skip their own LLM call.
                        self.increment_fix_attempts(domain).await?;

                        let fixed_schema = self
                            .schema_service
                            .fix_product_schema(
                                &refreshed_schema.product_schema,
                                &refreshed_error,
                                &html,
                            )
                            .await
                            .map_err(|fix_error| ScraperError::SchemaFixFailed {
                                apply_error,
                                fix_error,
                            })?;

                        // Persist the fixed schema so subsequent scrapes benefit.
                        self.schema_service
                            .save_product_schema(shop_id, domain, fixed_schema.clone())
                            .await?;

                        info!(domain, url = %url, "Schema fixed via LLM");

                        // Re-apply synchronously — drop Html before any await.
                        let re_apply_result = {
                            let parsed_html = Html::parse_document(&html);
                            fixed_schema.apply(&parsed_html)
                        };
                        match re_apply_result {
                            Ok(raw) => {
                                // Fixed schema applied — mark that a fix happened so
                                // we reset the counter after successful normalization.
                                schema_was_fixed = true;
                                raw
                            }
                            Err(re_apply_error) => {
                                warn!(
                                    domain,
                                    url = %url,
                                    error = %re_apply_error,
                                    "Fixed schema also failed to apply"
                                );
                                return Err(ScraperError::SchemaFixFailed {
                                    apply_error: re_apply_error,
                                    fix_error: ProductSchemaServiceError::NoTextResponse(
                                        "Fixed schema failed to apply after being persisted"
                                            .to_string(),
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        };

        // 4. Normalise --------------------------------------------------
        // If normalization fails with a schema-fixable error (e.g. the price
        // selector extracted page-title text instead of a price), we trigger
        // the same domain-serialized LLM fix flow and retry once.
        debug!(domain, url = %url, "Normalizing extracted product data");
        let final_product = match self
            .normalization_service
            .normalize(raw.clone(), url.clone())
            .await
        {
            Ok(normalized) => normalized,
            Err(norm_err) => {
                // Check whether this looks like a bad selector (the CSS
                // matched something but the extracted text is not a valid
                // field value), versus an unrecoverable error.
                let hint: Option<ApplySchemaError> = normalization_error_to_schema_hint(&norm_err);

                let Some(hint_error) = hint else {
                    return Err(ScraperError::NormalizationError(norm_err));
                };

                warn!(
                    domain,
                    url = %url,
                    error = %norm_err,
                    "Normalization failed with schema-fixable error, attempting LLM-based schema fix"
                );

                // Use the same per-domain serialised fix path.
                let domain_lock = {
                    let mut map = self.schema_fix_locks.lock().await;
                    map.entry(domain.to_string())
                        .or_insert_with(|| Arc::new(Mutex::new(())))
                        .clone()
                };
                let _domain_guard = domain_lock.lock().await;

                // Re-fetch schema — a sibling task may have already fixed it.
                let refreshed_schema = self
                    .schema_service
                    .get_product_schema(shop_id, domain, &html)
                    .await?;

                // Try re-applying the refreshed schema.
                let refreshed_raw = {
                    let parsed_html = Html::parse_document(&html);
                    refreshed_schema.product_schema.apply(&parsed_html)
                };

                let re_raw = match refreshed_raw {
                    Ok(r) => r,
                    Err(apply_err) => {
                        // Schema also broken at the extraction level — pre-
                        // increment the attempt counter before calling the LLM.
                        self.increment_fix_attempts(domain).await?;

                        let fixed = self
                            .schema_service
                            .fix_product_schema(&refreshed_schema.product_schema, &apply_err, &html)
                            .await
                            .map_err(|fix_error| ScraperError::SchemaFixFailed {
                                apply_error: apply_err,
                                fix_error,
                            })?;
                        self.schema_service
                            .save_product_schema(shop_id, domain, fixed.clone())
                            .await?;
                        info!(domain, url = %url, "Schema fixed via LLM (normalization-triggered)");
                        let apply_result = {
                            let parsed_html = Html::parse_document(&html);
                            fixed.apply(&parsed_html)
                        };
                        match apply_result {
                            Ok(raw) => {
                                schema_was_fixed = true;
                                raw
                            }
                            Err(re_apply_error) => {
                                warn!(
                                    domain,
                                    url = %url,
                                    error = %re_apply_error,
                                    "Fixed schema also failed to apply after normalization fix"
                                );
                                return Err(ScraperError::SchemaFixFailed {
                                    apply_error: re_apply_error,
                                    fix_error: ProductSchemaServiceError::NoTextResponse(
                                        "Fixed schema failed to apply after normalization-triggered fix"
                                            .to_string(),
                                    ),
                                });
                            }
                        }
                    }
                };

                // Try normalizing with the refreshed raw; if still bad, ask
                // LLM to fix the schema using the normalization hint.
                match self
                    .normalization_service
                    .normalize(re_raw.clone(), url.clone())
                    .await
                {
                    Ok(normalized) => {
                        debug!(domain, url = %url, "Refreshed schema normalized successfully");
                        normalized
                    }
                    Err(_) => {
                        // Refreshed schema still produced wrong text — pre-
                        // increment the attempt counter before calling the LLM.
                        self.increment_fix_attempts(domain).await?;

                        let fixed = self
                            .schema_service
                            .fix_product_schema(
                                &refreshed_schema.product_schema,
                                &hint_error,
                                &html,
                            )
                            .await
                            .map_err(|fix_error| ScraperError::SchemaFixFailed {
                                apply_error: hint_error,
                                fix_error,
                            })?;
                        self.schema_service
                            .save_product_schema(shop_id, domain, fixed.clone())
                            .await?;
                        info!(domain, url = %url, "Schema fixed via LLM (normalization-triggered)");

                        let fixed_raw = {
                            let apply_result = {
                                let parsed_html = Html::parse_document(&html);
                                fixed.apply(&parsed_html)
                            };
                            match apply_result {
                                Ok(raw) => {
                                    schema_was_fixed = true;
                                    raw
                                }
                                Err(re_apply_error) => {
                                    warn!(
                                        domain,
                                        url = %url,
                                        error = %re_apply_error,
                                        "Fixed schema also failed to apply after normalization fix"
                                    );
                                    return Err(ScraperError::SchemaFixFailed {
                                        apply_error: re_apply_error,
                                        fix_error: ProductSchemaServiceError::NoTextResponse(
                                            "Fixed schema failed to apply after normalization-triggered fix"
                                                .to_string(),
                                        ),
                                    });
                                }
                            }
                        };

                        match self
                            .normalization_service
                            .normalize(fixed_raw, url.clone())
                            .await
                        {
                            Ok(normalized) => normalized,
                            Err(norm_err2) => {
                                // The LLM-fixed schema still can't normalize —
                                // the counter was already pre-incremented before
                                // the LLM call, so no additional recording needed.
                                return Err(ScraperError::NormalizationError(norm_err2));
                            }
                        }
                    }
                }
            }
        };

        if let Err(e) = self
            .candidate_service
            .mark_as_scraped(shop_id, url, current_hash)
            .await
        {
            warn!(error = %e, "Failed to mark product as scraped after success");
        }

        // If a schema fix was applied this scrape and everything succeeded
        // (extraction + normalization), reset the per-domain failure counter so
        // the domain can recover if the LLM fixes the schema in a future batch.
        if schema_was_fixed {
            self.reset_fix_attempts(domain).await;
        }

        debug!(
            domain,
            shops_product_id = %final_product.shops_product_id,
            url = %url,
            "Scraping complete"
        );
        Ok(Some(final_product))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Maps a [`NormalizationError`] to a synthetic [`ApplySchemaError`] hint that
/// can be fed to the LLM schema-fix path, or returns `None` for errors that
/// cannot be attributed to a wrong CSS selector (e.g. state mapping DB
/// errors).
///
/// The hint tells the LLM *which field* produced bad text so it can update the
/// selector.  We use `NoElementMatched` as the inner error because it carries
/// only a selector name string — we set it to the field name so the LLM
/// understands the context.
fn normalization_error_to_schema_hint(err: &NormalizationError) -> Option<ApplySchemaError> {
    match err {
        NormalizationError::PriceUnknownCurrency { .. }
        | NormalizationError::PriceParseError { .. } => {
            Some(ApplySchemaError::Price(ExtractionError::NoElementMatched {
                selector: "price".to_string(),
            }))
        }
        NormalizationError::PriceEstimateMinUnknownCurrency { .. }
        | NormalizationError::PriceEstimateMinParseError { .. } => Some(
            ApplySchemaError::PriceEstimateMin(ExtractionError::NoElementMatched {
                selector: "price_estimate_min".to_string(),
            }),
        ),
        NormalizationError::PriceEstimateMaxUnknownCurrency { .. }
        | NormalizationError::PriceEstimateMaxParseError { .. } => Some(
            ApplySchemaError::PriceEstimateMax(ExtractionError::NoElementMatched {
                selector: "price_estimate_max".to_string(),
            }),
        ),
        NormalizationError::TitleEmpty | NormalizationError::TitleUnknownLanguage { .. } => {
            Some(ApplySchemaError::Title(ExtractionError::NoElementMatched {
                selector: "title".to_string(),
            }))
        }
        NormalizationError::ShopsProductIdEmpty => Some(ApplySchemaError::ShopsProductId(
            ExtractionError::NoElementMatched {
                selector: "shops_product_id".to_string(),
            },
        )),
        NormalizationError::StateTextTooLong { .. } => {
            Some(ApplySchemaError::State(ExtractionError::NoElementMatched {
                selector: "state".to_string(),
            }))
        }
        // State mapping errors and image/auction errors are not schema fixable
        // via the LLM selector path.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scraper::candidate_service::MockScraperCandidateService;
    use crate::scraper::css_selector::product_schema::{
        ApplySchemaError, ProductCssSelectorSchema, ShopsProductSchema,
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
  <span id="product-id">SKU-42</span>
  <h1>Biedermeier Chair</h1>
  <span id="state">In Stock</span>
  <img src="/images/chair.jpg">
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
        }
    }

    fn shops_product_schema(shop_id: ShopId) -> ShopsProductSchema {
        ShopsProductSchema {
            shop_id,
            product_schema: minimal_schema(),
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
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().once().returning(move |_, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let result = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.shops_product_id, ShopsProductId::from("SKU-42"));
        assert_eq!(result.state, ProductState::Available);
        assert_eq!(result.url, url);
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
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .returning(move |_, _, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let norm_clone = norm.clone();
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(move |_, _| {
            let n = norm_clone.clone();
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let result = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result, norm);
    }

    #[tokio::test]
    async fn should_skip_fetching_and_return_none_when_hashes_match() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher.expect_fetch().times(0); // MUST NOT BE CHECKED

        let schema_svc = MockProductSchemaService::new();
        let norm_svc = MockProductNormalizationService::new();
        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .once()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let result = service
            .scrape(&id, &url, "same_hash", Some("same_hash"))
            .await
            .unwrap();

        assert!(result.is_none());
    }

    // -----------------------------------------------------------------------
    // Schema-fix path
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_fix_and_save_schema_then_succeed_when_initial_apply_fails() {
        let id = shop_id();
        let url = product_url();

        // Build a broken schema (wrong selectors) so `apply` will error
        let broken_schema = {
            let bad_rule = ExtractionRule {
                selector: CssSelector::from("#does-not-exist"),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            };
            ShopsProductSchema {
                shop_id: id,
                product_schema: ProductCssSelectorSchema {
                    shops_product_id: bad_rule.clone(),
                    title: bad_rule.clone(),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: bad_rule.clone(),
                    images: bad_rule,
                    auction_start: None,
                    auction_end: None,
                },
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        };
        let good_schema = minimal_schema();

        let broken_schema_for_refetch = {
            let bad_rule = ExtractionRule {
                selector: CssSelector::from("#does-not-exist"),
                additional_selectors: vec![],
                extract: ExtractionKind::Text,
                cardinality: ExtractionCardinality::First,
            };
            ShopsProductSchema {
                shop_id: id,
                product_schema: ProductCssSelectorSchema {
                    shops_product_id: bad_rule.clone(),
                    title: bad_rule.clone(),
                    description: None,
                    price: None,
                    price_estimate_min: None,
                    price_estimate_max: None,
                    state: bad_rule.clone(),
                    images: bad_rule,
                    auction_start: None,
                    auction_end: None,
                },
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            }
        };

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();

        // First call: initial schema fetch
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = broken_schema.clone();
                Box::pin(async move { Ok(s) })
            });

        // Second call: re-fetch inside the fix-lock path (still broken — LLM fix will run)
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = broken_schema_for_refetch.clone();
                Box::pin(async move { Ok(s) })
            });

        schema_svc
            .expect_fix_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = good_schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let saved_schema = shops_product_schema(id);
        schema_svc
            .expect_save_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = saved_schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().once().returning(move |_, _| {
            let n = norm.clone();
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let result = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.shops_product_id, ShopsProductId::from("SKU-42"));
    }

    #[tokio::test]
    async fn should_return_schema_fix_failed_error_when_fix_service_errors() {
        let id = shop_id();
        let url = product_url();

        let bad_rule = ExtractionRule {
            selector: CssSelector::from("#nope"),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        };
        let broken_schema = ShopsProductSchema {
            shop_id: id,
            product_schema: ProductCssSelectorSchema {
                shops_product_id: bad_rule.clone(),
                title: bad_rule.clone(),
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: bad_rule.clone(),
                images: bad_rule,
                auction_start: None,
                auction_end: None,
            },
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .returning(move |_, _, _| {
                let s = broken_schema.clone();
                Box::pin(async move { Ok(s) })
            });
        schema_svc.expect_fix_product_schema().returning(|_, _, _| {
            Box::pin(async {
                Err(ProductSchemaServiceError::NoTextResponse(
                    "LLM gave up".to_string(),
                ))
            })
        });

        let norm_svc = MockProductNormalizationService::new();
        let cand_svc = MockScraperCandidateService::new();

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let err = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ScraperError::SchemaFixFailed { .. }),
            "expected SchemaFixFailed, got: {err}"
        );
    }

    #[tokio::test]
    async fn should_save_fixed_schema_before_applying_it_when_fix_succeeds() {
        let id = shop_id();
        let url = product_url();

        let bad_rule = ExtractionRule {
            selector: CssSelector::from("#no-match"),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        };
        let broken_schema = ShopsProductSchema {
            shop_id: id,
            product_schema: ProductCssSelectorSchema {
                shops_product_id: bad_rule.clone(),
                title: bad_rule.clone(),
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: bad_rule.clone(),
                images: bad_rule,
                auction_start: None,
                auction_end: None,
            },
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };
        let good_schema = minimal_schema();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .returning(move |_, _, _| {
                let s = broken_schema.clone();
                Box::pin(async move { Ok(s) })
            });
        schema_svc
            .expect_fix_product_schema()
            .returning(move |_, _, _| {
                let s = good_schema.clone();
                Box::pin(async move { Ok(s) })
            });

        // The key assertion: save must be called exactly once
        let saved = shops_product_schema(id);
        schema_svc
            .expect_save_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = saved.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(move |_, _| {
            let n = norm.clone();
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // HTTP errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_http_error_when_fetcher_fails() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher.expect_fetch().returning(|_| {
            Box::pin(async {
                reqwest::Client::new()
                    .get("http://0.0.0.0:1")
                    .send()
                    .await
                    .map(|_| String::new())
                    .map_err(|e| e.to_string())
            })
        });

        let schema_svc = MockProductSchemaService::new();
        let norm_svc = MockProductNormalizationService::new();
        let cand_svc = MockScraperCandidateService::new();

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let err = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ScraperError::HttpError { .. }),
            "expected HttpError, got: {err}"
        );
    }

    #[tokio::test]
    async fn should_include_url_in_http_error_when_fetch_fails() {
        let id = shop_id();
        let url = product_url();
        let url_clone = url.clone();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher.expect_fetch().returning(move |_| {
            Box::pin(async {
                reqwest::Client::new()
                    .get("http://0.0.0.0:1")
                    .send()
                    .await
                    .map(|_| String::new())
                    .map_err(|e| e.to_string())
            })
        });

        let schema_svc = MockProductSchemaService::new();
        let norm_svc = MockProductNormalizationService::new();
        let cand_svc = MockScraperCandidateService::new();

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let err = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap_err();

        if let ScraperError::HttpError { url: err_url, .. } = err {
            assert_eq!(err_url, url_clone);
        } else {
            panic!("expected HttpError");
        }
    }

    // -----------------------------------------------------------------------
    // Schema-service errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_schema_service_error_when_get_product_schema_fails() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();
        schema_svc.expect_get_product_schema().returning(|_, _, _| {
            Box::pin(async {
                Err(ProductSchemaServiceError::NoTextResponse(
                    "LLM timed out".to_string(),
                ))
            })
        });

        let norm_svc = MockProductNormalizationService::new();
        let cand_svc = MockScraperCandidateService::new();

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let err = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ScraperError::SchemaServiceError(_)),
            "expected SchemaServiceError, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // Normalization errors
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_return_normalization_error_when_normalization_service_fails() {
        let id = shop_id();
        let url = product_url();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .returning(move |_, _, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(|_, _| {
            Box::pin(async {
                Err(NormalizationError::InvalidImageUrl {
                    raw: "not-a-url".to_string(),
                    source: url::Url::parse("://bad").unwrap_err(),
                })
            })
        });

        let cand_svc = MockScraperCandidateService::new();

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let err = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap_err();

        assert!(
            matches!(err, ScraperError::NormalizationError(_)),
            "expected NormalizationError, got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // URL forwarding
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn should_forward_url_to_normalization_service_when_normalizing() {
        let id = shop_id();
        let url = Url::parse("https://example.com/items/999").unwrap();
        let expected_url = url.clone();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .returning(move |_, _, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .withf(move |_, received_url| received_url == &expected_url)
            .once()
            .returning(move |_, u| {
                let n = normalized_product(u);
                Box::pin(async move { Ok(n) })
            });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn should_preserve_url_from_normalization_result_when_returning_product() {
        let id = shop_id();
        let url = product_url();
        let canonical_url = Url::parse("https://example.com/canonical/123").unwrap();
        let canonical_for_norm = canonical_url.clone();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let schema = shops_product_schema(id);
        let mut schema_svc = MockProductSchemaService::new();
        schema_svc
            .expect_get_product_schema()
            .returning(move |_, _, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(move |_, _| {
            let n = normalized_product(canonical_for_norm.clone());
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );

        let result = service
            .scrape(&id, &url, "current_hash", None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.url, canonical_url);
    }

    // -----------------------------------------------------------------------
    // SpiderHtmlFetcher
    // -----------------------------------------------------------------------

    #[test]
    fn should_construct_spider_html_fetcher() {
        let _ = SpiderHtmlFetcher::new();
    }

    #[tokio::test]
    async fn should_fail_gracefully_when_fetching_invalid_url() {
        let fetcher = SpiderHtmlFetcher::new();
        // Use a port that is highly unlikely to have a web server running
        let url = Url::parse("http://127.0.0.1:1/nonexistent").unwrap();

        let result = fetcher.fetch(&url).await;

        assert!(
            result.is_err(),
            "Fetching from an invalid server should return an error"
        );
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("Spider could not fetch HTML"),
            "Error message should match the fetcher's custom error: {}",
            err_msg
        );
    }

    // -----------------------------------------------------------------------
    // ScraperServiceImpl constructor
    // -----------------------------------------------------------------------

    #[test]
    fn should_construct_scraper_service_impl_with_all_dependencies() {
        let fetcher = MockHtmlFetcher::new();
        let schema_svc = MockProductSchemaService::new();
        let norm_svc = MockProductNormalizationService::new();

        let cand_svc = MockScraperCandidateService::new();
        let _ = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            3,
        );
    }

    // -----------------------------------------------------------------------
    // ScraperError display
    // -----------------------------------------------------------------------

    #[test]
    fn should_display_schema_fix_failed_error_with_both_causes() {
        let apply_error = ApplySchemaError::ShopsProductId(
            crate::scraper::css_selector::rule::ExtractionError::NoElementMatched {
                selector: "#id".to_string(),
            },
        );
        let fix_error = ProductSchemaServiceError::NoTextResponse("no text".to_string());
        let err = ScraperError::SchemaFixFailed {
            apply_error,
            fix_error,
        };
        let display = err.to_string();
        assert!(
            display.contains("apply"),
            "display should mention apply: {display}"
        );
        assert!(
            display.contains("fix"),
            "display should mention fix: {display}"
        );
    }

    #[test]
    fn should_display_normalization_error_variant_correctly() {
        let err = ScraperError::NormalizationError(NormalizationError::ShopsProductIdEmpty);
        let display = err.to_string();
        assert!(
            display.to_lowercase().contains("normalization"),
            "display should mention normalization: {display}"
        );
    }

    // -----------------------------------------------------------------------
    // Schema fix attempt limiting (Bug 3)
    // -----------------------------------------------------------------------

    fn broken_shops_product_schema(id: ShopId) -> ShopsProductSchema {
        let bad_rule = ExtractionRule {
            selector: CssSelector::from("#does-not-exist"),
            additional_selectors: vec![],
            extract: ExtractionKind::Text,
            cardinality: ExtractionCardinality::First,
        };
        ShopsProductSchema {
            shop_id: id,
            product_schema: ProductCssSelectorSchema {
                shops_product_id: bad_rule.clone(),
                title: bad_rule.clone(),
                description: None,
                price: None,
                price_estimate_min: None,
                price_estimate_max: None,
                state: bad_rule.clone(),
                images: bad_rule,
                auction_start: None,
                auction_end: None,
            },
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        }
    }

    /// After `max_schema_fix_attempts` failed fix attempts the service must
    /// return `SchemaFixAttemptsExhausted` without calling `fix_product_schema`
    /// again.  We use `max=0` to immediately exhaust the budget on the first
    /// attempt, so no LLM call is made at all.
    #[tokio::test]
    async fn should_return_exhausted_error_after_max_failed_fix_attempts() {
        let id = shop_id();
        let url = product_url();

        let broken1 = broken_shops_product_schema(id);
        let broken2 = broken_shops_product_schema(id);

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();

        // Initial schema fetch
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = broken1.clone();
                Box::pin(async move { Ok(s) })
            });
        // Re-fetch inside the fix-lock (still broken)
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = broken2.clone();
                Box::pin(async move { Ok(s) })
            });
        // fix_product_schema must NOT be registered — max=0 means skip immediately.
        // If called, the mock will panic with an unexpected call.

        let norm_svc = MockProductNormalizationService::new();
        let cand_svc = MockScraperCandidateService::new();

        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            0, // max = 0 → immediately exhausted, no LLM calls
        );

        let err = service.scrape(&id, &url, "hash1", None).await.unwrap_err();

        assert!(
            matches!(err, ScraperError::SchemaFixAttemptsExhausted { .. }),
            "should return SchemaFixAttemptsExhausted when max=0, got: {err}"
        );
    }

    /// After a successful schema fix (LLM fix applied and normalized cleanly)
    /// the failure counter is reset to zero, allowing future fix attempts.
    #[tokio::test]
    async fn should_reset_fix_attempts_counter_after_successful_fix() {
        let id = shop_id();
        let url = product_url();

        // Build the broken schema that will always fail apply
        let broken1 = broken_shops_product_schema(id);
        let broken2 = broken_shops_product_schema(id);
        // The LLM "fixes" it with the good schema
        let good = minimal_schema();
        let good_clone = good.clone();

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();
        // First call: initial fetch
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = broken1.clone();
                Box::pin(async move { Ok(s) })
            });
        // Second call: re-fetch inside fix-lock
        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = broken2.clone();
                Box::pin(async move { Ok(s) })
            });
        // LLM returns a working schema
        schema_svc
            .expect_fix_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = good_clone.clone();
                Box::pin(async move { Ok(s) })
            });
        let saved = shops_product_schema(id);
        schema_svc
            .expect_save_product_schema()
            .once()
            .returning(move |_, _, _| {
                let s = saved.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().once().returning(move |_, _| {
            let n = norm.clone();
            Box::pin(async move { Ok(n) })
        });

        let mut cand_svc = MockScraperCandidateService::new();
        cand_svc
            .expect_mark_as_scraped()
            .returning(|_, _, _| Box::pin(async { Ok(()) }));

        // max = 1 so only 1 failed attempt is allowed before exhaustion
        let service = ScraperServiceImpl::new(
            Box::new(fetcher),
            Box::new(schema_svc),
            Box::new(norm_svc),
            Arc::new(cand_svc),
            1,
        );

        // This scrape should succeed (fix applied + normalization ok) and reset the counter
        let result = service
            .scrape(&id, &url, "hash1", None)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(result.shops_product_id, ShopsProductId::from("SKU-42"));

        // Counter is now 0 — verify by checking the internal state directly
        let attempts = service.schema_fix_attempts.lock().await;
        assert!(
            attempts.get("example.com").is_none(),
            "fix attempt counter should be reset after successful fix"
        );
    }

    // -----------------------------------------------------------------------
    // normalization_error_to_schema_hint — StateTextTooLong
    // -----------------------------------------------------------------------

    #[test]
    fn should_return_state_schema_hint_for_state_text_too_long() {
        let err = NormalizationError::StateTextTooLong {
            len: 1024,
            max: 512,
        };
        let hint = normalization_error_to_schema_hint(&err);
        assert!(
            matches!(
                hint,
                Some(ApplySchemaError::State(
                    ExtractionError::NoElementMatched { .. }
                ))
            ),
            "expected State schema hint for StateTextTooLong, got {hint:?}"
        );
    }

    #[test]
    fn should_return_none_for_state_mapping_error_in_schema_hint() {
        use crate::scraper::normalization::state_mapping_service::StateMappingServiceError;
        let err = NormalizationError::StateMappingError(StateMappingServiceError::DatabaseError(
            sqlx::Error::RowNotFound,
        ));
        let hint = normalization_error_to_schema_hint(&err);
        assert!(
            hint.is_none(),
            "StateMappingError should not produce a schema hint"
        );
    }
}
