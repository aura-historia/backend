//! Operator-authored product schemas: where LLM generation would run, the scraper opens a
//! pending review the operator completes in the review console.

use crate::review::model::SchemaReviewPageInput;
use crate::review::repository::CrawlerReviewRepository;
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::rule::{
    CssSelector, ExtractionCardinality, ExtractionKind, ExtractionRule,
};
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_review_gate::review_error_to_schema_service_error;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use serde_json::json;
use std::collections::BTreeMap;
use tracing::info;
use url::Url;

/// Review reason for a product schema an operator has to write because LLM calls are disabled.
pub const MANUAL_SCHEMA_REASON: &str = "manual_schema_required";

impl ScraperServiceImpl {
    /// Rejecting a manual review is the operator's verdict that its seed and triggering pages are
    /// not product pages, whichever schema extracts them. When a rejection covers `page_url`, the
    /// page the scrape fetched, classifies the crawler URL `requested_url` as other and fails with
    /// [`ScraperError::NotProductPage`], so the page is neither captured nor reopens the review;
    /// `Ok` when no operator rejected a manual review over it. A scrape checks its page before it
    /// obtains schemas, and again before it authors one, which a rejection given while the page
    /// was scraped must still stop.
    ///
    /// The two URLs differ after a redirect that moved the page. The requested row is the one
    /// classified, so the scrape does not request it again every pass; the redirect target's own
    /// row, if the spider stored one, is classified when it is scraped.
    ///
    /// `expected_last_captured_raw_input_sha256` is the scrape's guard for its writes to the
    /// requested row, as for any crawler URL write. Without it the classification of a page
    /// captured before is skipped and left to the page's next scrape.
    pub(crate) async fn skip_rejected_page(
        &self,
        listing_source_id: &ListingSourceId,
        requested_url: &Url,
        page_url: &Url,
        expected_last_captured_raw_input_sha256: Option<&[u8]>,
    ) -> Result<(), ScraperError> {
        let rejected = self
            .is_rejected_page(listing_source_id, page_url)
            .await
            .map_err(|error| {
                ScraperError::from(review_error_to_schema_service_error(error.into()))
            })?;
        if !rejected {
            return Ok(());
        }
        self.mark_url_other_best_effort(
            listing_source_id,
            requested_url,
            expected_last_captured_raw_input_sha256,
        )
        .await;
        Err(ScraperError::NotProductPage {
            url: requested_url.clone(),
            details: "an operator rejected the manual schema review for this page".to_owned(),
        })
    }

    /// Whether a rejected manual review covers the page of the crawler URL `url`, in any spelling
    /// of that page: the verdict that it is no product page. A rejection is persisted state of the
    /// ListingSource, so it applies in every authoring mode, also after a switch away from the
    /// mode that opened the review (`docs/crawler/operations.md#manual-authoring`). `false`
    /// without a review gate, which only tests omit.
    pub(crate) async fn is_rejected_page(
        &self,
        listing_source_id: &ListingSourceId,
        url: &Url,
    ) -> Result<bool, sqlx::Error> {
        let Some(review_repository) = &self.review_repository else {
            return Ok(false);
        };
        review_repository
            .has_rejected_schema_review_for_page(
                listing_source_id,
                MANUAL_SCHEMA_REASON,
                url.as_str(),
            )
            .await
    }

    /// Opens the pending manual schema review for `listing_source_id` and returns the error that
    /// makes the page wait until it is approved. A review that another worker opened meanwhile is
    /// joined instead. The caller has already checked [`Self::skip_rejected_page`].
    ///
    /// The candidate keeps `existing_schemas` and appends a starter schema for the operator to
    /// rewrite; approval replaces the ListingSource's schemas with the reviewed list.
    pub(crate) async fn open_manual_schema_review(
        &self,
        review_repository: &CrawlerReviewRepository,
        listing_source_id: &ListingSourceId,
        url: &Url,
        existing_schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
    ) -> ScraperError {
        let mut schemas = existing_schemas.to_vec();
        schemas.push(starter_schema());
        let validation_summary = json!({
            "manual_authoring": true,
            "starter_schema_index": existing_schemas.len(),
            "page_count": pages.len(),
        });
        match review_repository
            .create_schema_review(
                listing_source_id,
                MANUAL_SCHEMA_REASON,
                &schemas,
                pages,
                validation_summary,
            )
            .await
        {
            Ok(review_id) => {
                info!(
                    review_id = %review_id,
                    existing_schema_count = existing_schemas.len(),
                    "Opened manual product schema review; scraping waits for an operator schema"
                );
                ScraperError::PendingSchemaReview {
                    url: url.clone(),
                    review_id,
                }
            }
            Err(error) => review_error_to_schema_service_error(error).into(),
        }
    }
}

/// A valid placeholder the operator replaces: generic selectors that rarely fit a shop as-is.
fn starter_schema() -> ProductCssSelectorSchema {
    ProductCssSelectorSchema {
        source_listing_id: None,
        title: rule("h1", ExtractionKind::Text, ExtractionCardinality::First),
        description: None,
        price: None,
        price_estimate_min: None,
        price_estimate_max: None,
        state: rule(
            "button[type=submit]",
            ExtractionKind::Text,
            ExtractionCardinality::First,
        ),
        images: rule("img", ExtractionKind::ImageUrl, ExtractionCardinality::All),
        raw_attributes: BTreeMap::new(),
    }
}

fn rule(
    selector: &str,
    extract: ExtractionKind,
    cardinality: ExtractionCardinality,
) -> ExtractionRule {
    ExtractionRule {
        selector: CssSelector::from(selector.to_owned()),
        additional_selectors: Vec::new(),
        extract,
        cardinality,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_provide_valid_starter_schema() {
        assert!(ProductCssSelectorSchema::validate_all(&[starter_schema()]).is_ok());
    }
}
