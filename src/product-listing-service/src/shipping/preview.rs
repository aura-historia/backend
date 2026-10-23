//! Selection of the single advertised rate a compact personalized read may preview.
use super::{ShippingDisplay, ShippingPricePreview, conversion::AdvertisedRate};
use money::Price;
use shipping_core::{
    assessment::{PredicateResult, ServiceAssessment, ShippingAssessment},
    pricing::{ChargeBasis, ShippingPrice},
    terms::{ShippingEligibility, ShippingServiceCode},
};

/// The previewed rate at its zone/service position in the effective terms.
pub(super) struct SelectedPreview {
    zone_index: usize,
    service_index: usize,
    service: ShippingServiceCode,
    basis: ChargeBasis,
    price: ShippingPrice,
}
impl SelectedPreview {
    pub(super) fn rate(&self) -> AdvertisedRate {
        AdvertisedRate {
            zone_index: self.zone_index,
            service_index: self.service_index,
            price: self.price,
        }
    }
    /// Attaches the requested-currency value the display holds for this rate.
    pub(super) fn with_display(self, display: Option<&ShippingDisplay>) -> ShippingPricePreview {
        ShippingPricePreview {
            display_price: display
                .and_then(|display| display.price(self.zone_index, self.service_index)),
            service: self.service,
            basis: self.basis,
            price: self.price,
        }
    }
}

struct Candidate<'a> {
    index: usize,
    assessment: &'a ServiceAssessment,
    basis: ChargeBasis,
    price: ShippingPrice,
}

