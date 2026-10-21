//! Module-local public listing registrations. Release composition selects this family.
pub(crate) mod error;
#[cfg(test)]
mod tests;

use crate::{
    AccessClass, InvocationContext, OperationDefinition, OperationError, OperationPolicy,
    RegistryError,
};
use aura_historia_contracts::product_listings::{
    presentation::ProductListingPresentationData,
    product_data::{
        PersonalizedProductListingDetailsData, PersonalizedProductListingSummaryData,
        personalized_product_details_data, personalized_product_summary_data,
    },
    search::{
        CursoredProductListingsData, ProductListingSearchData, ProductListingSearchOptionsData,
    },
};
use aura_historia_runtime::PublicProductListings;
use product_listing_core::{
    product_listing_id::ProductListingId, product_listing_slug_id::ProductListingSlugId,
};
use product_listing_service::{
    ports::ProductListingEmbeddingLookup,
    use_cases::{
        GetProductListingRequest, GetSimilarProductListingsRequest,
        GetSimilarProductListingsResult, ProductListingLookup,
    },
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchInput {
    #[serde(flatten)]
    search: ProductListingSearchData,
    #[serde(flatten)]
    options: ProductListingSearchOptionsData,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListingInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::product_listing_id")]
    product_listing_id: ProductListingId,
    #[serde(flatten)]
    presentation: ProductListingPresentationData,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlugInput {
    #[schemars(schema_with = "aura_historia_contracts::wire::schema::slug")]
    product_listing_title_slug_id: ProductListingSlugId,
    #[serde(flatten)]
    presentation: ProductListingPresentationData,
}
/// The REST pending response has no body. MCP needs an explicit result distinct from
/// a completed computation with zero neighbors; the service's fixed neighbor limit applies.
#[derive(Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "SCREAMING_SNAKE_CASE")]
enum SimilarOutput {
    EmbeddingPending,
    Ready {
        items: Vec<PersonalizedProductListingSummaryData>,
    },
}
fn policy() -> OperationPolicy {
    OperationPolicy {
        access: AccessClass::Public,
        required_scopes: vec![],
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    }
}
/// Build exactly the four existing listing operations without installing them globally.
pub fn operations(
    services: PublicProductListings,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    Ok(vec![
        search(services.search)?,
        get(services.get.clone())?,
        get_by_slug(services.get)?,
        similar(services.similar)?,
    ])
}
fn search(
    service: std::sync::Arc<dyn product_listing_service::use_cases::SearchProductListingsUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "simpleSearchProductListings",
        "Search public listings with the complete catalogue filters, presentation, sorting and FX-bound continuation. Optional Aura userState requires each section's read scope: watchlist:read, search-filters:read, notifications:read or users:read. Write scopes do not imply reads.",
        policy(),
        move |input: SearchInput, context| {
            let service = service.clone();
            async move {
                let request = input
                    .options
                    .into_request(input.search)
                    .map_err(|e| OperationError::InvalidArguments(Box::new(e)))?;
                let result = context
                    .execute(service.execute(&context.operation, request))
                    .await?
                    .map_err(error::search)?;
                Ok(CursoredProductListingsData::from(result))
            }
        },
    )
}
fn get(
    service: std::sync::Arc<dyn product_listing_service::use_cases::GetProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getProductListing",
        "Get the complete public listing details by internal ProductListing ID. Optional Aura userState requires each section's read scope: watchlist:read, search-filters:read, notifications:read or users:read. Write scopes do not imply reads.",
        policy(),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                details(
                    service,
                    context,
                    ProductListingLookup::ById(input.product_listing_id),
                    input.presentation,
                )
                .await
            }
        },
    )
}
fn get_by_slug(
    service: std::sync::Arc<dyn product_listing_service::use_cases::GetProductListingUseCase>,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getProductListingByTitleSlug",
        "Get the complete public listing details by its immutable title slug. Optional Aura userState requires each section's read scope: watchlist:read, search-filters:read, notifications:read or users:read. Write scopes do not imply reads.",
        policy(),
        move |input: SlugInput, context| {
            let service = service.clone();
            async move {
                details(
                    service,
                    context,
                    ProductListingLookup::ByTitleSlug(input.product_listing_title_slug_id),
                    input.presentation,
                )
                .await
            }
        },
    )
}
async fn details(
    service: std::sync::Arc<dyn product_listing_service::use_cases::GetProductListingUseCase>,
    context: InvocationContext,
    lookup: ProductListingLookup,
    presentation: ProductListingPresentationData,
) -> Result<PersonalizedProductListingDetailsData, OperationError> {
    context
        .execute(service.execute(
            &context.operation,
            GetProductListingRequest {
                lookup,
                language: presentation.language,
                currency: presentation.currency,
            },
        ))
        .await?
        .map(personalized_product_details_data)
        .map_err(error::details)
}
fn similar(
    service: std::sync::Arc<
        dyn product_listing_service::use_cases::GetSimilarProductListingsUseCase,
    >,
) -> Result<OperationDefinition, RegistryError> {
    OperationDefinition::new(
        "getSimilarProductListings",
        "Find similar public listings using the fixed service neighbor limit. EMBEDDING_PENDING means retry this operation later; READY may contain an empty list. Optional Aura userState requires each section's read scope: watchlist:read, search-filters:read, notifications:read or users:read. Write scopes do not imply reads.",
        policy(),
        move |input: ListingInput, context| {
            let service = service.clone();
            async move {
                let result = context
                    .execute(service.execute(
                        &context.operation,
                        GetSimilarProductListingsRequest {
                            lookup: ProductListingEmbeddingLookup::ById(input.product_listing_id),
                            language: input.presentation.language,
                            currency: input.presentation.currency,
                        },
                    ))
                    .await?
                    .map_err(error::similar)?;
                Ok(match result {
                    GetSimilarProductListingsResult::EmbeddingPending => {
                        SimilarOutput::EmbeddingPending
                    }
                    GetSimilarProductListingsResult::Ready(items) => SimilarOutput::Ready {
                        items: items
                            .into_iter()
                            .map(personalized_product_summary_data)
                            .collect(),
                    },
                })
            }
        },
    )
}
