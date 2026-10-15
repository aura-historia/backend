//! Current shipping enrichment for public listing reads. Buyer state never enters item data.
use crate::{
    use_cases::{PersonalizedProductListingDetailsView, PersonalizedProductListingSummary},
    user_state::ProductListingUserState,
};
use application::operation_context::OperationContext;
use geo::GeographicDescription;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use shipping_core::assessment::{EffectiveDeliveryTerms, ShippingAssessment};
use shipping_service::assessment::{
    AssertionEvidence, AssessDeliveryRequest, AssessDeliveryUseCase, AssessListingRequest,
    ProfileAssessmentBasis,
};
use std::{collections::HashMap, sync::Arc};
use time::OffsetDateTime;

pub use shipping_service::assessment::AssessmentError as ShippingReadError;

#[derive(Debug, Clone, PartialEq)]
pub struct AdvertisedShipping {
    pub cache_until: Option<OffsetDateTime>,
    pub effective: EffectiveDeliveryTerms,
    pub shipping_reasons: Vec<shipping_core::assessment::AssessmentReason>,
    pub collection_reasons: Vec<shipping_core::assessment::AssessmentReason>,
    pub listing_revision: u64,
    pub source_revision: u64,
    pub default_revision: Option<u64>,
    pub profile: Option<ProfileAssessmentBasis>,
    pub listing_evidence: Option<AssertionEvidence>,
    pub evaluated_at: OffsetDateTime,
    pub notices: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PersonalizedShipping {
    pub assessment: ShippingAssessment,
    /// Minimal area facts only; never a street address, point or saved-address identity.
    pub destination: GeographicDescription,
    pub attribution: Option<DestinationAttribution>,
    pub evaluated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DestinationAttribution {
    pub provider: String,
    pub attribution_url: String,
}

/// Required by listing use cases; runtime composition supplies the current shipping service.
#[derive(Clone)]
pub struct ShippingEnrichment {
    assessments: Arc<dyn AssessDeliveryUseCase>,
}
struct ShippingView {
    advertised: AdvertisedShipping,
    personalized: Option<PersonalizedShipping>,
}
impl ShippingEnrichment {
    pub fn new(assessments: Arc<dyn AssessDeliveryUseCase>) -> Self {
        Self { assessments }
    }

    /// Existing use-case unit tests isolate unrelated shipping reads. Integration tests use
    /// the real reader and evaluator through ordinary runtime composition.
    #[cfg(any(test, feature = "test-data"))]
    pub fn for_tests() -> Self {
        Self::new(Arc::new(test_support::NoAssessments))
    }

    async fn load(
        &self,
        context: &OperationContext,
        ids: Vec<ProductListingId>,
    ) -> Result<HashMap<ProductListingId, ShippingView>, ShippingReadError> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let view = self
            .assessments
            .execute(
                context,
                AssessDeliveryRequest {
                    destination: None,
                    listings: ids
                        .into_iter()
                        .map(|listing_id| AssessListingRequest {
                            listing_id,
                            // Browsing establishes neither a purchase quantity nor a shipment/order total.
                            input: Default::default(),
                        })
                        .collect(),
                },
            )
            .await?;
        let attribution = view
            .destination
            .attribution
            .map(|a| DestinationAttribution {
                provider: a.provider,
                attribution_url: a.attribution_url,
            });
        Ok(view
            .items
            .into_iter()
            .map(|item| {
                let personalized =
                    view.destination
                        .effective
                        .as_ref()
                        .map(|destination| PersonalizedShipping {
                            assessment: item.assessment,
                            destination: destination.clone(),
                            attribution: attribution.clone(),
                            evaluated_at: item.evaluated_at,
                        });
                (
                    item.listing_id,
                    ShippingView {
                        advertised: AdvertisedShipping {
                            cache_until: item.cache_until,
                            effective: item.effective,
                            shipping_reasons: item.generic_assessment.reasons,
                            collection_reasons: item.generic_assessment.collection.reasons,
                            listing_revision: item.listing_revision,
                            source_revision: item.source_revision,
                            default_revision: item.default_revision,
                            profile: item.profile,
                            listing_evidence: item.listing_evidence,
                            evaluated_at: item.evaluated_at,
                            notices: item.notices,
                        },
                        personalized,
                    },
                )
            })
            .collect())
    }

    pub async fn details(
        &self,
        context: &OperationContext,
        items: &mut [PersonalizedProductListingDetailsView],
    ) -> Result<(), ShippingReadError> {
        let ids = items
            .iter()
            .filter(|p| eligible(p.item.lifecycle, p.user_state.as_ref()))
            .map(|p| p.item.product_listing_id)
            .collect();
        let mut shipping = self.load(context, ids).await?;
        for item in items {
            item.item.shipping = None;
            if let Some(state) = &mut item.user_state {
                state.shipping = None;
            }
            if let Some(view) = shipping.remove(&item.item.product_listing_id) {
                item.item.shipping = Some(view.advertised);
                if let Some(state) = &mut item.user_state {
                    state.shipping = view.personalized;
                }
            }
        }
        Ok(())
    }

    pub async fn summaries(
        &self,
        context: &OperationContext,
        items: &mut [PersonalizedProductListingSummary],
    ) -> Result<(), ShippingReadError> {
        let ids = items
            .iter()
            .filter(|p| eligible(p.item.lifecycle, p.user_state.as_ref()))
            .map(|p| p.item.product_listing_id)
            .collect();
        let mut shipping = self.load(context, ids).await?;
        for item in items {
            item.item.shipping = None;
            if let Some(state) = &mut item.user_state {
                state.shipping = None;
            }
            if let Some(view) = shipping.remove(&item.item.product_listing_id) {
                item.item.shipping = Some(view.advertised);
                if let Some(state) = &mut item.user_state {
                    state.shipping = view.personalized;
                }
            }
        }
        Ok(())
    }
}
fn eligible(lifecycle: ListingLifecycle, state: Option<&ProductListingUserState>) -> bool {
    lifecycle == ListingLifecycle::Active && state.is_none_or(|s| !s.search_filter.hidden)
}

#[cfg(any(test, feature = "test-data"))]
mod test_support {
    use super::*;
    use shipping_service::assessment::{DeliveryAssessmentView, DestinationSnapshot};
    pub(super) struct NoAssessments;
    #[async_trait::async_trait]
    impl AssessDeliveryUseCase for NoAssessments {
        async fn execute(
            &self,
            _: &OperationContext,
            _: AssessDeliveryRequest,
        ) -> Result<DeliveryAssessmentView, ShippingReadError> {
            Ok(DeliveryAssessmentView {
                destination: DestinationSnapshot {
                    submitted: None,
                    derived: None,
                    effective: None,
                    attribution: None,
                },
                items: vec![],
            })
        }
    }
}
