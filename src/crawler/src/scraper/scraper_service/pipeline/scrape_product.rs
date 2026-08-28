use crate::network::policy::NetworkErrorKind;
use crate::scraper::candidate_service::ProductListingSnapshot;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::domain::product::{ScrapedProduct, ScraperService};
use crate::scraper::scraper_service::pipeline::cached_schema_selection::ExistingSchemaSelection;
use crate::scraper::scraper_service::pipeline::fresh_schema_generation::FreshSchemaGenerationContext;
use crate::scraper::scraper_service::service::{FetchError, ScraperServiceImpl};
use crate::scraper::scraper_service::util::hash::{hash_html, hash_main_fragment};
use crate::scraper::scraper_service::util::html::extract_main_fragment;
use crate::spider::classification::url_metadata::UrlPresence;
use crate::spider::utils::url::CrawledUrl;
use listing_source_core::ListingSourceId;
use regex::Regex;
use tracing::{debug, warn};
use url::Url;

pub(crate) fn is_redirect_to_non_product_page(
    original_url: &Url,
    final_url: &Url,
    product_url_pattern: Option<&str>,
) -> bool {
    let normalized_original = CrawledUrl::new(original_url.clone());
    let normalized_final = CrawledUrl::new(final_url.clone());

    if normalized_original.as_url() == normalized_final.as_url() {
        return false;
    }

    if !is_same_logical_host(original_url, final_url) {
        return false;
    }

    if let Some(pattern) = product_url_pattern.and_then(|raw| Regex::new(raw).ok()) {
        return !normalized_final.matches_pattern(&pattern);
    }

    is_homepage(final_url)
}

fn is_same_logical_host(left: &Url, right: &Url) -> bool {
    normalize_host(left) == normalize_host(right)
}

fn normalize_host(url: &Url) -> Option<&str> {
    url.host_str()
        .map(|host| host.strip_prefix("www.").unwrap_or(host))
}

fn is_homepage(url: &Url) -> bool {
    url.path().is_empty() || url.path() == "/"
}

