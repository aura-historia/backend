//! The review console's answers as `review-mcp` decodes them. Where the console answers with a
//! review model that decodes, that model is the wire type; the rest is declared here with only the
//! keys the tools read. The tools map these to their own output types in
//! [`super::views`]; nothing here reaches an agent as is.

use crate::review::model::{
    CrawlerReview, CrawlerReviewPage, CrawlerReviewUrl, SchemaMatrix, UrlPatternReviewCandidate,
};
use crate::scraper::css_selector::product_schema::ProductCssSelectorSchema;
use crate::scraper::css_selector::removed_page_schema::RemovedPageSchema;
use crate::{CrawlerReviewId, CrawlerReviewPageId};
use serde::Deserialize;

/// `GET /api/reviews/{id}`.
#[derive(Debug, Deserialize)]
pub(super) struct ReviewDetail {
    pub(super) review: CrawlerReview,
    pub(super) candidate_is_live: bool,
    pub(super) pages: Vec<CrawlerReviewPage>,
    pub(super) urls: Vec<CrawlerReviewUrl>,
    pub(super) events: Vec<ReviewEvent>,
}

/// One event of `GET /api/reviews/{id}`; its details stay with the console.
#[derive(Debug, Deserialize)]
pub(super) struct ReviewEvent {
    pub(super) actor: String,
    pub(super) action: String,
    pub(super) candidate_version: i64,
    pub(super) notes: Option<String>,
    pub(super) created: String,
}

/// A product-schema review's candidate as the console stores it.
#[derive(Debug, Clone, Deserialize)]
pub(super) struct ProductSchemaCandidate {
    pub(super) schemas: Vec<ProductCssSelectorSchema>,
    #[serde(default)]
    pub(super) removed_page_schemas: Vec<RemovedPageSchema>,
}

/// The candidate of `artifact_type`'s review, decoded from its stored payload.
pub(super) enum Candidate {
    ProductSchema(ProductSchemaCandidate),
    UrlPattern(UrlPatternReviewCandidate),
}

/// `GET /api/reviews/{id}/matrix`: the matrix and the review pages it did not evaluate.
#[derive(Debug, Deserialize)]
pub(super) struct ReviewMatrix {
    #[serde(flatten)]
    pub(super) matrix: SchemaMatrix,
    #[serde(default)]
    pub(super) not_evaluated_pages: Vec<CrawlerReviewPageId>,
}

/// The `202` of a matrix refresh or a candidate evaluation still running.
#[derive(Debug, Deserialize)]
pub(super) struct StillRunning {
    pub(super) status: String,
    #[serde(default)]
    pub(super) evaluation_id: Option<String>,
}

/// One domain of `GET /api/listing-sources/{id}/domains`.
#[derive(Debug, Deserialize)]
pub(super) struct ListingSourceDomain {
    pub(super) domain: String,
}

/// `GET /api/reviews/{id}/page-projection`.
#[derive(Debug, Deserialize)]
pub(super) struct PageProjection {
    pub(super) review_id: CrawlerReviewId,
    pub(super) review_page_id: Option<CrawlerReviewPageId>,
    pub(super) url: String,
    pub(super) final_url: String,
    pub(super) projection: String,
    pub(super) describes_one_product: bool,
}

/// The answer of an accepted candidate edit.
#[derive(Debug, Deserialize)]
pub(super) struct CandidateEdited {
    pub(super) candidate_version: i64,
}
