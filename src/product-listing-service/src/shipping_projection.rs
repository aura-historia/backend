//! Compact discovery facts. Current listing responses continue to use shipping assessment.
use application::{
    error::BoxError,
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use geo::{CountryCode, GeographicDescription};
use money::Price;
use product_listing_core::product_listing_id::ProductListingId;
use shipping_core::{
    assessment::{EffectiveDeliveryTerms, PredicateResult},
    inputs::AssessmentInput,
    pricing::{ChargeBasis, ShippingPrice},
    terms::{CollectionDependency, ShippingEligibility, ShippingPolicy, ShippingServiceCode},
};
use shipping_service::assessment::{
    AssessListingRequest, AssessmentDependencies, AssessmentError, assess_snapshot,
};
use uuid::Uuid;

pub const POLICY: &str = "EFFECTIVE_SHIPPING_V1";
pub const MAX_COUNTRY_OFFERS: usize = 256;
domain_primitives::object_id_newtype!(ShippingWorkId, "swork");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountryShippingOffer {
    pub country: CountryCode,
    pub service: ShippingServiceCode,
    /// An advertised per-item amount in minor units, never a basket total or quote.
    pub per_item_fixed: Option<Price>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveListingShipping {
    pub collection: Option<ShippingEligibility>,
    pub prohibited: bool,
    pub conditional: bool,
    pub country_offers: Vec<CountryShippingOffer>,
}
impl Default for EffectiveListingShipping {
    fn default() -> Self {
        // An absent/rebuilding projection carries uncertainty, never a prohibition.
        Self {
            collection: None,
            prohibited: false,
            conditional: true,
            country_offers: vec![],
        }
    }
}
/// Country-only evaluation deliberately supplies no basket counts, variant or postal code.
/// The evaluator blocks unresolved exclusions and earlier zones before returning a service.
pub fn derive(
    effective: &EffectiveDeliveryTerms,
    locations: &[party_core::party_location::PartyLocation],
) -> EffectiveListingShipping {
    let generic = effective.assess(&AssessmentInput::default(), locations);
    let independent = effective
        .terms()
        .collection()
        .is_some_and(|c| c.dependency() == CollectionDependency::Independent);
    let mut summary = EffectiveListingShipping {
        collection: independent
            .then_some(generic.collection.eligibility)
            .flatten(),
        prohibited: matches!(
            effective.terms().shipping(),
            Some(ShippingPolicy::Prohibited)
        ),
        conditional: effective.terms().shipping().is_none()
            || (effective.terms().collection().is_some()
                && (!independent || generic.collection.eligibility.is_none())),
        country_offers: vec![],
    };
    let Some(ShippingPolicy::Coverage(coverage)) = effective.terms().shipping() else {
        return summary;
    };
    for country in coverage.release().country_codes() {
        let country = geo::country_from_code(country, coverage.release()).expect("release country");
        let assessment = effective.assess(
            &AssessmentInput {
                destination: GeographicDescription::new_in_release(
                    coverage.release(),
                    None,
                    Some(country),
                    None,
                    None,
                )
                .expect("release country"),
                ..Default::default()
            },
            locations,
        );
        if assessment.eligibility.is_none()
            || assessment.eligibility == Some(ShippingEligibility::SubjectToConfirmation)
        {
            summary.conditional = true;
        }
        for service in assessment.services {
            if service.applicability != PredicateResult::Match
                || service.eligibility != Some(ShippingEligibility::Offered)
            {
                summary.conditional |= service.applicability != PredicateResult::Match
                    || service.eligibility != Some(ShippingEligibility::NotOffered);
                continue;
            }
            let per_item_fixed = service.service.rate().and_then(|rate| {
                if rate.basis() != Some(ChargeBasis::PerItem) {
                    return None;
                }
                match rate.price() {
                    ShippingPrice::Fixed(price) => Some(price),
                    ShippingPrice::Free(Some(currency)) => Some(Price::new(0_u64.into(), currency)),
                    _ => None,
                }
            });
            summary.conditional |= per_item_fixed.is_none();
            if summary.country_offers.len() == MAX_COUNTRY_OFFERS {
                summary.conditional = true;
                continue;
            }
            summary.country_offers.push(CountryShippingOffer {
                country,
                service: service.service.code().clone(),
                per_item_fixed,
            });
        }
    }
    summary
}

#[derive(Debug, Clone)]
pub struct ShippingClaim {
    pub work_id: ShippingWorkId,
    pub generation: i64,
    pub token: Uuid,
    pub kind: String,
    pub dependency_id: String,
    pub after_listing: Option<Uuid>,
}
pub struct ShippingPage {
    pub dependencies: AssessmentDependencies,
    /// Includes withdrawn rows, which must have their derived state cleared.
    pub listing_ids: Vec<ProductListingId>,
    pub after_listing: Option<Uuid>,
    pub complete: bool,
}
pub struct ShippingWrite {
    pub listing_id: ProductListingId,
    pub summary: EffectiveListingShipping,
}
#[derive(Debug, thiserror::Error)]
pub enum ShippingProjectionError {
    #[error("effective shipping persistence failed")]
    Storage(#[source] BoxError),
    #[error("effective shipping transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
    #[error("effective shipping dependency read failed")]
    Assessment(
        #[from]
        #[source]
        AssessmentError,
    ),
    #[error("effective shipping claim was superseded or expired")]
    Superseded,
    #[error("effective shipping work is claimed by another worker")]
    Deferred,
    #[error("invalid effective shipping work input")]
    InvalidInput,
}
#[async_trait::async_trait]
pub trait ShippingProjectionStore: Send {
    /// Claim only the work identified by a CDC/SQS wake-up.
    async fn claim(
        &mut self,
        work: ShippingWorkId,
    ) -> Result<Option<ShippingClaim>, ShippingProjectionError>;
    async fn page(
        &mut self,
        claim: &ShippingClaim,
        limit: u16,
    ) -> Result<ShippingPage, ShippingProjectionError>;
    async fn apply(&mut self, writes: &[ShippingWrite]) -> Result<u32, ShippingProjectionError>;
    async fn checkpoint(
        &mut self,
        claim: &ShippingClaim,
        page: &ShippingPage,
    ) -> Result<(), ShippingProjectionError>;
}
pub trait ShippingProjectionStoreFactory<Tx>: Send + Sync {
    fn in_transaction<'a>(&'a self, tx: &'a mut Tx) -> impl ShippingProjectionStore + 'a;
}
#[async_trait::async_trait]
pub trait ProjectListingShippingUseCase: Send + Sync {
    async fn execute(
        &self,
        work: ShippingWorkId,
        limit: u16,
    ) -> Result<u32, ShippingProjectionError>;
}
pub struct ProjectListingShippingHandler<U, S> {
    uow: U,
    store: S,
}
impl<U, S> ProjectListingShippingHandler<U, S> {
    pub fn new(uow: U, store: S) -> Self {
        Self { uow, store }
    }
}
#[async_trait::async_trait]
impl<U, S> ProjectListingShippingUseCase for ProjectListingShippingHandler<U, S>
where
    U: UnitOfWork,
    S: ShippingProjectionStoreFactory<U::Tx>,
{
    async fn execute(
        &self,
        work: ShippingWorkId,
        limit: u16,
    ) -> Result<u32, ShippingProjectionError> {
        if !(1..=100).contains(&limit) {
            return Err(ShippingProjectionError::InvalidInput);
        }
        let mut tx = self.uow.begin().await?;
        let claim = self.store.in_transaction(&mut tx).claim(work).await?;
        tx.commit().await?;
        let Some(claim) = claim else {
            return Ok(0);
        };
        let mut tx = self.uow.begin().await?;
        let page = self
            .store
            .in_transaction(&mut tx)
            .page(&claim, limit)
            .await?;
        let views = assess_snapshot(
            &page.dependencies,
            page.dependencies
                .listings
                .iter()
                .map(|l| AssessListingRequest {
                    listing_id: l.id,
                    input: Default::default(),
                })
                .collect(),
            None,
        );
        let writes = page
            .listing_ids
            .iter()
            .map(|id| {
                let view = views.iter().find(|v| v.listing_id == *id);
                let summary = view
                    .map(|v| {
                        let listing = page
                            .dependencies
                            .listings
                            .iter()
                            .find(|l| l.id == *id)
                            .expect("assessed listing");
                        let ids =
                            shipping_service::assessment::referenced_locations(v.effective.terms());
                        let locations = page
                            .dependencies
                            .locations
                            .iter()
                            .filter(|l| {
                                ids.contains(&l.location.id())
                                    && l.location.party_id() == listing.operator_id
                            })
                            .map(|l| l.location.clone())
                            .collect::<Vec<_>>();
                        derive(&v.effective, &locations)
                    })
                    .unwrap_or_default();
                ShippingWrite {
                    listing_id: *id,
                    summary,
                }
            })
            .collect::<Vec<_>>();
        let changed = self.store.in_transaction(&mut tx).apply(&writes).await?;
        self.store
            .in_transaction(&mut tx)
            .checkpoint(&claim, &page)
            .await?;
        tx.commit().await?;
        tracing::info!(
            event = "listing_shipping.projected",
            changed,
            scanned = page.listing_ids.len(),
            complete = page.complete
        );
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shipping_core::{
        coverage::*,
        fixtures::gallery,
        inputs::{Applicability, QuantityRange},
        selection::{DeliveryOverrides, ListingDeliveryConfiguration, ProfileSelection},
        terms::*,
    };
    fn project(
        terms: DeliveryTerms,
        source: listing_source_core::ListingSourceId,
    ) -> EffectiveListingShipping {
        let config = ListingDeliveryConfiguration::new(
            source,
            ProfileSelection::inline(terms),
            Default::default(),
            DeliveryOverrides::default(),
        )
        .unwrap();
        derive(&EffectiveDeliveryTerms::resolve(&config, None, &[]), &[])
    }
    #[test]
    fn only_exact_per_item_prices_are_projected_with_their_source_currency() {
        use money::{Currency, Price};
        use shipping_core::pricing::{PriceRange, ShippingRate};
        let f = gallery();
        let p = Price::new(8000_u64.into(), Currency::Eur);
        let usd = Price::new(8000_u64.into(), Currency::Usd);
        for (price, exact) in [
            (ShippingPrice::Fixed(p), Some(p)),
            (
                ShippingPrice::Free(Some(Currency::Eur)),
                Some(Price::new(0_u64.into(), Currency::Eur)),
            ),
            (ShippingPrice::Free(None), None),
            (ShippingPrice::Fixed(usd), Some(usd)),
            (ShippingPrice::From(p), None),
            (
                ShippingPrice::EstimatedRange(PriceRange::new(p, p).unwrap()),
                None,
            ),
            (
                ShippingPrice::ExternallyCalculated(Some(Currency::Eur)),
                None,
            ),
            (ShippingPrice::OnRequest(Some(Currency::Eur)), None),
        ] {
            for basis in [
                None,
                Some(ChargeBasis::PerItem),
                Some(ChargeBasis::PerLot),
                Some(ChargeBasis::PerShipment),
                Some(ChargeBasis::PerSellerOrder),
            ] {
                let service = ShippingService::new(
                    ShippingServiceCode::new("rate".into()).unwrap(),
                    ShippingServiceTerms {
                        eligibility: Some(ShippingEligibility::Offered),
                        rate: Some(ShippingRate::new(price, basis, None, Default::default())),
                        ..Default::default()
                    },
                );
                let terms = DeliveryTerms::new(
                    Some(ShippingPolicy::coverage(
                        ShippingCoverage::new(
                            geo::reference::ReferenceRelease::CURRENT,
                            CoverageCompleteness::Partial,
                            Applicability::default(),
                            vec![],
                            vec![
                                ShippingZone::new(
                                    vec![DeliveryArea::Country(
                                        ReferenceCountry::new(
                                            "DE",
                                            geo::reference::ReferenceRelease::CURRENT,
                                        )
                                        .unwrap(),
                                    )],
                                    Applicability::default(),
                                    vec![service],
                                )
                                .unwrap(),
                            ],
                            None,
                        )
                        .unwrap(),
                    )),
                    None,
                );
                let projected = project(terms, f.source_id);
                assert_eq!(1, projected.country_offers.len());
                assert_eq!(geo::CountryCode::DEU, projected.country_offers[0].country);
                assert_eq!(
                    exact.filter(|_| basis == Some(ChargeBasis::PerItem)),
                    projected.country_offers[0].per_item_fixed,
                    "{price:?} {basis:?}"
                );
            }
        }
    }

    #[test]
    fn unknown_and_truncated_summaries_preserve_uncertainty() {
        assert!(EffectiveListingShipping::default().conditional);
        assert!(!EffectiveListingShipping::default().prohibited);
        let f = gallery();
        let base = shipping_core::fixtures::gallery_terms(8000);
        let ShippingPolicy::Coverage(c) = base.shipping().unwrap() else {
            unreachable!()
        };
        let service = c.zones()[0].services()[0].clone();
        let alternative = ShippingService::new(
            ShippingServiceCode::new("alternative".into()).unwrap(),
            ShippingServiceTerms {
                eligibility: Some(ShippingEligibility::Offered),
                rate: service.rate().cloned(),
                ..Default::default()
            },
        );
        let zone = ShippingZone::new(
            vec![DeliveryArea::Anywhere],
            Applicability::default(),
            vec![service, alternative],
        )
        .unwrap();
        let terms = DeliveryTerms::new(
            Some(ShippingPolicy::coverage(
                ShippingCoverage::new(
                    c.release(),
                    CoverageCompleteness::Complete,
                    Applicability::default(),
                    vec![],
                    vec![zone],
                    None,
                )
                .unwrap(),
            )),
            None,
        );
        let summary = project(terms, f.source_id);
        assert_eq!(MAX_COUNTRY_OFFERS, summary.country_offers.len());
        assert!(summary.conditional);
        assert_eq!(
            summary.country_offers[0].country,
            summary.country_offers[1].country
        );
        assert_ne!(
            summary.country_offers[0].service,
            summary.country_offers[1].service
        );
    }
    #[test]
    fn gallery_and_independent_collection_have_separate_discovery_facts() {
        let f = gallery();
        let painting = derive(
            &EffectiveDeliveryTerms::resolve(
                &f.painting,
                Some(&f.source_default),
                std::slice::from_ref(&f.profile),
            ),
            &[],
        );
        let cabinet = derive(
            &EffectiveDeliveryTerms::resolve(
                &f.cabinet,
                Some(&f.source_default),
                std::slice::from_ref(&f.profile),
            ),
            &[],
        );
        assert_eq!(Some(ShippingEligibility::Offered), painting.collection);
        assert_eq!(1, painting.country_offers.len());
        assert_eq!(geo::CountryCode::DEU, painting.country_offers[0].country);
        assert_eq!(
            8000,
            u64::from(
                painting.country_offers[0]
                    .per_item_fixed
                    .unwrap()
                    .monetary_amount
            )
        );
        assert!(cabinet.prohibited);
        assert!(cabinet.country_offers.is_empty());
        assert_eq!(painting.collection, cabinet.collection);
    }
    #[test]
    fn postal_exclusions_unknown_origins_baskets_and_variants_never_become_country_offers() {
        let f = gallery();
        let base = shipping_core::fixtures::gallery_terms(8000);
        let ShippingPolicy::Coverage(coverage) = base.shipping().unwrap() else {
            unreachable!()
        };
        let release = coverage.release();
        let postal = DeliveryArea::Postal(
            PostalRule::new(
                PostalRuleVersion::V1,
                ReferenceCountry::new("DE", release).unwrap(),
                PostalForm::Exact(geo::PostalCode::new("10115").unwrap()),
            )
            .unwrap(),
        );
        let terms = |exclusions, zones, origin| {
            DeliveryTerms::new(
                Some(ShippingPolicy::coverage(
                    ShippingCoverage::new(
                        release,
                        CoverageCompleteness::Partial,
                        Applicability::default(),
                        exclusions,
                        zones,
                        origin,
                    )
                    .unwrap(),
                )),
                base.collection().cloned(),
            )
        };
        let excluded = project(
            terms(vec![postal.clone()], coverage.zones().to_vec(), None),
            f.source_id,
        );
        assert!(excluded.country_offers.is_empty());
        assert!(excluded.conditional);
        assert_eq!(Some(ShippingEligibility::Offered), excluded.collection);
        let earlier = ShippingZone::new(
            vec![postal],
            Applicability::default(),
            coverage.zones()[0].services().to_vec(),
        )
        .unwrap();
        let blocked = project(
            terms(vec![], vec![earlier, coverage.zones()[0].clone()], None),
            f.source_id,
        );
        assert!(blocked.country_offers.is_empty());
        assert!(blocked.conditional);
        let domestic = ShippingZone::new(
            vec![DeliveryArea::Domestic],
            Applicability::default(),
            coverage.zones()[0].services().to_vec(),
        )
        .unwrap();
        assert!(
            project(terms(vec![], vec![domestic], None), f.source_id)
                .country_offers
                .is_empty()
        );
        let quantity = Applicability::new(
            None,
            None,
            Some(
                QuantityRange::new(
                    std::num::NonZeroU64::new(1).unwrap(),
                    std::num::NonZeroU64::new(2).unwrap(),
                )
                .unwrap(),
            ),
            None,
        )
        .unwrap();
        let zone = ShippingZone::new(
            coverage.zones()[0].areas().to_vec(),
            quantity,
            coverage.zones()[0].services().to_vec(),
        )
        .unwrap();
        assert!(
            project(terms(vec![], vec![zone], None), f.source_id)
                .country_offers
                .is_empty()
        );
        let variants = Applicability::new(
            None,
            Some(
                [
                    shipping_core::inputs::SourceVariant::new(f.source_id, "unknown".into())
                        .unwrap(),
                ]
                .into(),
            ),
            None,
            None,
        )
        .unwrap();
        let zone = ShippingZone::new(
            coverage.zones()[0].areas().to_vec(),
            variants,
            coverage.zones()[0].services().to_vec(),
        )
        .unwrap();
        assert!(
            project(terms(vec![], vec![zone], None), f.source_id)
                .country_offers
                .is_empty()
        );
        let location = party_core::party_location_id::PartyLocationId::new();
        assert!(
            project(
                terms(
                    vec![],
                    coverage.zones().to_vec(),
                    Some(DeliverySite::PartyLocation(location))
                ),
                f.source_id
            )
            .country_offers
            .is_empty()
        );
    }
}
