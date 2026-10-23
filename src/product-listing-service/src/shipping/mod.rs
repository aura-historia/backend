//! Current shipping enrichment for public listing reads. Buyer state never enters item data.
mod conversion;

use crate::{
    use_cases::{
        ProductListingDetailsView, ProductListingDetailsWithUserState, ProductListingSummary,
        ProductListingSummaryWithUserState,
    },
    user_state::ProductListingUserState,
};
use application::{operation_context::OperationContext, personalized::Personalized};
use conversion::ShippingConversion;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError};
use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use geo::GeographicDescription;
use money::Currency;
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use shipping_core::assessment::{EffectiveDeliveryTerms, ShippingAssessment};
use shipping_core::pricing::{ChargeBasis, ShippingPrice};
use shipping_service::assessment::{
    AssertionEvidence, AssessDeliveryRequest, AssessDeliveryUseCase, AssessListingRequest,
    DeliveryAssessmentView, ProfileAssessmentBasis,
};
use std::{collections::HashMap, sync::Arc};
use time::OffsetDateTime;

#[derive(Debug, thiserror::Error)]
pub enum ShippingReadError {
    #[error("shipping assessment failed")]
    Assessment(
        #[from]
        #[source]
        shipping_service::assessment::AssessmentError,
    ),
    #[error("shipping FX snapshot is missing")]
    FxSnapshotMissing,
    #[error("shipping FX snapshot read failed")]
    FxRead(
        #[from]
        #[source]
        FxRateSnapshotReadError,
    ),
    #[error("shipping conversion failed")]
    Conversion(
        #[from]
        #[source]
        FxRateSnapshotError,
    ),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShippingDisplay {
    pub currency: Currency,
    pub fx_rate_id: FxRateId,
    pub captured_at: OffsetDateTime,
    pub rates: Vec<ConvertedShippingRate>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConvertedShippingRate {
    pub zone_index: usize,
    pub service_index: usize,
    pub price: ShippingPrice,
    pub basis: Option<ChargeBasis>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AdvertisedShipping {
    pub display: Option<ShippingDisplay>,
    pub effective: EffectiveDeliveryTerms,
    pub shipping_reasons: Vec<shipping_core::assessment::AssessmentReason>,
    pub collection_reasons: Vec<shipping_core::assessment::AssessmentReason>,
    pub listing_revision: u64,
    pub source_revision: u64,
    pub default_revision: Option<u64>,
    pub profile: Option<ProfileAssessmentBasis>,
    pub listing_evidence: Option<AssertionEvidence>,
    pub geographic_valid_until: Option<OffsetDateTime>,
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
    fx_rates: Option<Arc<dyn FxRateSnapshotReader>>,
}
struct ShippingView {
    advertised: AdvertisedShipping,
    personalized: Option<PersonalizedShipping>,
}
impl ShippingEnrichment {
    pub fn new(
        assessments: Arc<dyn AssessDeliveryUseCase>,
        fx_rates: Arc<dyn FxRateSnapshotReader>,
    ) -> Self {
        Self {
            assessments,
            fx_rates: Some(fx_rates),
        }
    }

    /// Existing use-case unit tests isolate unrelated shipping reads. Integration tests use
    /// the real reader and evaluator through ordinary runtime composition.
    #[cfg(any(test, feature = "test-data"))]
    pub fn for_tests() -> Self {
        Self {
            assessments: Arc::new(test_support::NoAssessments),
            fx_rates: None,
        }
    }

    async fn load(
        &self,
        context: &OperationContext,
        ids: Vec<ProductListingId>,
        currency: Currency,
        pinned_snapshot: Option<&FxRateSnapshot>,
        valuation_at: OffsetDateTime,
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
        let mut shipping = shipping_views(view);
        if !shipping.is_empty() {
            let current_snapshot;
            let snapshot = match pinned_snapshot {
                Some(snapshot) => snapshot,
                None => {
                    current_snapshot = self.latest_snapshot(valuation_at).await?;
                    &current_snapshot
                }
            };
            let mut conversion = ShippingConversion::new(snapshot, currency);
            for view in shipping.values_mut() {
                view.advertised.display = Some(conversion.display(&view.advertised.effective)?);
            }
        }
        Ok(shipping)
    }

    async fn latest_snapshot(
        &self,
        valuation_at: OffsetDateTime,
    ) -> Result<FxRateSnapshot, ShippingReadError> {
        self.fx_rates
            .as_ref()
            .ok_or(ShippingReadError::FxSnapshotMissing)?
            .find_latest_at_or_before(valuation_at)
            .await?
            .ok_or(ShippingReadError::FxSnapshotMissing)
    }

    pub async fn details(
        &self,
        context: &OperationContext,
        items: &mut [ProductListingDetailsWithUserState],
        currency: Currency,
        snapshot: Option<&FxRateSnapshot>,
        valuation_at: OffsetDateTime,
    ) -> Result<(), ShippingReadError> {
        let shipping = self
            .load(
                context,
                eligible_ids(items),
                currency,
                snapshot,
                valuation_at,
            )
            .await?;
        apply(items, shipping);
        Ok(())
    }

    pub async fn summaries(
        &self,
        context: &OperationContext,
        items: &mut [ProductListingSummaryWithUserState],
        currency: Currency,
        snapshot: &FxRateSnapshot,
    ) -> Result<(), ShippingReadError> {
        let shipping = self
            .load(
                context,
                eligible_ids(items),
                currency,
                Some(snapshot),
                snapshot.captured_at(),
            )
            .await?;
        apply(items, shipping);
        Ok(())
    }
}

fn shipping_views(view: DeliveryAssessmentView) -> HashMap<ProductListingId, ShippingView> {
    let attribution = view
        .destination
        .attribution
        .map(|a| DestinationAttribution {
            provider: a.provider,
            attribution_url: a.attribution_url,
        });
    let destination = view.destination.effective;
    view.items
        .into_iter()
        .map(|item| {
            let personalized = destination
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
                        display: None,
                        effective: item.effective,
                        shipping_reasons: item.generic_assessment.reasons,
                        collection_reasons: item.generic_assessment.collection.reasons,
                        listing_revision: item.listing_revision,
                        source_revision: item.source_revision,
                        default_revision: item.default_revision,
                        profile: item.profile,
                        listing_evidence: item.listing_evidence,
                        geographic_valid_until: item.geographic_valid_until,
                        evaluated_at: item.evaluated_at,
                        notices: item.notices,
                    },
                    personalized,
                },
            )
        })
        .collect()
}

