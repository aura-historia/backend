//! Compact, destination-independent discovery facts. Never a copied delivery policy.
use crate::{
    assessment::{EffectiveDeliveryTerms, PredicateResult},
    inputs::AssessmentInput,
    pricing::{ChargeBasis, ShippingPrice},
    terms::{CollectionDependency, ShippingEligibility, ShippingPolicy},
};
use geo::{CountryCode, GeographicDescription, country_from_code, reference::ReferenceRelease};
use money::Price;
use party_core::party_location::PartyLocation;

pub const POLICY: &str = "EFFECTIVE_SHIPPING_V1";
pub const MAX_COUNTRY_OFFERS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CountryOffer {
    pub country: CountryCode,
    /// Ordered rule/service coordinates, without arbitrary provider vocabulary.
    pub zone: u16,
    pub service: u16,
    /// Only a fixed advertised PER_ITEM amount, in currency minor units.
    pub per_item_price: Option<Price>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveShipping {
    pub collection: Option<ShippingEligibility>,
    pub shipping_prohibited: bool,
    /// Unrepresented or conditional facts remain explicit; absence is never prohibition.
    pub conditional: bool,
    pub country_offers: Vec<CountryOffer>,
}
impl Default for EffectiveShipping {
    fn default() -> Self {
        Self {
            collection: None,
            shipping_prohibited: false,
            conditional: true,
            country_offers: vec![],
        }
    }
}

pub fn derive(
    effective: &EffectiveDeliveryTerms,
    locations: &[PartyLocation],
) -> EffectiveShipping {
    let generic = effective.assess(&AssessmentInput::default(), locations);
    let collection = effective
        .terms()
        .collection()
        .filter(|c| c.dependency() == CollectionDependency::Independent)
        .and(generic.collection.eligibility);
    let mut summary = EffectiveShipping {
        collection,
        ..Default::default()
    };
    match effective.terms().shipping() {
        Some(ShippingPolicy::Prohibited) => {
            summary.shipping_prohibited = true;
            summary.conditional = false;
        }
        Some(ShippingPolicy::Coverage(coverage)) => {
            summary.conditional = false;
            for code in coverage.release().country_codes() {
                let country =
                    country_from_code(code, coverage.release()).expect("reference country");
                let input = AssessmentInput {
                    destination: GeographicDescription::new_in_release(
                        coverage.release(),
                        None,
                        Some(country),
                        None,
                        None,
                    )
                    .expect("reference country"),
                    ..Default::default()
                };
                // No quantity, basket, postcode, subdivision or selected variant is invented.
                // The ordinary evaluator proves a whole-country offer, including earlier
                // zones, exclusions, origin eligibility and known listing applicability.
                let assessed = effective.assess(&input, locations);
                if assessed.eligibility.is_none()
                    || assessed.eligibility == Some(ShippingEligibility::SubjectToConfirmation)
                {
                    summary.conditional = true;
                }
                if let Some(zone) = assessed.selected_zone {
                    for (index, service) in assessed.services.iter().enumerate() {
                        if service.applicability != PredicateResult::Match
                            || service.eligibility != Some(ShippingEligibility::Offered)
                        {
                            if !matches!(service.applicability, PredicateResult::NoMatch(_)) {
                                summary.conditional = true;
                            }
                            continue;
                        }
                        if summary.country_offers.len() == MAX_COUNTRY_OFFERS {
                            summary.conditional = true;
                            continue;
                        }
                        let price = service.service.rate().and_then(|rate| {
                            if rate.basis() != Some(ChargeBasis::PerItem) {
                                return None;
                            }
                            match rate.price() {
                                ShippingPrice::Fixed(price) => Some(price),
                                ShippingPrice::Free(Some(currency)) => {
                                    Some(Price::new(0_u64.into(), currency))
                                }
                                _ => None,
                            }
                        });
                        if price.is_none() {
                            summary.conditional = true;
                        }
                        summary.country_offers.push(CountryOffer {
                            country,
                            zone: zone as u16,
                            service: index as u16,
                            per_item_price: price,
                        });
                    }
                }
            }
        }
        None => {}
    }
    if !effective.shipping_reasons().is_empty()
        || !effective.collection_reasons().is_empty()
        || (effective.terms().collection().is_some() && collection.is_none())
    {
        summary.conditional = true;
    }
    summary
}

/// Strict codecs use this after mapping their own representations.
pub fn validate(summary: &EffectiveShipping) -> bool {
    summary.country_offers.len() <= MAX_COUNTRY_OFFERS
        && (!summary.shipping_prohibited || summary.country_offers.is_empty())
        && summary.country_offers.iter().enumerate().all(|(i, offer)| {
            offer.zone < 64
                && offer.service < 64
                && country_from_code(offer.country.alpha2(), ReferenceRelease::CURRENT).is_ok()
                && !summary.country_offers[..i].iter().any(|old| {
                    old.country == offer.country
                        && old.zone == offer.zone
                        && old.service == offer.service
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        coverage::*,
        fixtures::gallery,
        inputs::{Applicability, QuantityRange},
        terms::*,
    };
    use std::num::NonZeroU64;
    #[test]
    fn gallery_minor_units_and_independent_collection_survive_shared_profile_update() {
        let mut g = gallery();
        let before = derive(
            &EffectiveDeliveryTerms::resolve(
                &g.painting,
                Some(&g.source_default),
                std::slice::from_ref(&g.profile),
            ),
            &[],
        );
        assert_eq!(
            8000,
            u64::from(
                before.country_offers[0]
                    .per_item_price
                    .unwrap()
                    .monetary_amount
            )
        );
        let cabinet = derive(
            &EffectiveDeliveryTerms::resolve(
                &g.cabinet,
                Some(&g.source_default),
                std::slice::from_ref(&g.profile),
            ),
            &[],
        );
        let mut content = g.profile.content().clone();
        content.terms = crate::fixtures::gallery_terms(9500);
        g.profile.replace_content(content).unwrap();
        let after = derive(
            &EffectiveDeliveryTerms::resolve(
                &g.painting,
                Some(&g.source_default),
                std::slice::from_ref(&g.profile),
            ),
            &[],
        );
        assert_eq!(
            9500,
            u64::from(
                after.country_offers[0]
                    .per_item_price
                    .unwrap()
                    .monetary_amount
            )
        );
        assert_eq!(before.collection, after.collection);
        assert!(cabinet.shipping_prohibited);
        assert_eq!(
            cabinet,
            derive(
                &EffectiveDeliveryTerms::resolve(&g.cabinet, Some(&g.source_default), &[g.profile]),
                &[]
            )
        );
    }
    fn with_coverage(coverage: ShippingCoverage) -> EffectiveDeliveryTerms {
        let g = gallery();
        let config = crate::selection::ListingDeliveryConfiguration::new(
            g.source_id,
            crate::selection::ProfileSelection::Inline(Box::new(DeliveryTerms::new(
                Some(ShippingPolicy::coverage(coverage)),
                None,
            ))),
            Default::default(),
            Default::default(),
        )
        .unwrap();
        EffectiveDeliveryTerms::resolve(&config, None, &[])
    }
    #[test]
    fn country_only_proof_rejects_postal_exclusions_earlier_uncertainty_and_basket_inputs() {
        let g = gallery();
        let ShippingPolicy::Coverage(c) = g.profile.content().terms.shipping().unwrap() else {
            unreachable!()
        };
        let postal = DeliveryArea::Postal(
            PostalRule::new(
                PostalRuleVersion::V1,
                ReferenceCountry::new("DE", c.release()).unwrap(),
                PostalForm::GermanPrefix("1".into()),
            )
            .unwrap(),
        );
        let zone = c.zones()[0].clone();
        let earlier = ShippingZone::new(
            vec![postal.clone()],
            Default::default(),
            zone.services().to_vec(),
        )
        .unwrap();
        let quantity = Applicability::new(
            None,
            None,
            Some(
                QuantityRange::new(NonZeroU64::new(1).unwrap(), NonZeroU64::new(5).unwrap())
                    .unwrap(),
            ),
            None,
        )
        .unwrap();
        for (app, exclusions, zones, origin) in [
            (
                Applicability::default(),
                vec![postal],
                vec![zone.clone()],
                None,
            ),
            (
                Applicability::default(),
                vec![],
                vec![earlier, zone.clone()],
                None,
            ),
            (quantity, vec![], vec![zone.clone()], None),
            (
                Applicability::default(),
                vec![],
                vec![
                    ShippingZone::new(
                        vec![DeliveryArea::Domestic],
                        Default::default(),
                        zone.services().to_vec(),
                    )
                    .unwrap(),
                ],
                None,
            ),
        ] {
            let effective = with_coverage(
                ShippingCoverage::new(
                    c.release(),
                    c.completeness(),
                    app,
                    exclusions,
                    zones,
                    origin,
                )
                .unwrap(),
            );
            let summary = derive(&effective, &[]);
            assert!(summary.country_offers.is_empty());
            assert!(summary.conditional);
            assert!(!summary.shipping_prohibited);
        }
    }
}
