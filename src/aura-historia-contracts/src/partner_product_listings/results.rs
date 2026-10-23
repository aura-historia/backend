use listing_source_core::ListingSourceId;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use product_listing_service::use_cases::{
    CreateProductListingResult, UpdateProductListingResult, UpsertProductListingResult,
    WithdrawProductListingResult,
    commands::partner_product_listing_batches::ProductListingBatchResult,
};
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductListingBatchStatus {
    Completed,
    PartialSuccess,
    Failed,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProductListingBatchFailureCode {
    Forbidden,
    NotFound,
    Conflict,
    InvalidInput,
    TemporarilyUnavailable,
    InternalError,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingBatchReportData {
    pub status: ProductListingBatchStatus,
    pub input_count: usize,
    pub succeeded_count: usize,
    pub failed_count: usize,
    pub successes: Vec<ProductListingBatchSuccessData>,
    pub failures: Vec<ProductListingBatchFailureData>,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingBatchSuccessData {
    pub index: usize,
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: String,
    #[serde(flatten)]
    pub result: ProductListingBatchMutationData,
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "SCREAMING_SNAKE_CASE",
    rename_all_fields = "camelCase"
)]
pub enum ProductListingBatchMutationData {
    Created {
        #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
        product_listing_id: ProductListingId,
        #[schemars(with = "String")]
        product_listing_title_slug_id: ProductListingSlugId,
    },
    Updated {
        #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
        product_listing_id: ProductListingId,
        changed: bool,
    },
    Withdrawn {
        #[schemars(schema_with = "crate::wire::schema::product_listing_id")]
        product_listing_id: ProductListingId,
        changed: bool,
    },
}
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProductListingBatchFailureData {
    pub index: usize,
    #[schemars(schema_with = "crate::wire::schema::listing_source_id")]
    pub listing_source_id: ListingSourceId,
    pub source_listing_id: String,
    pub error: ProductListingBatchFailureCode,
}
impl ProductListingBatchReportData {
    pub fn from_result<T: Into<ProductListingBatchMutationData>, E>(
        source_ids: Vec<String>,
        result: ProductListingBatchResult<T, E>,
        error_code: impl Fn(E) -> ProductListingBatchFailureCode,
    ) -> Self {
        let input_count = result.items.len();
        let mut successes = Vec::new();
        let mut failures = Vec::new();
        for item in result.items {
            let source_listing_id = source_ids[item.index].clone();
            match item.result {
                Ok(result) => successes.push(ProductListingBatchSuccessData {
                    index: item.index,
                    listing_source_id: item.product_key.listing_source_id,
                    source_listing_id,
                    result: result.into(),
                }),
                Err(error) => failures.push(ProductListingBatchFailureData {
                    index: item.index,
                    listing_source_id: item.product_key.listing_source_id,
                    source_listing_id,
                    error: error_code(error),
                }),
            }
        }
        Self {
            status: if failures.is_empty() {
                ProductListingBatchStatus::Completed
            } else if successes.is_empty() {
                ProductListingBatchStatus::Failed
            } else {
                ProductListingBatchStatus::PartialSuccess
            },
            input_count,
            succeeded_count: successes.len(),
            failed_count: failures.len(),
            successes,
            failures,
        }
    }
}
impl From<CreateProductListingResult> for ProductListingBatchMutationData {
    fn from(value: CreateProductListingResult) -> Self {
        Self::Created {
            product_listing_id: value.product_listing_id,
            product_listing_title_slug_id: value.product_listing_title_slug_id,
        }
    }
}
impl From<UpdateProductListingResult> for ProductListingBatchMutationData {
    fn from(value: UpdateProductListingResult) -> Self {
        Self::Updated {
            product_listing_id: value.product_listing_id,
            changed: value.outcome == domain_primitives::change_outcome::ChangeOutcome::Changed,
        }
    }
}
impl From<UpsertProductListingResult> for ProductListingBatchMutationData {
    fn from(value: UpsertProductListingResult) -> Self {
        match value {
            UpsertProductListingResult::Created(value) => value.into(),
            UpsertProductListingResult::Updated(value) => value.into(),
        }
    }
}
impl From<WithdrawProductListingResult> for ProductListingBatchMutationData {
    fn from(value: WithdrawProductListingResult) -> Self {
        Self::Withdrawn {
            product_listing_id: value.product_listing_id,
            changed: value.outcome == domain_primitives::change_outcome::ChangeOutcome::Changed,
        }
    }
}
