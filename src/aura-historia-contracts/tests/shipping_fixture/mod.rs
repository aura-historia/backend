//! A stable large advertised policy resembling observed staging terms: a domestic zone with
//! parcel, conditional free freight and class-restricted furniture rates, repeated regional
//! zones and an international on-request zone.
#![allow(dead_code)]
use geo::{CountryCode, GeographicDescription, reference::ReferenceRelease};
use money::{Currency, Price};
use product_listing_service::shipping::{ConvertedShippingRate, ShippingDisplay};
use shipping_core::{
    assessment::{EffectiveDeliveryTerms, ShippingAssessment, assess},
    coverage::{
        CoverageCompleteness, DeliveryArea, ReferenceCountry, ShippingCoverage, ShippingZone,
    },
    inputs::{AmountThreshold, Applicability, AssessmentInput, SourceShippingClass},
    pricing::{ChargeBasis, QualifyingScope, ShippingPrice, ShippingRate},
    selection::ProfileSelection,
    terms::{
        CollectionDependency, CollectionPolicy, DeliveryTerms, ServiceFeature, ShippingEligibility,
        ShippingPolicy, ShippingService, ShippingServiceCode, ShippingServiceTerms,
    },
    time::{CollectionArrangement, LeadTimes, TimeBasis, TimeRange, TimeUnit},
};

const RELEASE: ReferenceRelease = ReferenceRelease::IsoCodes4_20_1;
const REGIONS: [&str; 40] = [
    "FR", "AT", "NL", "BE", "IT", "ES", "PL", "DK", "SE", "FI", "NO", "IE", "PT", "CZ", "SK", "HU",
    "SI", "HR", "RO", "BG", "GR", "LT", "LV", "EE", "LU", "MT", "CY", "IS", "LI", "MC", "SM", "AD",
    "AL", "BA", "ME", "MK", "RS", "MD", "UA", "TR",
];

pub struct LargePolicy {
    pub effective: EffectiveDeliveryTerms,
    pub generic: ShippingAssessment,
    pub personalized: ShippingAssessment,
    pub destination: GeographicDescription,
}

fn eur(minor: u64) -> Price {
    Price::new(minor.into(), Currency::Eur)
}
fn service(code: &str, terms: ShippingServiceTerms) -> ShippingService {
    ShippingService::new(ShippingServiceCode::new(code.to_owned()).unwrap(), terms)
}
fn rate(price: ShippingPrice, basis: Option<ChargeBasis>) -> Option<ShippingRate> {
    Some(ShippingRate::new(price, basis, None, Default::default()))
}
fn country(code: &str) -> DeliveryArea {
    DeliveryArea::Country(ReferenceCountry::new(code, RELEASE).unwrap())
}

