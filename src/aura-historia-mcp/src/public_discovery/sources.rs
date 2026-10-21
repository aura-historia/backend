use super::{anonymous_context, errors, read_policy};
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
        "Browse or search public listing sources by name, with query-bound cursor pagination and page sizes from 1 through 50.",
        read_policy(),
        move |input: SearchPublicListingSourcesData, invocation| {
            let search = services.search.clone();
            let budget = budget.clone();
            async move {
                let request = input
                    .try_into()
                    .map_err(|source| OperationError::InvalidArguments(Box::new(source)))?;
                let context = anonymous_context(&invocation);
                let result = execute(&budget, &invocation, search.execute(&context, request))
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
        "Read the public listing-source card by its canonical slug.",
        read_policy(),
        move |input: GetPublicListingSourceBySlugData, invocation| {
            let get = services.get_by_slug.clone();
            let budget = detail_budget.clone();
            async move {
                let request = input
                    .try_into()
                    .map_err(|source| OperationError::InvalidArguments(Box::new(source)))?;
                let context = anonymous_context(&invocation);
                let result = execute(&budget, &invocation, get.execute(&context, request))
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
