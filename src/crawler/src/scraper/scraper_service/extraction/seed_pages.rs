use crate::logging::{COMPONENT_SCRAPER, CRAWLER_SERVICE_NAME};
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use common::shop_id::ShopId;
use std::collections::HashSet;
use tracing::warn;
use url::Url;

impl ScraperServiceImpl {
    /// Fetches up to `schema_seed_pages` HTML pages to use as context when
    /// generating a schema for the first time.  Always includes `primary_html`
    /// as the first entry.  Best-effort: any fetch failure is logged and
    /// skipped.
    pub(crate) async fn collect_schema_seed_pages(
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
                    service = CRAWLER_SERVICE_NAME,
                    component = COMPONENT_SCRAPER,
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
                        service = CRAWLER_SERVICE_NAME,
                        component = COMPONENT_SCRAPER,
                        error = %err,
                        shop_id = %shop_id,
                        url = %url,
                        sample_url = %sample_url,
                        "Failed to fetch sampled schema-seed page; continuing with available samples"
                    );
                }
            }
        }

        pages
    }
}
