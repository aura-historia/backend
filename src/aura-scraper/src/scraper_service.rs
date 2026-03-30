use crate::css_selector::product_schema::{
    ApplySchemaError, ProductCssSelectorSchema, RawExtractedProduct,
};
use crate::css_selector::product_schema_service::{
    ProductSchemaService, ProductSchemaServiceError,
};
use crate::normalization::error::NormalizationError;
use crate::normalization::product::NormalizedProduct;
use crate::normalization::product_normalization_service::ProductNormalizationService;
use common::shop_id::ShopId;
use scraper::Html;
use tracing::{debug, warn};
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
    async fn scrape(&self, shop_id: &ShopId, url: &Url) -> Result<NormalizedProduct, ScraperError>;
}

// ---------------------------------------------------------------------------
// ScraperServiceImpl
// ---------------------------------------------------------------------------

pub struct ScraperServiceImpl {
    html_fetcher: Box<dyn HtmlFetcher>,
    schema_service: Box<dyn ProductSchemaService + Send + Sync>,
    normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
}

impl ScraperServiceImpl {
    pub fn new(
        html_fetcher: Box<dyn HtmlFetcher>,
        schema_service: Box<dyn ProductSchemaService + Send + Sync>,
        normalization_service: Box<dyn ProductNormalizationService + Send + Sync>,
    ) -> Self {
        Self {
            html_fetcher,
            schema_service,
            normalization_service,
        }
    }
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    async fn scrape(&self, shop_id: &ShopId, url: &Url) -> Result<NormalizedProduct, ScraperError> {
        // 1. Fetch HTML --------------------------------------------------
        debug!(shopId = %shop_id, url = %url, "Fetching product page HTML");
        let html =
            self.html_fetcher
                .fetch(url)
                .await
                .map_err(|details| ScraperError::HttpError {
                    url: url.clone(),
                    details,
                })?;

        // 2. Obtain schema (from DB or freshly created by LLM) -----------
        debug!(shopId = %shop_id, url = %url, "Obtaining product CSS selector schema");
        let shops_product_schema = self
            .schema_service
            .get_product_schema(shop_id, &html)
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
                    debug!(shopId = %shop_id, url = %url, "Schema applied successfully");
                    ApplyOutcome::Ok(raw)
                }
                Err(apply_error) => {
                    warn!(
                        shopId = %shop_id,
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
                // 3a. Schema failed — ask LLM to fix it, then persist and retry.
                // Html has been dropped above so we can safely await here.
                let fixed_schema = self
                    .schema_service
                    .fix_product_schema(schema, &apply_error, &html)
                    .await
                    .map_err(|fix_error| ScraperError::SchemaFixFailed {
                        apply_error,
                        fix_error,
                    })?;

                // Persist the fixed schema so subsequent scrapes benefit from it
                self.schema_service
                    .save_product_schema(shop_id, fixed_schema.clone())
                    .await?;

                // Re-apply synchronously — again drop Html before any await
                let parsed_html = Html::parse_document(&html);
                fixed_schema.apply(&parsed_html).map_err(|re_apply_error| {
                    warn!(
                        shopId = %shop_id,
                        url = %url,
                        error = %re_apply_error,
                        "Fixed schema also failed to apply"
                    );
                    ScraperError::SchemaFixFailed {
                        apply_error: re_apply_error,
                        fix_error: ProductSchemaServiceError::NoTextResponse(
                            "Fixed schema failed to apply after being persisted".to_string(),
                        ),
                    }
                })?
            }
        };

        // 4. Normalise --------------------------------------------------
        debug!(shopId = %shop_id, url = %url, "Normalizing extracted product data");
        let normalized = self
            .normalization_service
            .normalize(raw, url.clone())
            .await?;

        debug!(
            shopId = %shop_id,
            shopsProductId = %normalized.shops_product_id,
            url = %url,
            "Scraping complete"
        );
        Ok(normalized)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css_selector::product_schema::{
        ApplySchemaError, ProductCssSelectorSchema, ShopsProductSchema,
    };
    use crate::css_selector::product_schema_service::MockProductSchemaService;
    use crate::css_selector::rule::{
        CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
    };
    use crate::normalization::product_normalization_service::MockProductNormalizationService;
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
            .returning(move |_, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let expected = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().once().returning(move |_, _| {
            let n = expected.clone();
            Box::pin(async move { Ok(n) })
        });

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let result = service.scrape(&id, &url).await.unwrap();

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
            .returning(move |_, _| {
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

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let result = service.scrape(&id, &url).await.unwrap();

        assert_eq!(result, norm);
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

        let mut fetcher = MockHtmlFetcher::new();
        fetcher
            .expect_fetch()
            .returning(|_| Box::pin(async { Ok(sample_html()) }));

        let mut schema_svc = MockProductSchemaService::new();

        schema_svc
            .expect_get_product_schema()
            .once()
            .returning(move |_, _| {
                let s = broken_schema.clone();
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
            .returning(move |_, _| {
                let s = saved_schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().once().returning(move |_, _| {
            let n = norm.clone();
            Box::pin(async move { Ok(n) })
        });

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let result = service.scrape(&id, &url).await.unwrap();

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
            .returning(move |_, _| {
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

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let err = service.scrape(&id, &url).await.unwrap_err();

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
            .returning(move |_, _| {
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
            .returning(move |_, _| {
                let s = saved.clone();
                Box::pin(async move { Ok(s) })
            });

        let norm = normalized_product(url.clone());
        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(move |_, _| {
            let n = norm.clone();
            Box::pin(async move { Ok(n) })
        });

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        service.scrape(&id, &url).await.unwrap();
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

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let err = service.scrape(&id, &url).await.unwrap_err();

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

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let err = service.scrape(&id, &url).await.unwrap_err();

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
        schema_svc.expect_get_product_schema().returning(|_, _| {
            Box::pin(async {
                Err(ProductSchemaServiceError::NoTextResponse(
                    "LLM timed out".to_string(),
                ))
            })
        });

        let norm_svc = MockProductNormalizationService::new();

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let err = service.scrape(&id, &url).await.unwrap_err();

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
            .returning(move |_, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc
            .expect_normalize()
            .returning(|_, _| Box::pin(async { Err(NormalizationError::ShopsProductIdEmpty) }));

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let err = service.scrape(&id, &url).await.unwrap_err();

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
            .returning(move |_, _| {
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

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        service.scrape(&id, &url).await.unwrap();
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
            .returning(move |_, _| {
                let s = schema.clone();
                Box::pin(async move { Ok(s) })
            });

        let mut norm_svc = MockProductNormalizationService::new();
        norm_svc.expect_normalize().returning(move |_, _| {
            let n = normalized_product(canonical_for_norm.clone());
            Box::pin(async move { Ok(n) })
        });

        let service =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));

        let result = service.scrape(&id, &url).await.unwrap();

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

        let _ =
            ScraperServiceImpl::new(Box::new(fetcher), Box::new(schema_svc), Box::new(norm_svc));
    }

    // -----------------------------------------------------------------------
    // ScraperError display
    // -----------------------------------------------------------------------

    #[test]
    fn should_display_schema_fix_failed_error_with_both_causes() {
        let apply_error = ApplySchemaError::ShopsProductId(
            crate::css_selector::rule::ExtractionError::NoElementMatched {
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
}
