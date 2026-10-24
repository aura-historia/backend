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
    /// Opens the pending manual schema review for `listing_source_id`, or joins the one already
    /// pending as a waiting page, and returns the error that makes the page wait until it is
    /// approved.
    ///
    /// The candidate keeps `existing_schemas` and appends a starter schema for the operator to
    /// rewrite; approval replaces the ListingSource's schemas with the reviewed list.
    ///
    /// Rejecting a manual review is the operator's verdict that its page is not a product
    /// page: the URL is then classified as other instead of reopening the same review.
    pub(crate) async fn open_manual_schema_review(
        &self,
        review_repository: &CrawlerReviewRepository,
        listing_source_id: &ListingSourceId,
        url: &Url,
        html: &str,
        existing_schemas: &[ProductCssSelectorSchema],
        pages: Vec<SchemaReviewPageInput>,
    ) -> ScraperError {
        match review_repository
            .has_rejected_schema_review_for_page(
                listing_source_id,
                MANUAL_SCHEMA_REASON,
                url.as_str(),
            )
            .await
        {
            Ok(true) => {
                self.mark_url_other_best_effort(listing_source_id, url, None)
                    .await;
                return ScraperError::NotProductPage {
                    url: url.clone(),
                    details: "an operator rejected the manual schema review for this page"
                        .to_owned(),
                };
            }
            Ok(false) => {}
            Err(error) => {
                return review_error_to_schema_service_error(error.into()).into();
            }
        }
        match self
            .join_pending_product_schema_review(listing_source_id, url, html)
            .await
        {
            Ok(Some(review_id)) => {
                return ScraperError::PendingSchemaReview {
                    url: url.clone(),
                    review_id,
                };
            }
            Ok(None) => {}
            Err(error) => return error.into(),
        }
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
