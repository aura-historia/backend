use crate::scraper::scraper_service::pipeline::scrape_product::is_redirect_to_non_product_page;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use shop_core::shop_id::ShopId;
use std::collections::HashSet;
use tracing::warn;
use url::Url;

pub(crate) struct SchemaSeedPage {
    pub(crate) url: Url,
    pub(crate) raw_html: String,
}

impl ScraperServiceImpl {
    /// Fetches up to `schema_seed_pages` HTML pages to use as context when
    /// generating a schema for the first time.  Always includes `primary_html`
    /// as the first entry.  Best-effort: any fetch failure is logged and
    /// skipped.
    #[tracing::instrument(
        skip(self, primary_html),
        fields(shop_id = %shop_id, url = %url, schema_seed_pages = self.schema_seed_pages)
    )]
    pub(crate) async fn collect_schema_seed_pages(
        &self,
        shop_id: &ShopId,
        url: &Url,
        product_url_pattern: Option<&str>,
        primary_html: &str,
    ) -> Vec<SchemaSeedPage> {
        let mut pages = vec![SchemaSeedPage {
            url: url.clone(),
            raw_html: primary_html.to_string(),
        }];
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
                    error = ?err,
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
                Ok(sample) => {
                    if is_redirect_to_non_product_page(
                        &sample_url,
                        &sample.final_url,
                        product_url_pattern,
                    ) {
                        warn!(
                            sample_url = %sample_url,
                            final_url = %sample.final_url,
                            "Skipping sampled schema-seed page because it redirected to a non-product page"
                        );
                        continue;
                    }
                    pages.push(SchemaSeedPage {
                        url: sample_url.clone(),
                        raw_html: sample.html,
                    });
                }
                Err(err) => {
                    warn!(
                        error = ?err,
                        sample_url = %sample_url,
                        "Failed to fetch sampled schema-seed page; continuing with available samples"
                    );
                }
            }
        }

        pages
    }
}
