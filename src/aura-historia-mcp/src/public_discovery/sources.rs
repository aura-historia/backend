use super::{errors, read_policy};
use crate::{InvocationContext, OperationDefinition, OperationError, RegistryError};
use application::error::static_error;
use aura_historia_contracts::listing_sources::{
    GetPublicListingSourceBySlugData, PublicListingSourceData,
    PublicListingSourceSearchCollectionData, SearchPublicListingSourcesData,
    cursor::encode_continuation,
};
use aura_historia_runtime::{PublicListingSourceReadBudget, PublicListingSources};
use std::future::Future;

pub(super) fn operations(
    services: PublicListingSources,
    budget: PublicListingSourceReadBudget,
) -> Result<Vec<OperationDefinition>, RegistryError> {
    let detail_budget = budget.clone();
    let search = OperationDefinition::new(
        "searchPublicListingSources",
        "Search listing sources",
        "Search the providers whose listings Aura Historia indexes (dealers, auction houses, marketplaces and auction platforms) by name. Without a query it browses all providers; a query too short to match returns no results. Returns 1 to 50 providers per page (default 21); pass the returned searchAfter unchanged, with the same query and size, to get the next page.",
        read_policy(),
        move |input: SearchPublicListingSourcesData, invocation| {
            let search = services.search.clone();
            let budget = budget.clone();
            async move {
                let request = input
                    .try_into()
                    .map_err(|source| OperationError::InvalidArguments(Box::new(source)))?;
                let result = execute(
                    &budget,
                    &invocation,
                    search.execute(&invocation.operation, request),
                )
                .await?
                .map_err(errors::search)?;
                let search_after = result
                    .continuation
                    .as_ref()
                    .map(encode_continuation)
                    .transpose()
                    .map_err(|source| OperationError::Internal(Box::new(source)))?;
                Ok(PublicListingSourceSearchCollectionData::new(
                    result,
                    search_after,
                ))
            }
        },
    )?;
    let detail = OperationDefinition::new(
        "getPublicListingSourceBySlug",
        "Get listing source",
        "Get the public profile of one provider (a dealer, auction house, marketplace or auction platform) by its listingSourceSlugId, the last segment of an Aura Historia provider page URL (/shops/{slug}): name, operator, website and image.",
        read_policy(),
        move |input: GetPublicListingSourceBySlugData, invocation| {
            let get = services.get_by_slug.clone();
            let budget = detail_budget.clone();
            async move {
                let request = input
                    .try_into()
                    .map_err(|source| OperationError::InvalidArguments(Box::new(source)))?;
                let result = execute(
                    &budget,
                    &invocation,
                    get.execute(&invocation.operation, request),
                )
                .await?
                .map_err(errors::detail)?;
                Ok(PublicListingSourceData::from(result))
            }
        },
    )?;
    Ok(vec![search, detail])
}

async fn execute<T>(
    budget: &PublicListingSourceReadBudget,
    invocation: &InvocationContext,
    work: impl Future<Output = T>,
) -> Result<T, OperationError> {
    let _permit = budget.try_acquire().ok_or_else(|| {
        OperationError::Unavailable(static_error("public source reads are at capacity"))
    })?;
    invocation
        .execute(tokio::time::timeout(budget.request_timeout(), work))
        .await?
        .map_err(|source| OperationError::Unavailable(Box::new(source)))
}