impl ScraperServiceImpl {
    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id, url = %url))]
    pub(crate) async fn mark_product_removed_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) {
        if let Err(err) = self
            .candidate_service
            .set_presence(listing_source_id, url, UrlPresence::Withdrawn)
            .await
        {
            warn!(error = ?err, "Failed to mark product as withdrawn");
        }
    }

    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id, url = %url))]
    pub(crate) async fn mark_product_present_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) {
        if let Err(err) = self
            .candidate_service
            .set_presence(listing_source_id, url, UrlPresence::Present)
            .await
        {
            warn!(error = ?err, "Failed to mark product as PRESENT");
        }
    }

    #[tracing::instrument(skip(self), fields(listing_source_id = %listing_source_id, url = %url))]
    pub(crate) async fn mark_url_other_best_effort(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) {
        if let Err(err) = self
            .candidate_service
            .set_class(
                listing_source_id,
                url,
                crate::spider::classification::url_metadata::UrlClass::Other,
            )
            .await
        {
            warn!(error = ?err, "Failed to mark URL as other");
        }
    }

    #[allow(clippy::result_large_err)]
    async fn removed_page_schemas_for(
        &self,
        listing_source_id: &ListingSourceId,
    ) -> Result<Vec<RemovedPageSchema>, ScraperError> {
        let mut schemas = Vec::new();

        if let Some(stored) = self
            .removed_page_schema_repository
            .find_removed_page_schema(listing_source_id)
            .await
            .map_err(ScraperError::RemovedPageSchemaDatabaseError)?
        {
            schemas.extend(stored.removed_page_schemas);
        }

        Ok(schemas)
    }

    #[allow(clippy::result_large_err)]
    async fn is_removed_page(
        &self,
        listing_source_id: &ListingSourceId,
        html: &str,
    ) -> Result<bool, ScraperError> {
        Ok(self
            .removed_page_schemas_for(listing_source_id)
            .await?
            .iter()
            .any(|schema| schema.matches(html)))
    }
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    #[tracing::instrument(skip(self, last_scraped_hash), fields(listing_source_id = %listing_source_id, url = %url))]
    async fn scrape(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
        product_url_pattern: Option<&str>,
        last_scraped_hash: Option<&str>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let domain = url
            .host_str()
            .ok_or_else(|| ScraperError::NoHost { url: url.clone() })?;

        if let Some(review_id) = self
            .pending_product_schema_review_id(listing_source_id)
            .await?
        {
            return Err(ScraperError::PendingSchemaReview {
                url: url.clone(),
                review_id,
            });
        }

        // 1. Fetch HTML --------------------------------------------------
        debug!(domain, "Fetching product page HTML");
        let fetched = match self.html_fetcher.fetch(url).await {
            Ok(fetched) => fetched,
            Err(FetchError::Network {
                kind: NetworkErrorKind::HttpStatus(404 | 410),
                details,
            }) => {
                self.mark_product_removed_best_effort(listing_source_id, url)
                    .await;
                return Err(ScraperError::ProductListingRemoved {
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
        if is_redirect_to_non_product_page(url, &fetched.final_url, product_url_pattern) {
            self.mark_product_removed_best_effort(listing_source_id, url)
                .await;
            return Err(ScraperError::ProductListingRemoved {
                url: url.clone(),
                details: format!(
                    "product URL redirected to non-product page: original={url}, final={}",
                    fetched.final_url
                ),
            });
        }
        let html = fetched.html;

        if self.is_removed_page(listing_source_id, &html).await? {
            self.mark_product_removed_best_effort(listing_source_id, url)
                .await;
            return Err(ScraperError::ProductListingRemoved {
                url: url.clone(),
                details: "soft-404 removed page matched configured removed-page schema".to_string(),
            });
        }

        let has_main = extract_main_fragment(&html).is_some();
        let current_hash = hash_main_fragment(&html).unwrap_or_else(|| hash_html(&html));

        if has_main && last_scraped_hash == Some(current_hash.as_str()) {
            debug!("Hash matches last scraped hash, skipping extraction.");
            if let Err(e) = self
                .candidate_service
                .touch_scraped(listing_source_id, url, &current_hash)
                .await
            {
                warn!(error = %e, "Failed to touch url as scraped after hash-match skip");
            }
            return Ok(None);
        }

        // 2. Obtain schemas (from DB or freshly created by LLM) -----------
        let listing_source_product_schemas = self
            .obtain_schemas(listing_source_id, url, product_url_pattern, &html)
            .await?;

        // 3. Select the richest cached schema that normalizes successfully.
        let final_product = match self
            .select_existing_schema_with_normalization(
                listing_source_id,
                url,
                &html,
                &listing_source_product_schemas.product_schemas,
            )
            .await?
        {
            ExistingSchemaSelection::Normalized(product) => *product,
            ExistingSchemaSelection::GenerateNewSchema { reason } => {
                debug!(
                    domain,
                    schemas = listing_source_product_schemas.product_schemas.len(),
                    fresh_schema_generation_reason = reason.as_str(),
                    "Cached selection exhausted; generating new schema"
                );
                self.generate_fresh_schema_for_page(FreshSchemaGenerationContext {
                    listing_source_id,
                    domain,
                    url,
                    html: &html,
                    existing_schemas: &listing_source_product_schemas.product_schemas,
                })
                .await?
            }
        };

        // 4. Bookkeeping ------------------------------------------------
        self.mark_product_present_best_effort(listing_source_id, url)
            .await;

        // `mark_as_scraped` is intentionally NOT called here.  The caller
        // (cron pipeline) must call it only after the push to the product
        // backend has been confirmed, so that a failed push is retried on
        // the next cycle.
        let snapshot = ProductListingSnapshot::from_normalized(&final_product);

        debug!(
            domain,
            source_listing_id = %final_product.source_listing_id,
            "Scraping complete"
        );
        Ok(Some(ScrapedProduct {
            product: final_product,
            hash: current_hash,
            snapshot,
        }))
    }
}
