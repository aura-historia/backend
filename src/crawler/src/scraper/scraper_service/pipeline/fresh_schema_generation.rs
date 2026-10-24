use crate::review::model::{PAGE_ROLE_TRIGGERING_GENERATION_PAGE, SchemaReviewPageInput};
use crate::scraper::css_selector::product_schema::{
    ListingSourceProductSchema, ProductCssSelectorSchema,
};
use crate::scraper::plausibility::PlausibilityFindings;
use crate::scraper::scraper_service::domain::errors::ScraperError;
use crate::scraper::scraper_service::extraction::schema_review_gate::GeneratedSchemaReviewOutcome;
use crate::scraper::scraper_service::image_validation::filter_valid_image_urls;
use crate::scraper::scraper_service::pipeline::cached_schema_selection::SelectedSchemaCandidate;
use crate::scraper::scraper_service::service::ScraperServiceImpl;
use listing_source_core::ListingSourceId;
use serde_json::json;
use tracing::debug;
use url::Url;

pub(crate) struct FreshSchemaGenerationContext<'a> {
    pub(crate) listing_source_id: &'a ListingSourceId,
    pub(crate) domain: &'a str,
    pub(crate) url: &'a Url,
    pub(crate) html: &'a str,
    pub(crate) existing_schemas: &'a [ProductCssSelectorSchema],
    pub(crate) expected_last_captured_raw_input_sha256: Option<&'a [u8]>,
}

impl ScraperServiceImpl {
    /// Makes a page that failed every cached schema wait instead of authoring a schema: in manual
    /// mode a page an operator rejected is classified as other, and in every mode the page joins a
    /// pending product-schema review. Returns `Ok` when the page may author a fresh schema.
    pub(crate) async fn wait_instead_of_authoring(
        &self,
        ctx: &FreshSchemaGenerationContext<'_>,
    ) -> Result<(), ScraperError> {
        if let Some(review_repository) = self.manual_schema_review_repository()
            && let Some(error) = self
                .rejected_manual_page(review_repository, ctx.listing_source_id, ctx.url)
                .await
        {
            return Err(error);
        }
        match self
            .join_pending_product_schema_review(ctx.listing_source_id, ctx.url, ctx.html)
            .await?
        {
            Some(review_id) => Err(ScraperError::PendingSchemaReview {
                url: ctx.url.clone(),
                review_id,
            }),
            None => Ok(()),
        }
    }

    /// The ListingSource's live product schemas when they differ from `read`. Only an approved
    /// review changes them while a page is scraped, so without a review gate they never differ.
    pub(crate) async fn product_schemas_changed_since(
        &self,
        listing_source_id: &ListingSourceId,
        read: &[ProductCssSelectorSchema],
    ) -> Result<Option<ListingSourceProductSchema>, ScraperError> {
        if self.review_repository.is_none() {
            return Ok(None);
        }
        let live = self
            .schema_service
            .find_product_schema(listing_source_id)
            .await?;
        Ok(live.filter(|live| live.product_schemas != read))
    }

    /// Authors a fresh schema for a page that failed every cached schema: opens a manual review in
    /// manual mode, otherwise generates one with the LLM. The caller has already let the page
    /// [wait](Self::wait_instead_of_authoring) where it must.
    #[tracing::instrument(
        skip(self, ctx),
        fields(
            listing_source_id = %ctx.listing_source_id,
            domain = ctx.domain,
            url = %ctx.url,
            schema_count = ctx.existing_schemas.len()
        )
    )]
    pub(crate) async fn generate_fresh_schema_for_page(
        &self,
        ctx: FreshSchemaGenerationContext<'_>,
    ) -> Result<SelectedSchemaCandidate, ScraperError> {
        if let Some(review_repository) = self.manual_schema_review_repository() {
            return Err(self
                .open_manual_schema_review(
                    review_repository,
                    ctx.listing_source_id,
                    ctx.url,
                    ctx.existing_schemas,
                    vec![triggering_page(&ctx)],
                )
                .await);
        }

        let (generated_schema, reapplied, evaluation) = self
            .generate_single_schema_for_page(
                ctx.listing_source_id,
                ctx.url,
                ctx.html,
                ctx.expected_last_captured_raw_input_sha256,
            )
            .await?;

        let findings = PlausibilityFindings::of(&reapplied);
        if findings.rejects_candidate() {
            return Err(ScraperError::FreshSchemaImplausible {
                url: ctx.url.clone(),
                findings,
            });
        }
        let validated_image_urls =
            filter_valid_image_urls(reapplied.images.clone(), ctx.url, &*self.image_validator)
                .await;

        let mut persisted_schemas = ctx.existing_schemas.to_vec();
        persisted_schemas.push(generated_schema.clone());
        let pages = vec![triggering_page(&ctx)];
        match self
            .handle_generated_schema_review(
                ctx.listing_source_id,
                "fresh_schema_generation",
                persisted_schemas,
                evaluation,
                pages,
                json!({ "schema_applied": true, "fresh_generation": true }),
            )
            .await?
        {
            GeneratedSchemaReviewOutcome::Persisted(_) => {
                debug!(
                    domain = ctx.domain,
                    url = %ctx.url,
                    plausibility_findings = %findings,
                    "Freshly generated schema produced a plausible product"
                );
                Ok(SelectedSchemaCandidate {
                    raw: reapplied,
                    validated_image_urls,
                    schema: generated_schema,
                    fresh_schema: true,
                })
            }
            GeneratedSchemaReviewOutcome::PendingReview(review_id) => {
                Err(ScraperError::PendingSchemaReview {
                    url: ctx.url.clone(),
                    review_id,
                })
            }
        }
    }
}

/// The page whose cached schemas were exhausted, as review evidence.
fn triggering_page(ctx: &FreshSchemaGenerationContext<'_>) -> SchemaReviewPageInput {
    SchemaReviewPageInput {
        url: ctx.url.to_string(),
        role: PAGE_ROLE_TRIGGERING_GENERATION_PAGE.to_string(),
        raw_html: ctx.html.to_string(),
    }
}