/// A listing read model that carries public advertised shipping.
trait ShippingReadItem {
    fn listing_id(&self) -> ProductListingId;
    fn lifecycle(&self) -> ListingLifecycle;
    fn shipping_mut(&mut self) -> &mut Option<AdvertisedShipping>;
}
impl ShippingReadItem for ProductListingDetailsView {
    fn listing_id(&self) -> ProductListingId {
        self.product_listing_id
    }
    fn lifecycle(&self) -> ListingLifecycle {
        self.lifecycle
    }
    fn shipping_mut(&mut self) -> &mut Option<AdvertisedShipping> {
        &mut self.shipping
    }
}
impl ShippingReadItem for ProductListingSummary {
    fn listing_id(&self) -> ProductListingId {
        self.product_listing_id
    }
    fn lifecycle(&self) -> ListingLifecycle {
        self.lifecycle
    }
    fn shipping_mut(&mut self) -> &mut Option<AdvertisedShipping> {
        &mut self.shipping
    }
}

fn eligible_ids<T: ShippingReadItem>(
    items: &[Personalized<T, ProductListingUserState>],
) -> Vec<ProductListingId> {
    items
        .iter()
        .filter(|p| eligible(p.item.lifecycle(), p.user_state.as_ref()))
        .map(|p| p.item.listing_id())
        .collect()
}

/// Replaces shipping on every page item, so ineligible items disclose none.
fn apply<T: ShippingReadItem>(
    items: &mut [Personalized<T, ProductListingUserState>],
    mut shipping: HashMap<ProductListingId, ShippingView>,
) {
    for item in items {
        let (advertised, personalized) = match shipping.remove(&item.item.listing_id()) {
            Some(view) => (Some(view.advertised), view.personalized),
            None => (None, None),
        };
        *item.item.shipping_mut() = advertised;
        if let Some(state) = &mut item.user_state {
            state.shipping = personalized;
        }
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
        ) -> Result<DeliveryAssessmentView, shipping_service::assessment::AssessmentError> {
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
