use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use time::OffsetDateTime;
use tracing::{debug, info, warn};

use crate::classification::gemini_client::GeminiClient;
use crate::classification::link_metadata_repository::{LinkMetadataRepository, SpiderLinkRecord};
use crate::classification::url_classification_service::{
    filter_product_urls, matches_product_pattern,
};
use crate::classification::url_pattern_repository::ShopUrlPatternRepository;
use crate::classification::url_pattern_service::UrlPatternService;
use crate::crawling::crawl_service::{CrawledPage, start_crawl};
use crate::error::SpiderError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkClass {
    Product,
    Category,
    Imprint,
    Info,
    Other,
}

impl LinkClass {
    pub fn as_str(self) -> &'static str {
        match self {
            LinkClass::Product => "product",
            LinkClass::Category => "category",
            LinkClass::Imprint => "imprint",
            LinkClass::Info => "info",
            LinkClass::Other => "other",
        }
    }

    fn from_db(value: &str) -> Self {
        match value {
            "product" => LinkClass::Product,
            "category" => LinkClass::Category,
            "imprint" => LinkClass::Imprint,
            "info" => LinkClass::Info,
            _ => LinkClass::Other,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawledLinkMetadata {
    pub url: String,
    pub class: LinkClass,
    pub hash: String,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,

    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderRunResult {
    pub links: Vec<CrawledLinkMetadata>,
    pub product_urls: Vec<String>,
    pub product_pattern: Option<String>,
}

impl From<SpiderLinkRecord> for CrawledLinkMetadata {
    fn from(value: SpiderLinkRecord) -> Self {
        Self {
            url: value.url,
            class: LinkClass::from_db(&value.link_class),
            hash: value.main_hash,
            created: value.created,
            updated: value.updated,
        }
    }
}

pub struct SpiderService {
    shop_url: String,
    classify_threshold: usize,
    gemini_client: GeminiClient,
    pattern_repository: Arc<dyn ShopUrlPatternRepository>,
    link_metadata_repository: Arc<dyn LinkMetadataRepository>,
}

impl SpiderService {
    pub fn new(
        shop_url: String,
        api_key: String,
        classify_threshold: usize,
        pattern_repository: Arc<dyn ShopUrlPatternRepository>,
        link_metadata_repository: Arc<dyn LinkMetadataRepository>,
    ) -> Self {
        let gemini_client = GeminiClient::new(api_key);
        Self {
            shop_url,
            classify_threshold,
            gemini_client,
            pattern_repository,
            link_metadata_repository,
        }
    }

    pub async fn run(&self) -> Result<SpiderRunResult, SpiderError> {
        info!(shopUrl = %self.shop_url, "Starting crawl");
        info!(
            classifyThreshold = self.classify_threshold,
            "Configured one-time classification threshold"
        );

        let mut crawl_rx = start_crawl(&self.shop_url).await?;

        let pattern_service = UrlPatternService::new(self.pattern_repository.clone());

        let mut all_pages: Vec<CrawledPage> = Vec::new();
        let mut all_urls: Vec<String> = Vec::new();
        let mut total_crawled: usize = 0;

        // Check the store first; only fall back to Gemini when nothing is persisted.
        let mut pattern: Option<Regex> = pattern_service
            .load_pattern_for_shop_url(&self.shop_url)
            .await?;
        let mut classification_done = pattern.is_some();
        let mut pattern_loaded_from_store = pattern.is_some();

        if pattern_loaded_from_store {
            info!(
                shopUrl = %self.shop_url,
                "Loaded persisted product URL pattern"
            );
        }

        while let Some(page) = crawl_rx.recv().await {
            total_crawled += 1;
            all_urls.push(page.url.clone());
            all_pages.push(page.clone());

            if !classification_done && all_urls.len() >= self.classify_threshold {
                info!(
                    urlCount = all_urls.len(),
                    "Threshold reached, requesting Gemini URL pattern"
                );
                pattern = pattern_service
                    .classify_and_save(&self.shop_url, &all_urls, &self.gemini_client)
                    .await?;
                if pattern.is_none() {
                    warn!(
                        shopUrl = %self.shop_url,
                        "Gemini found no product URL pattern at threshold classification"
                    );
                    classification_done = true;
                    pattern_loaded_from_store = false;
                    continue;
                }
                classification_done = true;
                pattern_loaded_from_store = false;

                let matched_count = all_urls
                    .iter()
                    .filter(|url| matches_product_pattern(&pattern, url))
                    .count();
                info!(
                    matchedCount = matched_count,
                    urlCount = all_urls.len(),
                    "Classified threshold sample URLs"
                );
            }

            if classification_done {
                if let Some(last_url) = all_urls.last() {
                    if matches_product_pattern(&pattern, last_url) {
                        debug!(index = total_crawled, url = %last_url, "URL matches product pattern");
                    } else {
                        debug!(
                            index = total_crawled,
                            url = %last_url,
                            "URL does not match product pattern"
                        );
                    }
                }
            } else if let Some(last_url) = all_urls.last() {
                debug!(index = total_crawled, url = %last_url, "Crawled URL");
            }

            if total_crawled.is_multiple_of(100) {
                let products_so_far = all_urls
                    .iter()
                    .filter(|url| matches_product_pattern(&pattern, url))
                    .count();
                info!(
                    totalCrawled = total_crawled,
                    productsSoFar = products_so_far,
                    "Crawl progress"
                );
            }
        }

        info!(totalCrawled = total_crawled, "Crawl complete");

        if !classification_done && !all_urls.is_empty() {
            info!(
                urlCount = all_urls.len(),
                "Threshold not reached, classifying collected URLs"
            );
            pattern = pattern_service
                .classify_and_save(&self.shop_url, &all_urls, &self.gemini_client)
                .await?;
            if pattern.is_none() {
                warn!(
                    shopUrl = %self.shop_url,
                    "Gemini found no product URL pattern in collected URLs"
                );
            }

            let matched_count = all_urls
                .iter()
                .filter(|url| matches_product_pattern(&pattern, url))
                .count();
            info!(
                matchedCount = matched_count,
                urlCount = all_urls.len(),
                "Classified collected URLs"
            );
        }

        let mut confirmed_products = if pattern.is_some() {
            filter_product_urls(&pattern, &all_urls)
        } else {
            Ok(Vec::new())
        };

        if pattern_loaded_from_store && confirmed_products.is_err() {
            warn!(
                shopUrl = %self.shop_url,
                "Persisted product URL pattern did not match crawl results, reclassifying"
            );
            pattern = pattern_service
                .classify_and_save(&self.shop_url, &all_urls, &self.gemini_client)
                .await?;
            if pattern.is_none() {
                warn!(
                    shopUrl = %self.shop_url,
                    "Gemini found no product URL pattern while refreshing persisted pattern"
                );
                confirmed_products = Ok(Vec::new());
            } else {
                confirmed_products = filter_product_urls(&pattern, &all_urls);
            }
        }

        let confirmed_products = match confirmed_products {
            Ok(urls) => urls,
            Err(error) => {
                warn!(error = %error, "No confirmed product URLs after classification");
                Vec::new()
            }
        };

        let product_pattern = pattern.as_ref().map(|regex| regex.as_str().to_string());

        info!(
            confirmedProductCount = confirmed_products.len(),
            "Collected confirmed product URLs"
        );

        let links = self.persist_link_metadata(&all_pages, &pattern).await?;

        Ok(SpiderRunResult {
            links,
            product_urls: confirmed_products,
            product_pattern,
        })
    }

    async fn persist_link_metadata(
        &self,
        pages: &[CrawledPage],
        pattern: &Option<Regex>,
    ) -> Result<Vec<CrawledLinkMetadata>, SpiderError> {
        let normalized_shop_url = Self::normalize_shop_url(&self.shop_url)?;
        let mut metadata = Vec::with_capacity(pages.len());

        for page in pages {
            let link_class = classify_link(&page.url, pattern);
            let record = self
                .link_metadata_repository
                .upsert_link(
                    &normalized_shop_url,
                    &page.url,
                    link_class.as_str(),
                    &page.main_hash,
                )
                .await?;

            metadata.push(record.into());
        }

        Ok(metadata)
    }

    fn normalize_shop_url(shop_url: &str) -> Result<String, SpiderError> {
        let parsed = url::Url::parse(shop_url).map_err(|error| {
            SpiderError::Spider(format!(
                "Invalid shop URL '{shop_url}' while resolving pattern scope: {error}"
            ))
        })?;

        let host = parsed.host_str().ok_or_else(|| {
            SpiderError::Spider(format!(
                "Shop URL '{shop_url}' has no host for pattern scope"
            ))
        })?;

        let scheme = parsed.scheme().to_ascii_lowercase();
        let host = host.to_ascii_lowercase();

        Ok(match parsed.port() {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        })
    }
}

fn classify_link(url: &str, product_pattern: &Option<Regex>) -> LinkClass {
    if matches_product_pattern(product_pattern, url) {
        return LinkClass::Product;
    }

    let lower = url.to_ascii_lowercase();

    if ["imprint", "impressum", "mentions-legales", "legal-notice"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return LinkClass::Imprint;
    }

    if ["category", "categorie", "kategorie", "collections", "shop"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return LinkClass::Category;
    }

    if ["about", "contact", "faq", "terms", "privacy"]
        .iter()
        .any(|needle| lower.contains(needle))
    {
        return LinkClass::Info;
    }

    LinkClass::Other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_classify_product_when_pattern_matches_for_type() {
        let pattern = Regex::new(r"/product/").ok();

        let class = classify_link("https://example.com/product/42", &pattern);

        assert_eq!(class, LinkClass::Product);
    }

    #[test]
    fn should_classify_imprint_when_url_contains_legal_keywords_for_type() {
        let pattern: Option<Regex> = None;

        let class = classify_link("https://example.com/impressum", &pattern);

        assert_eq!(class, LinkClass::Imprint);
    }

    #[test]
    fn should_classify_category_when_url_contains_category_keywords_for_type() {
        let pattern: Option<Regex> = None;

        let class = classify_link("https://example.com/collections/modern", &pattern);

        assert_eq!(class, LinkClass::Category);
    }

    #[test]
    fn should_classify_other_when_url_does_not_match_any_rule_for_type() {
        let pattern: Option<Regex> = None;

        let class = classify_link("https://example.com/random-page", &pattern);

        assert_eq!(class, LinkClass::Other);
    }

    #[test]
    fn should_map_known_db_value_when_loading_link_class() {
        let class = LinkClass::from_db("category");

        assert_eq!(class, LinkClass::Category);
    }

    #[test]
    fn should_map_unknown_db_value_to_other_when_loading_link_class() {
        let class = LinkClass::from_db("unknown-value");

        assert_eq!(class, LinkClass::Other);
    }
}
