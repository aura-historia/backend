use crate::network::policy::NetworkErrorKind;
use crate::scraper::candidate_service::ProductSnapshot;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::domain::product::{ScrapedProduct, ScraperService};
use crate::scraper::scraper_service::recovery::normalization_retry::{
    ExistingSchemaSelection, NormalizationRetryContext,
};
use crate::scraper::scraper_service::service::{FetchError, ScraperServiceImpl};
use crate::scraper::scraper_service::util::hash::{hash_html, hash_main_fragment};
use crate::scraper::scraper_service::util::html::extract_main_fragment;
use crate::spider::classification::url_metadata::UrlState;
use common::shop_id::ShopId;
use tracing::{debug, warn};
use url::Url;

impl ScraperServiceImpl {
    #[tracing::instrument(skip(self), fields(shop_id = %shop_id, url = %url))]
    pub(crate) async fn mark_product_removed_best_effort(&self, shop_id: &ShopId, url: &Url) {
        if let Err(err) = self
            .candidate_service
            .set_state(shop_id, url, UrlState::Removed)
            .await
        {
            warn!(error = ?err, "Failed to mark product as REMOVED");
        }
    }

    #[tracing::instrument(skip(self), fields(shop_id = %shop_id, url = %url, state = %state))]
    pub(crate) async fn persist_scraped_state_best_effort(
        &self,
        shop_id: &ShopId,
        url: &Url,
        state: UrlState,
    ) {
        if let Err(err) = self.candidate_service.set_state(shop_id, url, state).await {
            warn!(error = ?err, "Failed to persist scraped URL state");
        }
    }
}

#[async_trait::async_trait]
impl ScraperService for ScraperServiceImpl {
    #[tracing::instrument(skip(self, last_scraped_hash), fields(shop_id = %shop_id, url = %url))]
    async fn scrape(
        &self,
        shop_id: &ShopId,
        url: &Url,
        last_scraped_hash: Option<&str>,
    ) -> Result<Option<ScrapedProduct>, ScraperError> {
        let domain = url
            .host_str()
            .ok_or_else(|| ScraperError::NoHost { url: url.clone() })?;

        if let Some(review_id) = self.pending_product_schema_review_id(shop_id).await? {
            return Err(ScraperError::PendingSchemaReview {
                url: url.clone(),
                review_id,
            });
        }

        // 1. Fetch HTML --------------------------------------------------
        debug!(domain, "Fetching product page HTML");
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
            debug!("Hash matches last scraped hash, skipping extraction.");
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
        let shops_product_schema = self.obtain_schemas(shop_id, url, &html).await?;

        // 3. Select a schema only after extraction and normalization succeed.
        let final_product = match self
            .select_existing_schema_with_normalization(
                shop_id,
                domain,
                url,
                &html,
                &shops_product_schema.product_schemas,
            )
            .await?
        {
            ExistingSchemaSelection::Normalized(product) => *product,
            ExistingSchemaSelection::NeedsRepair {
                selected_schema,
                last_norm_error,
            } => {
                self.fix_normalization_with_schema_retry(
                    NormalizationRetryContext {
                        shop_id,
                        domain,
                        url,
                        html: &html,
                        existing_schemas: &shops_product_schema.product_schemas,
                        selected_schema: *selected_schema,
                    },
                    last_norm_error,
                )
                .await?
            }
            ExistingSchemaSelection::NoSchemaApplied { last_error } => {
                warn!(
                    domain,
                    schemas = shops_product_schema.product_schemas.len(),
                    error = ?last_error,
                    "No cached schema applied; generating new schema candidates"
                );
                let (selected_schema, raw, existing_schemas_for_norm) = self
                    .append_and_reapply_with_retry(
                        shop_id,
                        url,
                        &html,
                        &shops_product_schema.product_schemas,
                    )
                    .await?;

                debug!(domain, "Normalizing generated schema extraction");
                self.normalize_with_schema_fix_retry(
                    NormalizationRetryContext {
                        shop_id,
                        domain,
                        url,
                        html: &html,
                        existing_schemas: &existing_schemas_for_norm,
                        selected_schema,
                    },
                    raw,
                )
                .await?
            }
        };

        // 4. Bookkeeping ------------------------------------------------
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
            "Scraping complete"
        );
        Ok(Some(ScrapedProduct {
            product: final_product,
            hash: current_hash,
            snapshot,
        }))
    }
}