/// `regions` repeated regional zones sit between the domestic and international zones.
pub fn large_policy(regions: usize) -> LargePolicy {
    let fixture = shipping_core::fixtures::gallery();
    let source = fixture.source_id;
    let offered = Some(ShippingEligibility::Offered);
    let domestic = ShippingZone::new(
        vec![country("DE")],
        Applicability::default(),
        vec![
            service(
                "PARCEL",
                ShippingServiceTerms {
                    eligibility: offered,
                    rate: rate(ShippingPrice::Fixed(eur(1_200)), Some(ChargeBasis::PerItem)),
                    lead_times: LeadTimes {
                        handling: Some(
                            TimeRange::new(1, 2, TimeUnit::Days, Some(TimeBasis::Business), None)
                                .unwrap(),
                        ),
                        ..Default::default()
                    },
                    features: [ServiceFeature::Insured, ServiceFeature::Tracking].into(),
                    ..Default::default()
                },
            ),
            service(
                "FREIGHT",
                ShippingServiceTerms {
                    applicability: Applicability::new(
                        None,
                        None,
                        None,
                        Some(AmountThreshold {
                            minimum: eur(500_000),
                            scope: QualifyingScope::SellerOrder,
                        }),
                    )
                    .unwrap(),
                    eligibility: offered,
                    rate: rate(
                        ShippingPrice::Free(Some(Currency::Eur)),
                        Some(ChargeBasis::PerSellerOrder),
                    ),
                    ..Default::default()
                },
            ),
            service(
                "FURNITURE_FREIGHT",
                ShippingServiceTerms {
                    applicability: Applicability::new(
                        Some(SourceShippingClass::new(source, "furniture".to_owned()).unwrap()),
                        None,
                        None,
                        None,
                    )
                    .unwrap(),
                    eligibility: offered,
                    rate: rate(
                        ShippingPrice::Fixed(eur(15_000)),
                        Some(ChargeBasis::PerItem),
                    ),
                    ..Default::default()
                },
            ),
        ],
    )
    .unwrap();
    let regional = REGIONS[..regions].iter().map(|code| {
        ShippingZone::new(
            vec![country(code)],
            Applicability::default(),
            vec![
                service(
                    "PARCEL",
                    ShippingServiceTerms {
                        eligibility: offered,
                        rate: rate(ShippingPrice::Fixed(eur(2_500)), Some(ChargeBasis::PerItem)),
                        features: [ServiceFeature::Tracking].into(),
                        ..Default::default()
                    },
                ),
                service(
                    "FREIGHT",
                    ShippingServiceTerms {
                        eligibility: offered,
                        rate: rate(
                            ShippingPrice::From(eur(20_000)),
                            Some(ChargeBasis::PerShipment),
                        ),
                        ..Default::default()
                    },
                ),
            ],
        )
        .unwrap()
    });
    let international = ShippingZone::new(
        ["US", "CA", "AU", "JP", "CH", "GB"]
            .into_iter()
            .map(country)
            .collect(),
        Applicability::default(),
        vec![service(
            "INTERNATIONAL",
            ShippingServiceTerms {
                eligibility: Some(ShippingEligibility::SubjectToConfirmation),
                rate: rate(ShippingPrice::OnRequest(Some(Currency::Eur)), None),
                ..Default::default()
            },
        )],
    )
    .unwrap();
    let terms = DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                RELEASE,
                CoverageCompleteness::Partial,
                Applicability::default(),
                vec![],
                std::iter::once(domestic)
                    .chain(regional)
                    .chain(std::iter::once(international))
                    .collect(),
                None,
            )
            .unwrap(),
        )),
        Some(CollectionPolicy::new(
            Applicability::default(),
            CollectionDependency::Independent,
            offered,
            None,
            Some(CollectionArrangement::ByAppointment),
        )),
    );
    let configuration = shipping_core::selection::ListingDeliveryConfiguration::new(
        source,
        ProfileSelection::inline(terms),
        Default::default(),
        Default::default(),
    )
    .unwrap();
    let effective = EffectiveDeliveryTerms::resolve(&configuration, None, &[]);
    let destination =
        GeographicDescription::new_in_release(RELEASE, None, Some(CountryCode::DEU), None, None)
            .unwrap()
            .unwrap();
    let generic = assess(&effective, &AssessmentInput::default(), &[]);
    let personalized = assess(
        &effective,
        &AssessmentInput {
            destination: Some(destination.clone()),
            ..Default::default()
        },
        &[],
    );
    LargePolicy {
        effective,
        generic,
        personalized,
        destination,
    }
}

/// Test-local display conversion: EUR is identity, USD multiplies by 1.1 with HalfUp rounding.
pub fn converted(price: ShippingPrice, currency: Currency) -> Option<ShippingPrice> {
    let convert = |p: Price| {
        let minor = u64::from(p.monetary_amount);
        let minor = if currency == Currency::Usd {
            (minor * 11 + 5) / 10
        } else {
            minor
        };
        Price::new(minor.into(), currency)
    };
    Some(match price {
        ShippingPrice::Free(Some(_)) => ShippingPrice::Free(Some(currency)),
        ShippingPrice::Fixed(p) => ShippingPrice::Fixed(convert(p)),
        ShippingPrice::From(p) => ShippingPrice::From(convert(p)),
        ShippingPrice::EstimatedRange(r) => ShippingPrice::EstimatedRange(
            shipping_core::pricing::PriceRange::new(convert(r.minimum()), convert(r.maximum()))
                .unwrap(),
        ),
        _ => return None,
    })
}

/// Display values for every convertible advertised rate, as listing enrichment supplies them.
pub fn display(effective: &EffectiveDeliveryTerms, currency: Currency) -> ShippingDisplay {
    let mut rates = Vec::new();
    if let Some(ShippingPolicy::Coverage(coverage)) = effective.terms().shipping() {
        for (zone_index, zone) in coverage.zones().iter().enumerate() {
            for (service_index, service) in zone.services().iter().enumerate() {
                if let Some(price) = service.rate().and_then(|r| converted(r.price(), currency)) {
                    rates.push(ConvertedShippingRate {
                        zone_index,
                        service_index,
                        price,
                    });
                }
            }
        }
    }
    ShippingDisplay {
        currency,
        fx_rate_id: "fx_01h455vb4pex5vy7enb1p677v4".parse().unwrap(),
        captured_at: time::macros::datetime!(2026-10-23 06:00 UTC),
        rates,
    }
}
