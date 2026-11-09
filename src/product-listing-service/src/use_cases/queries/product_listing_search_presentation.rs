//! Presentation of ranked listing search results: listing-source summaries, the requester's
//! disclosed user state, content policy, advertised shipping and geographic distance.

use crate::ports::{
    ListingSourceSummaryReader, ProductListingContentAssessmentReader,
    ProductListingUserStateReader,
};
use crate::shipping::ShippingEnrichment;
use crate::use_cases::queries::product_listing_summary_personalization::{
    ProductListingSummaryPersonalizationError, apply_product_user_states, attach_listing_sources,
    listing_source_ids, product_listing_ids, product_listing_user_state_lookup,
};
use crate::use_cases::queries::search_product_listings::{
    PersonalizedProductListingSummary, ProductListingSearchItem,
    ProductListingSearchReadExecutionPolicy, ProductListingSummaryWithUserState,
    SearchProductListingsError, measure_search_stage, present_product_summaries_from_assessments,
    record_search_stage,
};
use crate::user_state::ProductListingUserStateDisclosure;
use application::operation_context::{OperationContext, Principal};
use application::personalized::Personalized;
use fxrate_core::FxRateSnapshot;
use product_listing_core::product_listing_search::ProductListingSearch;
use std::time::Instant;

pub struct ProductListingSearchPresentation<L, U, A> {
    listing_sources: L,
    user_states: U,
    assessments: A,
    shipping: ShippingEnrichment,
    read_execution_policy: ProductListingSearchReadExecutionPolicy,
}

impl<L, U, A> ProductListingSearchPresentation<L, U, A> {
    pub fn new(
        listing_sources: L,
        user_states: U,
        assessments: A,
        shipping: ShippingEnrichment,
    ) -> Self {
        Self {
            listing_sources,
            user_states,
            assessments,
            shipping,
            read_execution_policy: ProductListingSearchReadExecutionPolicy::Sequential,
        }
    }

    pub fn with_read_execution_policy(
        mut self,
        read_execution_policy: ProductListingSearchReadExecutionPolicy,
    ) -> Self {
        self.read_execution_policy = read_execution_policy;
        self
    }

    pub(crate) fn read_execution_policy(&self) -> ProductListingSearchReadExecutionPolicy {
        self.read_execution_policy
    }
}

impl<L, U, A> ProductListingSearchPresentation<L, U, A>
where
    L: ListingSourceSummaryReader,
    U: ProductListingUserStateReader,
    A: ProductListingContentAssessmentReader,
{
    /// Presents `items` in their given order. `search` and `snapshot` are the ones that
    /// retrieved them, so shipping and distance use the requested currency and origin.
    pub async fn present(
        &self,
        context: &OperationContext,
        items: Vec<ProductListingSearchItem>,
        search: &ProductListingSearch,
        snapshot: &FxRateSnapshot,
    ) -> Result<Vec<PersonalizedProductListingSummary>, SearchProductListingsError> {
        let mut items = if items.is_empty() {
            Vec::new()
        } else {
            let enrichment_started = Instant::now();
            let summaries = self.enrich(&context.principal, items).await?;
            record_search_stage("enrichment_and_presentation", enrichment_started, "success");
            summaries
        };
        self.shipping
            .summaries(context, &mut items, search.currency, snapshot)
            .await?;
        let disclosure = ProductListingUserStateDisclosure::for_principal(&context.principal);
        Ok(items
            .into_iter()
            .map(|mut item| {
                if let Some(geography) = &mut item.item.geography {
                    geography.set_distance(search);
                }
                disclosure.disclose(item)
            })
            .collect())
    }

    async fn enrich(
        &self,
        principal: &Principal,
        items: Vec<ProductListingSearchItem>,
    ) -> Result<Vec<ProductListingSummaryWithUserState>, SearchProductListingsError> {
        let source_ids = listing_source_ids(&items);
        let listing_ids = product_listing_ids(&items);
        let user_state_lookup = personalization_user_id(principal)
            .map(|user_id| product_listing_user_state_lookup(user_id, &listing_ids));

        match self.read_execution_policy {
            ProductListingSearchReadExecutionPolicy::Sequential => {
                let sources = measure_search_stage(
                    "source_resolution",
                    self.listing_sources.find_summaries(&source_ids),
                )
                .await
                .map_err(ProductListingSummaryPersonalizationError::from)?;
                let sourced_items = attach_listing_sources(items, &sources)?;
                let mut items = sourced_items
                    .into_iter()
                    .map(|item| Personalized {
                        item,
                        user_state: None,
                    })
                    .collect::<Vec<_>>();

                if let Some(lookup) = user_state_lookup.as_ref() {
                    let user_states = measure_search_stage(
                        "user_state_resolution",
                        self.user_states.find_for_user(lookup),
                    )
                    .await
                    .map_err(ProductListingSummaryPersonalizationError::from)?;
                    apply_product_user_states(&mut items, &user_states)?;
                }

                let assessments = measure_search_stage(
                    "assessment_resolution",
                    self.assessments.find_current_assessments(&listing_ids),
                )
                .await?;
                Ok(present_product_summaries_from_assessments(
                    items,
                    &assessments,
                ))
            }
            ProductListingSearchReadExecutionPolicy::Concurrent => {
                let (source_result, user_state_result, assessment_result) = tokio::join!(
                    measure_search_stage(
                        "source_resolution",
                        self.listing_sources.find_summaries(&source_ids),
                    ),
                    async {
                        match user_state_lookup.as_ref() {
                            Some(lookup) => measure_search_stage(
                                "user_state_resolution",
                                self.user_states.find_for_user(lookup),
                            )
                            .await
                            .map(Some),
                            None => Ok(None),
                        }
                    },
                    measure_search_stage(
                        "assessment_resolution",
                        self.assessments.find_current_assessments(&listing_ids),
                    ),
                );
                let sources =
                    source_result.map_err(ProductListingSummaryPersonalizationError::from)?;
                let sourced_items = attach_listing_sources(items, &sources)?;
                let mut items = sourced_items
                    .into_iter()
                    .map(|item| Personalized {
                        item,
                        user_state: None,
                    })
                    .collect::<Vec<_>>();

                if let Some(user_states) = user_state_result
                    .map_err(ProductListingSummaryPersonalizationError::from)?
                    .as_ref()
                {
                    apply_product_user_states(&mut items, user_states)?;
                }

                let assessments = assessment_result?;
                Ok(present_product_summaries_from_assessments(
                    items,
                    &assessments,
                ))
            }
        }
    }
}

fn personalization_user_id(principal: &Principal) -> Option<user_core::user_id::UserId> {
    match principal {
        Principal::User(user_id) | Principal::DelegatedUser { user_id, .. } => Some(*user_id),
        Principal::Anonymous | Principal::Service(_) | Principal::System => None,
    }
}
