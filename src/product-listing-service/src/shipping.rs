//! Current shipping enrichment for public listing reads. Buyer state never enters item data.
use crate::{
    use_cases::{ProductListingDetailsWithUserState, ProductListingSummaryWithUserState},
    user_state::ProductListingUserState,
};
use application::operation_context::OperationContext;
use fxrate_core::{FxRateId, FxRateSnapshot, FxRateSnapshotError, RoundingMode};
use fxrate_service::ports::{FxRateSnapshotReadError, FxRateSnapshotReader};
use geo::GeographicDescription;
use money::{Currency, Price};
use product_listing_core::{
    listing_lifecycle::ListingLifecycle, product_listing_id::ProductListingId,
};
use shipping_core::assessment::{EffectiveDeliveryTerms, ShippingAssessment};
use shipping_core::{
    pricing::{ChargeBasis, PriceRange, ShippingPrice},
    terms::ShippingPolicy,
};
use shipping_service::assessment::{
    AssertionEvidence, AssessDeliveryRequest, AssessDeliveryUseCase, AssessListingRequest,
    ProfileAssessmentBasis,
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

/// Request-local memoization reuses conversions across profiles, inline terms and overrides.
struct ShippingConversion<'a> {
    snapshot: &'a FxRateSnapshot,
    currency: Currency,
    amounts: HashMap<(Currency, u64), Price>,
}
impl<'a> ShippingConversion<'a> {
    fn new(snapshot: &'a FxRateSnapshot, currency: Currency) -> Self {
        Self {
            snapshot,
            currency,
            amounts: HashMap::new(),
        }
    }
    fn amount(&mut self, price: Price) -> Result<Price, FxRateSnapshotError> {
        let key = (price.currency, price.monetary_amount.into());
        if let Some(converted) = self.amounts.get(&key) {
            return Ok(*converted);
        }
        let converted = self
            .snapshot
            .convert(price, self.currency, RoundingMode::HalfUp)?;
        self.amounts.insert(key, converted);
        Ok(converted)
    }
    fn price(
        &mut self,
        price: ShippingPrice,
    ) -> Result<Option<ShippingPrice>, FxRateSnapshotError> {
        Ok(Some(match price {
            ShippingPrice::Free(Some(currency)) => {
                self.amount(Price::new(0_u64.into(), currency))?;
                ShippingPrice::Free(Some(self.currency))
            }
            ShippingPrice::Fixed(price) => ShippingPrice::Fixed(self.amount(price)?),
            ShippingPrice::From(price) => ShippingPrice::From(self.amount(price)?),
            ShippingPrice::EstimatedRange(range) => ShippingPrice::EstimatedRange(
                PriceRange::new(self.amount(range.minimum())?, self.amount(range.maximum())?)
                    .expect("monotone conversion preserves range bounds"),
            ),
            _ => return Ok(None),
        }))
    }
    fn display(
        &mut self,
        effective: &EffectiveDeliveryTerms,
    ) -> Result<ShippingDisplay, FxRateSnapshotError> {
        let mut rates = Vec::new();
        if let Some(ShippingPolicy::Coverage(coverage)) = effective.terms().shipping() {
            for (zone_index, zone) in coverage.zones().iter().enumerate() {
                for (service_index, service) in zone.services().iter().enumerate() {
                    if let Some(rate) = service.rate()
                        && let Some(price) = self.price(rate.price())?
                    {
                        rates.push(ConvertedShippingRate {
                            zone_index,
                            service_index,
                            price,
                            basis: rate.basis(),
                        });
                    }
                }
            }
        }
        Ok(ShippingDisplay {
            currency: self.currency,
            fx_rate_id: self.snapshot.id(),
            captured_at: self.snapshot.captured_at(),
            rates,
        })
    }
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
        let attribution = view
            .destination
            .attribution
            .map(|a| DestinationAttribution {
                provider: a.provider,
                attribution_url: a.attribution_url,
            });
        let mut shipping: HashMap<_, _> =
            view.items
                .into_iter()
                .map(|item| {
                    let personalized = view.destination.effective.as_ref().map(|destination| {
                        PersonalizedShipping {
                            assessment: item.assessment,
                            destination: destination.clone(),
                            attribution: attribution.clone(),
                            evaluated_at: item.evaluated_at,
                        }
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
                .collect();
        if !shipping.is_empty() {
            let current_snapshot;
            let snapshot = match pinned_snapshot {
                Some(snapshot) => snapshot,
                None => {
                    current_snapshot = self
                        .fx_rates
                        .as_ref()
                        .ok_or(ShippingReadError::FxSnapshotMissing)?
                        .find_latest_at_or_before(valuation_at)
                        .await?
                        .ok_or(ShippingReadError::FxSnapshotMissing)?;
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

    pub async fn details(
        &self,
        context: &OperationContext,
        items: &mut [ProductListingDetailsWithUserState],
        currency: Currency,
        snapshot: Option<&FxRateSnapshot>,
        valuation_at: OffsetDateTime,
    ) -> Result<(), ShippingReadError> {
        let ids = items
            .iter()
            .filter(|p| eligible(p.item.lifecycle, p.user_state.as_ref()))
            .map(|p| p.item.product_listing_id)
            .collect();
        let mut shipping = self
            .load(context, ids, currency, snapshot, valuation_at)
            .await?;
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
        items: &mut [ProductListingSummaryWithUserState],
        currency: Currency,
        snapshot: &FxRateSnapshot,
    ) -> Result<(), ShippingReadError> {
        let ids = items
            .iter()
            .filter(|p| eligible(p.item.lifecycle, p.user_state.as_ref()))
            .map(|p| p.item.product_listing_id)
            .collect();
        let mut shipping = self
            .load(
                context,
                ids,
                currency,
                Some(snapshot),
                snapshot.captured_at(),
            )
            .await?;
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

#[cfg(test)]
mod fx_tests {
    use super::*;
    use fxrate_core::{FX_RATE_SCALE, FxRateQuote, FxRateSource, NewFxRateSnapshot};
    use shipping_core::{
        coverage::{
            CoverageCompleteness, DeliveryArea, ReferenceCountry, ShippingCoverage, ShippingZone,
        },
        inputs::Applicability,
        selection::{
            ComponentOverride, DeliveryOverrides, ListingDeliveryConfiguration, ProfileSelection,
        },
        terms::{DeliveryTerms, ShippingService, ShippingServiceCode, ShippingServiceTerms},
    };
    use strum::IntoEnumIterator;

    fn snapshot() -> FxRateSnapshot {
        NewFxRateSnapshot::capture_eur(
            FxRateId::new(),
            OffsetDateTime::UNIX_EPOCH,
            FxRateSource::FxRatesApi,
            Currency::Eur,
            Currency::iter().map(|currency| {
                FxRateQuote::new(
                    currency,
                    match currency {
                        Currency::Usd => 1_100_000,
                        Currency::Jpy => 160_000_000,
                        _ => FX_RATE_SCALE,
                    },
                )
            }),
        )
        .unwrap()
        .into_persisted(1_i64.try_into().unwrap())
    }
    fn terms(prices: &[ShippingPrice]) -> shipping_core::terms::DeliveryTerms {
        let release = geo::reference::ReferenceRelease::CURRENT;
        DeliveryTerms::new(
            Some(ShippingPolicy::coverage(
                ShippingCoverage::new(
                    release,
                    CoverageCompleteness::Partial,
                    Applicability::default(),
                    vec![],
                    vec![
                        ShippingZone::new(
                            vec![DeliveryArea::Country(
                                ReferenceCountry::new("DE", release).unwrap(),
                            )],
                            Applicability::default(),
                            prices
                                .iter()
                                .enumerate()
                                .map(|(i, price)| {
                                    ShippingService::new(
                                        ShippingServiceCode::new(format!("service-{i}")).unwrap(),
                                        ShippingServiceTerms {
                                            rate: Some(shipping_core::pricing::ShippingRate::new(
                                                *price,
                                                Some(ChargeBasis::PerItem),
                                                None,
                                                Default::default(),
                                            )),
                                            ..Default::default()
                                        },
                                    )
                                })
                                .collect(),
                        )
                        .unwrap(),
                    ],
                    None,
                )
                .unwrap(),
            )),
            None,
        )
    }
    #[test]
    fn effective_multi_currency_profile_and_override_preserve_source_terms_and_reuse_conversions() {
        let mut gallery = shipping_core::fixtures::gallery();
        let eur = Price::new(100_u64.into(), Currency::Eur);
        let jpy = Price::new(160_u64.into(), Currency::Jpy);
        let mut content = gallery.profile.content().clone();
        content.terms = terms(&[ShippingPrice::Fixed(eur), ShippingPrice::Fixed(jpy)]);
        gallery.profile.replace_content(content).unwrap();
        let shared = EffectiveDeliveryTerms::resolve(
            &gallery.painting,
            Some(&gallery.source_default),
            &[gallery.profile],
        );
        let original = shared.clone();
        let snapshot = snapshot();
        let mut conversion = ShippingConversion::new(&snapshot, Currency::Usd);
        for _ in 0..50 {
            let display = conversion.display(&shared).unwrap();
            assert_eq!(snapshot.id(), display.fx_rate_id);
            assert_eq!(snapshot.captured_at(), display.captured_at);
            assert_eq!(2, display.rates.len());
            for rate in display.rates {
                assert_eq!(
                    ShippingPrice::Fixed(Price::new(110_u64.into(), Currency::Usd)),
                    rate.price
                );
            }
        }
        assert_eq!(2, conversion.amounts.len());
        assert_eq!(original, shared);
        let override_terms = terms(&[ShippingPrice::Fixed(Price::new(
            220_u64.into(),
            Currency::Usd,
        ))]);
        let override_config = ListingDeliveryConfiguration::new(
            gallery.source_id,
            ProfileSelection::SuppressInheritance,
            Default::default(),
            DeliveryOverrides {
                shipping: ComponentOverride::Replace(override_terms.shipping().unwrap().clone()),
                ..Default::default()
            },
        )
        .unwrap();
        let effective = EffectiveDeliveryTerms::resolve(&override_config, None, &[]);
        assert_eq!(
            ShippingPrice::Fixed(Price::new(220_u64.into(), Currency::Usd)),
            conversion.display(&effective).unwrap().rates[0].price
        );
        let mut yen = ShippingConversion::new(&snapshot, Currency::Jpy);
        assert_eq!(
            Some(ShippingPrice::Fixed(jpy)),
            yen.price(ShippingPrice::Fixed(eur)).unwrap()
        );
    }
    #[test]
    fn conversion_retains_uncertainty_and_basis_without_inventing_amounts() {
        let snapshot = snapshot();
        let mut conversion = ShippingConversion::new(&snapshot, Currency::Usd);
        let eur = Price::new(100_u64.into(), Currency::Eur);
        let usd = Price::new(110_u64.into(), Currency::Usd);
        assert_eq!(
            Some(ShippingPrice::Free(Some(Currency::Usd))),
            conversion
                .price(ShippingPrice::Free(Some(Currency::Jpy)))
                .unwrap()
        );
        assert_eq!(
            Some(ShippingPrice::From(usd)),
            conversion.price(ShippingPrice::From(eur)).unwrap()
        );
        assert_eq!(
            Some(ShippingPrice::EstimatedRange(
                PriceRange::new(usd, Price::new(220_u64.into(), Currency::Usd)).unwrap()
            )),
            conversion
                .price(ShippingPrice::EstimatedRange(
                    PriceRange::new(eur, Price::new(200_u64.into(), Currency::Eur)).unwrap()
                ))
                .unwrap()
        );
        for price in [
            ShippingPrice::Free(None),
            ShippingPrice::OnRequest(None),
            ShippingPrice::OnRequest(Some(Currency::Eur)),
            ShippingPrice::ExternallyCalculated(Some(Currency::Eur)),
        ] {
            assert_eq!(None, conversion.price(price).unwrap());
        }
        assert_eq!(
            Err(FxRateSnapshotError::ConversionOverflow),
            conversion.price(ShippingPrice::Fixed(Price::new(
                u64::MAX.into(),
                Currency::Eur
            )))
        );
    }
    #[test]
    fn missing_legacy_quote_fails_instead_of_substituting_a_currency() {
        let snapshot = snapshot();
        let legacy = FxRateSnapshot::rehydrate(
            snapshot.id(),
            snapshot.generation(),
            snapshot.captured_at(),
            snapshot.source(),
            snapshot.quotes().iter().copied().take(19),
        )
        .unwrap();
        let mut conversion = ShippingConversion::new(&legacy, Currency::Eur);
        assert_eq!(
            Err(FxRateSnapshotError::MissingQuote(Currency::Krw)),
            conversion.price(ShippingPrice::Fixed(Price::new(
                1_u64.into(),
                Currency::Krw
            )))
        );
    }
}