/// Previews a rate of a service that establishes the assessed eligibility in the selected zone.
/// Services with undetermined conditions never supply the preview. Every candidate needs an
/// explicit basis; several candidates are compared only when they share that basis and have
/// exact prices in one currency, choosing the lowest and then the first advertised.
/// Any other choice would rank incomparable or unknown terms, so no preview is returned.
pub(super) fn price_preview(assessment: &ShippingAssessment) -> Option<SelectedPreview> {
    let eligibility = assessment
        .eligibility
        .filter(|eligibility| *eligibility != ShippingEligibility::NotOffered)?;
    let zone = assessment.selected_zone?;
    let candidates = assessment
        .services
        .iter()
        .enumerate()
        .filter(|(_, service)| {
            service.applicability == PredicateResult::Match
                && service.eligibility == Some(eligibility)
        })
        .map(|(index, assessment)| {
            let rate = assessment.service.rate()?;
            Some(Candidate {
                index,
                assessment,
                basis: rate.basis()?,
                price: rate.price(),
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let chosen = match candidates.as_slice() {
        [] => return None,
        [only] => only,
        several => lowest_comparable(several)?,
    };
    Some(SelectedPreview {
        zone_index: zone,
        service_index: chosen.index,
        service: chosen.assessment.service.code().clone(),
        basis: chosen.basis,
        price: chosen.price,
    })
}

fn lowest_comparable<'a>(candidates: &'a [Candidate<'a>]) -> Option<&'a Candidate<'a>> {
    let first = candidates.first()?;
    let currency = exact_amount(first.price)?.currency;
    let mut amounts = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let amount = exact_amount(candidate.price)?;
        if candidate.basis != first.basis || amount.currency != currency {
            return None;
        }
        amounts.push((u64::from(amount.monetary_amount), candidate));
    }
    amounts
        .into_iter()
        .min_by_key(|(amount, _)| *amount)
        .map(|(_, candidate)| candidate)
}

fn exact_amount(price: ShippingPrice) -> Option<Price> {
    match price {
        ShippingPrice::Fixed(amount) => Some(amount),
        ShippingPrice::Free(Some(currency)) => Some(Price::new(0_u64.into(), currency)),
        ShippingPrice::Free(None)
        | ShippingPrice::From(_)
        | ShippingPrice::EstimatedRange(_)
        | ShippingPrice::ExternallyCalculated(_)
        | ShippingPrice::OnRequest(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shipping::ConvertedShippingRate;
    use geo::{CountryCode, GeographicDescription, reference::ReferenceRelease};
    use money::Currency;
    use shipping_core::{
        assessment::{EffectiveDeliveryTerms, assess},
        coverage::{
            CoverageCompleteness, DeliveryArea, ReferenceCountry, ShippingCoverage, ShippingZone,
        },
        inputs::{AmountThreshold, Applicability, AssessmentInput},
        pricing::{PriceRange, QualifyingScope, ShippingRate},
        selection::{ListingDeliveryConfiguration, ProfileSelection},
        terms::{
            DeliveryTerms, ShippingPolicy, ShippingService, ShippingServiceCode,
            ShippingServiceTerms,
        },
    };

    const RELEASE: ReferenceRelease = ReferenceRelease::IsoCodes4_20_1;

    fn eur(minor: u64) -> Price {
        Price::new(minor.into(), Currency::Eur)
    }
    fn service(
        code: &str,
        eligibility: ShippingEligibility,
        price: ShippingPrice,
        basis: Option<ChargeBasis>,
    ) -> ShippingService {
        conditional(code, eligibility, price, basis, Applicability::default())
    }
    fn conditional(
        code: &str,
        eligibility: ShippingEligibility,
        price: ShippingPrice,
        basis: Option<ChargeBasis>,
        applicability: Applicability,
    ) -> ShippingService {
        ShippingService::new(
            ShippingServiceCode::new(code.to_owned()).unwrap(),
            ShippingServiceTerms {
                applicability,
                eligibility: Some(eligibility),
                rate: Some(ShippingRate::new(price, basis, None, Default::default())),
                ..Default::default()
            },
        )
    }
    /// Assesses a German destination against an unrelated French zone and the given German one.
    fn assessed(services: Vec<ShippingService>) -> ShippingAssessment {
        let zone = |code: &str, services| {
            ShippingZone::new(
                vec![DeliveryArea::Country(
                    ReferenceCountry::new(code, RELEASE).unwrap(),
                )],
                Applicability::default(),
                services,
            )
            .unwrap()
        };
        let unrelated = vec![service(
            "UNRELATED_FREE",
            ShippingEligibility::Offered,
            ShippingPrice::Free(Some(Currency::Eur)),
            Some(ChargeBasis::PerItem),
        )];
        let terms = DeliveryTerms::new(
            Some(ShippingPolicy::coverage(
                ShippingCoverage::new(
                    RELEASE,
                    CoverageCompleteness::Partial,
                    Applicability::default(),
                    vec![],
                    vec![zone("FR", unrelated), zone("DE", services)],
                    None,
                )
                .unwrap(),
            )),
            None,
        );
        let source = shipping_core::fixtures::gallery().source_id;
        let configuration = ListingDeliveryConfiguration::new(
            source,
            ProfileSelection::inline(terms),
            Default::default(),
            Default::default(),
        )
        .unwrap();
        let input = AssessmentInput {
            destination: GeographicDescription::new_in_release(
                RELEASE,
                None,
                Some(CountryCode::DEU),
                None,
                None,
            )
            .unwrap(),
            ..Default::default()
        };
        assess(
            &EffectiveDeliveryTerms::resolve(&configuration, None, &[]),
            &input,
            &[],
        )
    }
    fn preview_of(services: Vec<ShippingService>) -> Option<(String, ChargeBasis, ShippingPrice)> {
        price_preview(&assessed(services))
            .map(|p| (p.service.as_str().to_owned(), p.basis, p.price))
    }
    fn fixed_per_item(code: &str, minor: u64) -> ShippingService {
        service(
            code,
            ShippingEligibility::Offered,
            ShippingPrice::Fixed(eur(minor)),
            Some(ChargeBasis::PerItem),
        )
    }

    #[test]
    fn should_preview_an_applicable_per_item_rate_without_inferring_a_quantity() {
        let assessment = assessed(vec![fixed_per_item("PARCEL", 1_200)]);
        assert!(assessment.services[0].charge.is_none());
        assert_eq!(
            Some((
                "PARCEL".to_owned(),
                ChargeBasis::PerItem,
                ShippingPrice::Fixed(eur(1_200))
            )),
            preview_of(vec![fixed_per_item("PARCEL", 1_200)])
        );
    }

    #[test]
    fn should_ignore_conditional_free_thresholds_and_unrelated_zones() {
        let threshold = Applicability::new(
            None,
            None,
            None,
            Some(AmountThreshold {
                minimum: eur(500_000),
                scope: QualifyingScope::SellerOrder,
            }),
        )
        .unwrap();
        let freight = conditional(
            "FREIGHT",
            ShippingEligibility::Offered,
            ShippingPrice::Free(Some(Currency::Eur)),
            Some(ChargeBasis::PerSellerOrder),
            threshold,
        );
        assert_eq!(
            Some((
                "PARCEL".to_owned(),
                ChargeBasis::PerItem,
                ShippingPrice::Fixed(eur(1_200))
            )),
            preview_of(vec![freight.clone(), fixed_per_item("PARCEL", 1_200)])
        );
        assert_eq!(None, preview_of(vec![freight]));
    }

    #[test]
    fn should_choose_the_lowest_then_first_comparable_exact_rate() {
        assert_eq!(
            Some("ECONOMY".to_owned()),
            preview_of(vec![
                fixed_per_item("EXPRESS", 2_500),
                fixed_per_item("ECONOMY", 900),
                fixed_per_item("STANDARD", 900),
            ])
            .map(|p| p.0)
        );
        assert_eq!(
            Some((
                "PICKUP_POINT".to_owned(),
                ChargeBasis::PerItem,
                ShippingPrice::Free(Some(Currency::Eur))
            )),
            preview_of(vec![
                fixed_per_item("PARCEL", 1_200),
                service(
                    "PICKUP_POINT",
                    ShippingEligibility::Offered,
                    ShippingPrice::Free(Some(Currency::Eur)),
                    Some(ChargeBasis::PerItem),
                ),
            ])
        );
    }

    #[test]
    fn should_omit_the_preview_when_alternatives_are_not_comparable() {
        let per_shipment = service(
            "FREIGHT",
            ShippingEligibility::Offered,
            ShippingPrice::Fixed(eur(500)),
            Some(ChargeBasis::PerShipment),
        );
        let from = service(
            "FREIGHT",
            ShippingEligibility::Offered,
            ShippingPrice::From(eur(500)),
            Some(ChargeBasis::PerItem),
        );
        let usd = service(
            "FREIGHT",
            ShippingEligibility::Offered,
            ShippingPrice::Fixed(Price::new(500_u64.into(), Currency::Usd)),
            Some(ChargeBasis::PerItem),
        );
        let unknown_basis = service(
            "FREIGHT",
            ShippingEligibility::Offered,
            ShippingPrice::Fixed(eur(500)),
            None,
        );
        for alternative in [per_shipment, from, usd, unknown_basis] {
            assert_eq!(
                None,
                preview_of(vec![fixed_per_item("PARCEL", 1_200), alternative])
            );
        }
    }

    #[test]
    fn should_omit_the_preview_for_an_unknown_charge_basis() {
        assert_eq!(
            None,
            preview_of(vec![service(
                "PARCEL",
                ShippingEligibility::Offered,
                ShippingPrice::Fixed(eur(1_200)),
                None,
            )])
        );
    }

    #[test]
    fn should_keep_every_price_kind_of_a_single_applicable_rate() {
        let range = PriceRange::new(eur(1_000), eur(3_000)).unwrap();
        for price in [
            ShippingPrice::Free(None),
            ShippingPrice::Free(Some(Currency::Eur)),
            ShippingPrice::Fixed(eur(1_200)),
            ShippingPrice::From(eur(1_200)),
            ShippingPrice::EstimatedRange(range),
            ShippingPrice::ExternallyCalculated(None),
            ShippingPrice::OnRequest(Some(Currency::Eur)),
        ] {
            assert_eq!(
                Some(price),
                preview_of(vec![service(
                    "PARCEL",
                    ShippingEligibility::Offered,
                    price,
                    Some(ChargeBasis::PerLot),
                )])
                .map(|p| p.2)
            );
        }
    }

    #[test]
    fn should_preview_only_services_establishing_the_summarized_eligibility() {
        let confirmation = service(
            "FREIGHT",
            ShippingEligibility::SubjectToConfirmation,
            ShippingPrice::OnRequest(None),
            Some(ChargeBasis::PerShipment),
        );
        assert_eq!(
            Some("PARCEL".to_owned()),
            preview_of(vec![confirmation.clone(), fixed_per_item("PARCEL", 1_200)]).map(|p| p.0)
        );
        assert_eq!(
            Some("FREIGHT".to_owned()),
            preview_of(vec![confirmation]).map(|p| p.0)
        );
        let refused = service(
            "PARCEL",
            ShippingEligibility::NotOffered,
            ShippingPrice::Fixed(eur(1_200)),
            Some(ChargeBasis::PerItem),
        );
        assert_eq!(None, preview_of(vec![refused]));
    }

    #[test]
    fn should_attach_the_requested_currency_value_of_the_selected_rate() {
        let assessment = assessed(vec![
            fixed_per_item("EXPRESS", 2_500),
            fixed_per_item("PARCEL", 1_200),
        ]);
        let converted = |zone_index, service_index, minor: u64| ConvertedShippingRate {
            zone_index,
            service_index,
            price: ShippingPrice::Fixed(Price::new(minor.into(), Currency::Usd)),
        };
        let display = ShippingDisplay {
            currency: Currency::Usd,
            fx_rate_id: fxrate_core::FxRateId::new(),
            captured_at: time::OffsetDateTime::UNIX_EPOCH,
            rates: vec![
                converted(0, 1, 1),
                converted(1, 0, 2_750),
                converted(1, 1, 1_320),
            ],
        };
        let preview = price_preview(&assessment)
            .unwrap()
            .with_display(Some(&display));
        assert_eq!(ShippingPrice::Fixed(eur(1_200)), preview.price);
        assert_eq!(
            Some(ShippingPrice::Fixed(Price::new(
                1_320_u64.into(),
                Currency::Usd
            ))),
            preview.display_price
        );
    }
}
