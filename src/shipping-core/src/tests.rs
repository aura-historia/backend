use crate::{
    InvalidShippingValue,
    assessment::{
        AssessedCharge, AssessmentReason as Reason, EffectiveDeliveryTerms, MissingInput,
        PredicateResult, ShippingAssessment, evaluate_applicability, evaluate_area,
    },
    coverage::{
        CoverageCompleteness, DeliveryArea, PostalForm, PostalRule, PostalRuleVersion,
        ReferenceCountry, ShippingCoverage, ShippingZone, UnresolvedArea,
    },
    fixtures::{
        GALLERY_JSON, GALLERY_REFERENCE_RELEASE, UPDATED_GALLERY_RATE_MINOR, gallery, gallery_terms,
    },
    inputs::{
        AmountThreshold, Applicability, AssessmentInput, ClassMembership, FactCompleteness,
        ListingRateInputs, QuantityRange, SourceShippingClass, SourceVariant,
    },
    pricing::{
        ChargeBasis, ChargeComponent, ComponentInclusion, PriceRange, QualifyingScope,
        ShippingPrice, ShippingRate,
    },
    profile::{
        ShippingProfile, ShippingProfileError, ShippingProfileId, ShippingProfileLifecycle,
        ShippingProfileName, ShippingProfileRevision,
    },
    selection::{
        ComponentOverride, DeliveryOverrides, ListingDeliveryConfiguration, ProfileSelection,
        SourceDefaultProfile,
    },
    terms::{
        CollectionDependency, CollectionPolicy, DeliveryActor, DeliverySite, DeliveryTerms,
        ServiceFeature, ShippingEligibility as Eligibility, ShippingPolicy, ShippingService,
        ShippingServiceCode, ShippingServiceTerms, SiteAssertions,
    },
    time::{CollectionArrangement, LeadTimes, TimeBasis, TimeRange, TimeTrigger, TimeUnit},
};
use geo::{
    AddressText, CountryCode, GeoPoint, GeographicDescription, PostalCode, SpatialPosition,
    SpatialPrecision, SubdivisionCode, reference::ReferenceRelease,
};
use listing_source_core::ListingSourceId;
use money::{Currency, Price};
use party_core::party_location::{
    PartyLocation, PartyLocationContent, PartyLocationLabel, PartyLocationLifecycle,
    PartyLocationRole,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU64,
};

fn source() -> ListingSourceId {
    "ls_01h455vb4pex5vy7enb1p677v1".parse().unwrap()
}
fn other_source() -> ListingSourceId {
    "ls_01h455vb4pex5vy7enb1p677v4".parse().unwrap()
}
fn amount(value: u64) -> Price {
    Price::new(value.into(), Currency::Eur)
}
fn count(value: u64) -> NonZeroU64 {
    NonZeroU64::new(value).unwrap()
}
fn country(code: &str) -> DeliveryArea {
    DeliveryArea::Country(ReferenceCountry::new(code, ReferenceRelease::CURRENT).unwrap())
}
fn destination(code: CountryCode, postal: Option<&str>) -> AssessmentInput {
    AssessmentInput {
        destination: GeographicDescription::new(
            None,
            Some(code),
            None,
            postal.map(|v| PostalCode::new(v).unwrap()),
        )
        .unwrap(),
        quantity: Some(count(1)),
        ..Default::default()
    }
}
fn service(
    code: &str,
    eligibility: Option<Eligibility>,
    price: Option<ShippingPrice>,
    basis: Option<ChargeBasis>,
    applicability: Applicability,
) -> ShippingService {
    ShippingService::new(
        ShippingServiceCode::new(code.to_owned()).unwrap(),
        ShippingServiceTerms {
            eligibility,
            applicability,
            rate: price.map(|price| ShippingRate::new(price, basis, None, BTreeMap::new())),
            ..Default::default()
        },
    )
}
fn offered(code: &str) -> ShippingService {
    service(
        code,
        Some(Eligibility::Offered),
        Some(ShippingPrice::Fixed(amount(8000))),
        Some(ChargeBasis::PerItem),
        Applicability::default(),
    )
}
fn zone(area: DeliveryArea, services: Vec<ShippingService>) -> ShippingZone {
    ShippingZone::new(vec![area], Applicability::default(), services).unwrap()
}
fn coverage(
    completeness: CoverageCompleteness,
    exclusions: Vec<DeliveryArea>,
    zones: Vec<ShippingZone>,
    origin: Option<DeliverySite>,
) -> ShippingPolicy {
    ShippingPolicy::coverage(
        ShippingCoverage::new(
            ReferenceRelease::CURRENT,
            completeness,
            Applicability::default(),
            exclusions,
            zones,
            origin,
        )
        .unwrap(),
    )
}
fn assess_terms(
    terms: DeliveryTerms,
    facts: ListingRateInputs,
    input: &AssessmentInput,
    locations: &[PartyLocation],
) -> ShippingAssessment {
    let config = ListingDeliveryConfiguration::new(
        source(),
        ProfileSelection::inline(terms),
        facts,
        DeliveryOverrides::default(),
    )
    .unwrap();
    EffectiveDeliveryTerms::resolve(&config, None, &[]).assess(input, locations)
}
fn assess_policy(policy: ShippingPolicy, input: &AssessmentInput) -> ShippingAssessment {
    assess_terms(
        DeliveryTerms::new(Some(policy), None),
        ListingRateInputs::default(),
        input,
        &[],
    )
}
fn postal(form: PostalForm) -> DeliveryArea {
    DeliveryArea::Postal(
        PostalRule::new(
            PostalRuleVersion::V1,
            ReferenceCountry::new("DE", ReferenceRelease::CURRENT).unwrap(),
            form,
        )
        .unwrap(),
    )
}
fn location(geography: Option<GeographicDescription>) -> PartyLocation {
    PartyLocation::create(
        "ploc_01h455vb4pex5vy7enb1p677v5".parse().unwrap(),
        "pty_01h455vb4pex5vy7enb1p677v2".parse().unwrap(),
        PartyLocationContent {
            label: PartyLocationLabel::new("Synthetic warehouse".to_owned()).unwrap(),
            roles: BTreeSet::from([PartyLocationRole::Warehouse]),
            geography,
            position: None,
        },
    )
}

#[test]
fn gallery_profile_updates_painting_but_preserves_cabinet_prohibition_and_collection() {
    let mut fixture = gallery();
    let input = fixture.assessment_input();
    let evaluate = |profile: &ShippingProfile, config: &ListingDeliveryConfiguration| {
        EffectiveDeliveryTerms::resolve(
            config,
            Some(&fixture.source_default),
            std::slice::from_ref(profile),
        )
        .assess(&input, &[])
    };
    let painting = evaluate(&fixture.profile, &fixture.painting);
    let cabinet = evaluate(&fixture.profile, &fixture.cabinet);
    assert_eq!(Some(Eligibility::Offered), painting.eligibility);
    assert_eq!(
        Some(AssessedCharge::Fixed(amount(8000))),
        painting.services[0].charge
    );
    assert_eq!(Some(Eligibility::NotOffered), cabinet.eligibility);
    assert_eq!(Some(Eligibility::Offered), cabinet.collection.eligibility);
    assert_eq!(
        Some(CollectionArrangement::ByAppointment),
        cabinet.collection.policy.as_ref().unwrap().arrangement()
    );
    let mut content = fixture.profile.content().clone();
    content.terms = gallery_terms(UPDATED_GALLERY_RATE_MINOR);
    assert!(fixture.profile.replace_content(content).unwrap().changed());
    let new_painting = evaluate(&fixture.profile, &fixture.painting);
    let new_cabinet = evaluate(&fixture.profile, &fixture.cabinet);
    assert_eq!(
        Some(AssessedCharge::Fixed(amount(9500))),
        new_painting.services[0].charge
    );
    assert_eq!(Some(Eligibility::NotOffered), new_cabinet.eligibility);
    assert_eq!(cabinet.collection, new_cabinet.collection);
    assert_eq!(2, new_painting.profile_basis.unwrap().revision.into_inner());
    assert_eq!(
        fixture.profile.id(),
        new_painting.profile_basis.unwrap().profile_id
    );
}

#[test]
fn price_request_eligibility_confirmation_and_absent_assertions_are_independent() {
    for (eligibility, price) in [
        (
            Some(Eligibility::Offered),
            Some(ShippingPrice::OnRequest(None)),
        ),
        (
            Some(Eligibility::SubjectToConfirmation),
            Some(ShippingPrice::Fixed(amount(8000))),
        ),
        (None, Some(ShippingPrice::OnRequest(None))),
        (Some(Eligibility::Offered), None),
    ] {
        let result = assess_policy(
            coverage(
                CoverageCompleteness::Partial,
                vec![],
                vec![zone(
                    country("DE"),
                    vec![service(
                        "service",
                        eligibility,
                        price,
                        Some(ChargeBasis::PerItem),
                        Applicability::default(),
                    )],
                )],
                None,
            ),
            &destination(CountryCode::DEU, None),
        );
        assert_eq!(eligibility, result.eligibility);
        if price == Some(ShippingPrice::OnRequest(None)) && eligibility.is_some() {
            assert!(result.reasons.contains(&Reason::PriceOnRequest));
        }
        if eligibility.is_none() {
            assert!(result.reasons.contains(&Reason::NoEligibilityAssertion));
        }
        if price.is_none() {
            assert!(result.services[0].service.rate().is_none());
            assert!(result.services[0].charge.is_none());
        }
    }
}

#[test]
fn missing_destination_country_postcode_quantity_and_variant_are_specific() {
    let rule = coverage(
        CoverageCompleteness::Complete,
        vec![],
        vec![zone(country("DE"), vec![offered("road")])],
        None,
    );
    assert!(
        assess_policy(rule.clone(), &AssessmentInput::default())
            .reasons
            .contains(&Reason::MissingInput(MissingInput::Destination))
    );
    let text_only = AssessmentInput {
        destination: GeographicDescription::new(
            Some(AddressText::new("Synthetic destination assertion").unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
        ..Default::default()
    };
    assert!(
        assess_policy(rule.clone(), &text_only)
            .reasons
            .contains(&Reason::MissingInput(MissingInput::DestinationCountry))
    );
    let mut input = destination(CountryCode::DEU, None);
    input.quantity = None;
    let result = assess_policy(rule, &input);
    assert_eq!(Some(Eligibility::Offered), result.eligibility);
    assert_eq!(None, result.services[0].charge);
    assert!(
        result
            .reasons
            .contains(&Reason::MissingInput(MissingInput::Quantity))
    );
    let area = postal(PostalForm::GermanPrefix("01".to_owned()));
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(MissingInput::PostalCode)]),
        evaluate_area(&area, ReferenceRelease::CURRENT, &input, None, &[])
    );
    let variant = SourceVariant::new(source(), "small".to_owned()).unwrap();
    let applicability = Applicability::new(None, Some(vec![variant]), None, None).unwrap();
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(MissingInput::Variant)]),
        evaluate_applicability(&applicability, &ListingRateInputs::default(), &input)
    );
}

#[test]
fn partial_coverage_stays_unknown_and_complete_coverage_can_establish_a_negative() {
    let input = destination(CountryCode::FRA, None);
    for completeness in [
        CoverageCompleteness::Partial,
        CoverageCompleteness::Complete,
    ] {
        let result = assess_policy(
            coverage(
                completeness,
                vec![],
                vec![zone(country("DE"), vec![offered("road")])],
                None,
            ),
            &input,
        );
        assert_eq!(
            if completeness == CoverageCompleteness::Complete {
                Some(Eligibility::NotOffered)
            } else {
                None
            },
            result.eligibility
        );
    }
    assert_eq!(
        Some(Eligibility::NotOffered),
        assess_policy(ShippingPolicy::Prohibited, &AssessmentInput::default()).eligibility
    );
    assert_eq!(
        None,
        assess_terms(
            DeliveryTerms::default(),
            ListingRateInputs::default(),
            &input,
            &[]
        )
        .eligibility
    );
}

#[test]
fn unresolved_postal_exclusion_blocks_an_otherwise_matching_zone() {
    let exclusions = vec![postal(PostalForm::GermanRange {
        first: "01000".to_owned(),
        last: "01999".to_owned(),
    })];
    let policy = coverage(
        CoverageCompleteness::Complete,
        exclusions,
        vec![zone(country("DE"), vec![offered("road")])],
        None,
    );
    let unknown = assess_policy(policy.clone(), &destination(CountryCode::DEU, None));
    assert_eq!(None, unknown.eligibility);
    assert_eq!(None, unknown.selected_zone);
    assert!(unknown.services.is_empty());
    assert!(
        unknown
            .reasons
            .contains(&Reason::MissingInput(MissingInput::PostalCode))
    );
    let excluded = assess_policy(
        policy.clone(),
        &destination(CountryCode::DEU, Some("01001")),
    );
    assert_eq!(Some(Eligibility::NotOffered), excluded.eligibility);
    assert_eq!(vec![Reason::Excluded], excluded.reasons);
    assert_eq!(
        Some(Eligibility::Offered),
        assess_policy(policy, &destination(CountryCode::DEU, Some("10000"))).eligibility
    );
}

#[test]
fn uncertain_earlier_zone_is_not_skipped_and_declared_order_wins_over_specificity() {
    let policy = coverage(
        CoverageCompleteness::Complete,
        vec![],
        vec![
            zone(
                postal(PostalForm::GermanPrefix("01".to_owned())),
                vec![offered("earlier")],
            ),
            zone(country("DE"), vec![offered("later")]),
        ],
        None,
    );
    let result = assess_policy(policy, &destination(CountryCode::DEU, None));
    assert_eq!(None, result.eligibility);
    assert!(
        result
            .reasons
            .contains(&Reason::EarlierZoneUndetermined { index: 0 })
    );
    let policy = coverage(
        CoverageCompleteness::Complete,
        vec![],
        vec![
            zone(DeliveryArea::Anywhere, vec![offered("first")]),
            zone(country("DE"), vec![offered("specific")]),
        ],
        None,
    );
    let result = assess_policy(policy, &destination(CountryCode::DEU, None));
    assert_eq!(Some(0), result.selected_zone);
    assert_eq!("first", result.services[0].service.code().as_str());
}

#[test]
fn unsupported_areas_and_exclusions_never_become_permissive() {
    let unresolved = DeliveryArea::Unresolved(
        UnresolvedArea::new("Synthetic unreviewed island group".to_owned()).unwrap(),
    );
    for (exclusions, zones) in [
        (
            vec![unresolved.clone()],
            vec![zone(DeliveryArea::Anywhere, vec![offered("road")])],
        ),
        (
            vec![],
            vec![
                zone(unresolved, vec![offered("islands")]),
                zone(DeliveryArea::Anywhere, vec![offered("road")]),
            ],
        ),
    ] {
        let result = assess_policy(
            coverage(CoverageCompleteness::Complete, exclusions, zones, None),
            &destination(CountryCode::DEU, None),
        );
        assert_eq!(None, result.eligibility);
        assert!(result.reasons.contains(&Reason::UnresolvedArea));
    }
}

#[test]
fn every_service_in_the_selected_zone_is_evaluated_separately() {
    let services = vec![
        offered("road"),
        service(
            "express",
            Some(Eligibility::SubjectToConfirmation),
            Some(ShippingPrice::OnRequest(None)),
            None,
            Applicability::default(),
        ),
        service(
            "air",
            Some(Eligibility::NotOffered),
            None,
            None,
            Applicability::default(),
        ),
    ];
    let result = assess_policy(
        coverage(
            CoverageCompleteness::Complete,
            vec![],
            vec![zone(country("DE"), services)],
            None,
        ),
        &destination(CountryCode::DEU, None),
    );
    assert_eq!(3, result.services.len());
    assert_eq!(Some(Eligibility::Offered), result.eligibility);
    assert_eq!(
        Some(AssessedCharge::Fixed(amount(8000))),
        result.services[0].charge
    );
    assert_eq!(
        Some(Eligibility::SubjectToConfirmation),
        result.services[1].eligibility
    );
    assert_eq!(None, result.services[1].charge);
    assert_eq!(
        Some(Eligibility::NotOffered),
        result.services[2].eligibility
    );
}

#[test]
fn mixed_variant_applicability_requires_selection_and_class_membership_never_sets_a_price() {
    let small = SourceVariant::new(source(), "small".to_owned()).unwrap();
    let large = SourceVariant::new(source(), "large".to_owned()).unwrap();
    let class = SourceShippingClass::new(source(), "furniture".to_owned()).unwrap();
    let facts = ListingRateInputs::new(
        Some(ClassMembership::new(vec![class.clone()], FactCompleteness::Complete).unwrap()),
        Some(vec![small.clone(), large.clone()]),
    )
    .unwrap();
    let applicability =
        Applicability::new(Some(class), Some(vec![small.clone()]), None, None).unwrap();
    let services = vec![service(
        "road",
        Some(Eligibility::Offered),
        None,
        None,
        applicability.clone(),
    )];
    let terms = DeliveryTerms::new(
        Some(coverage(
            CoverageCompleteness::Complete,
            vec![],
            vec![zone(country("DE"), services)],
            None,
        )),
        None,
    );
    let mut input = destination(CountryCode::DEU, None);
    let result = assess_terms(terms.clone(), facts.clone(), &input, &[]);
    assert_eq!(None, result.eligibility);
    assert!(result.reasons.contains(&Reason::MixedVariantApplicability));
    input.selected_variant = Some(small);
    let result = assess_terms(terms.clone(), facts.clone(), &input, &[]);
    assert_eq!(Some(Eligibility::Offered), result.eligibility);
    assert!(result.services[0].service.rate().is_none());
    assert_eq!(None, result.services[0].charge);
    input.selected_variant = Some(large);
    assert_eq!(
        Some(Eligibility::NotOffered),
        assess_terms(terms, facts, &input, &[]).eligibility
    );
    input.selected_variant = Some(SourceVariant::new(other_source(), "small".to_owned()).unwrap());
    assert!(
        evaluate_applicability(&applicability, &ListingRateInputs::default(), &input)
            .reasons()
            .contains(&Reason::SourceMismatch)
    );
}

#[test]
fn applicability_uses_three_state_conjunction_and_scoped_thresholds() {
    let class = SourceShippingClass::new(source(), "oversize".to_owned()).unwrap();
    let rule = Applicability::new(
        Some(class.clone()),
        None,
        Some(QuantityRange::new(count(2), count(4)).unwrap()),
        Some(AmountThreshold {
            minimum: amount(10000),
            scope: QualifyingScope::SellerOrder,
        }),
    )
    .unwrap();
    let known = ListingRateInputs::new(
        Some(ClassMembership::new(vec![class], FactCompleteness::Complete).unwrap()),
        None,
    )
    .unwrap();
    let missing = evaluate_applicability(
        &rule,
        &ListingRateInputs::default(),
        &AssessmentInput::default(),
    );
    assert_eq!(
        PredicateResult::Undetermined(vec![
            Reason::MissingInput(MissingInput::ClassMembership),
            Reason::MissingInput(MissingInput::Quantity),
            Reason::MissingInput(MissingInput::QualifyingAmount(QualifyingScope::SellerOrder))
        ]),
        missing
    );
    let mut input = AssessmentInput {
        quantity: Some(count(2)),
        seller_order_value: Some(amount(10000)),
        ..Default::default()
    };
    assert_eq!(
        PredicateResult::Match,
        evaluate_applicability(&rule, &known, &input)
    );
    input.seller_order_value = Some(amount(9999));
    assert_eq!(
        PredicateResult::NoMatch(vec![Reason::QualifyingThresholdNotMet]),
        evaluate_applicability(&rule, &known, &input)
    );
    input.seller_order_value = Some(Price::new(10000_u64.into(), Currency::Usd));
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::CurrencyMismatch]),
        evaluate_applicability(&rule, &known, &input)
    );
    input.quantity = Some(count(5));
    assert_eq!(
        PredicateResult::NoMatch(vec![Reason::QuantityOutsideRange]),
        evaluate_applicability(&rule, &known, &input)
    );
    let partial = ListingRateInputs::new(
        Some(ClassMembership::new(vec![], FactCompleteness::Partial).unwrap()),
        None,
    )
    .unwrap();
    input.quantity = Some(count(2));
    input.seller_order_value = Some(amount(10000));
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::IncompleteClassMembership]),
        evaluate_applicability(&rule, &partial, &input)
    );
}

#[test]
fn complete_coverage_outside_its_applicability_does_not_assert_a_prohibition() {
    let applicability = Applicability::new(
        None,
        None,
        Some(QuantityRange::new(count(2), count(3)).unwrap()),
        None,
    )
    .unwrap();
    let policy = ShippingPolicy::coverage(
        ShippingCoverage::new(
            ReferenceRelease::CURRENT,
            CoverageCompleteness::Complete,
            applicability,
            vec![],
            vec![zone(country("DE"), vec![offered("road")])],
            None,
        )
        .unwrap(),
    );
    let result = assess_policy(policy, &destination(CountryCode::FRA, None));
    assert_eq!(None, result.eligibility);
    assert_eq!(vec![Reason::QuantityOutsideRange], result.reasons);
}

#[test]
fn charge_bases_have_independent_counts_and_unknown_basis_has_no_computed_charge() {
    let mut input = destination(CountryCode::DEU, None);
    input.quantity = Some(count(2));
    input.lots = Some(count(3));
    input.shipments = Some(count(4));
    input.seller_orders = Some(count(5));
    for (basis, expected) in [
        (Some(ChargeBasis::PerItem), Some(16000)),
        (Some(ChargeBasis::PerLot), Some(24000)),
        (Some(ChargeBasis::PerShipment), Some(32000)),
        (Some(ChargeBasis::PerSellerOrder), Some(40000)),
        (None, None),
    ] {
        let services = vec![service(
            "rate",
            Some(Eligibility::Offered),
            Some(ShippingPrice::Fixed(amount(8000))),
            basis,
            Applicability::default(),
        )];
        let result = assess_policy(
            coverage(
                CoverageCompleteness::Partial,
                vec![],
                vec![zone(country("DE"), services)],
                None,
            ),
            &input,
        );
        assert_eq!(
            expected.map(|v| AssessedCharge::Fixed(amount(v))),
            result.services[0].charge
        );
        if basis.is_none() {
            assert!(result.reasons.contains(&Reason::UnknownChargeBasis));
        }
    }
    input.lots = None;
    input.shipments = None;
    input.seller_orders = None;
    for (basis, missing) in [
        (ChargeBasis::PerLot, MissingInput::Lots),
        (ChargeBasis::PerShipment, MissingInput::Shipments),
        (ChargeBasis::PerSellerOrder, MissingInput::SellerOrders),
    ] {
        let services = vec![service(
            "rate",
            Some(Eligibility::Offered),
            Some(ShippingPrice::Fixed(amount(8000))),
            Some(basis),
            Applicability::default(),
        )];
        let result = assess_policy(
            coverage(
                CoverageCompleteness::Partial,
                vec![],
                vec![zone(country("DE"), services)],
                None,
            ),
            &input,
        );
        assert_eq!(None, result.services[0].charge);
        assert!(result.reasons.contains(&Reason::MissingInput(missing)));
    }
}

#[test]
fn free_fixed_from_range_external_and_requested_prices_keep_their_semantics() {
    let range = PriceRange::new(amount(1000), amount(2000)).unwrap();
    let prices = [
        ShippingPrice::Free(Some(Currency::Eur)),
        ShippingPrice::Fixed(amount(1000)),
        ShippingPrice::From(amount(1000)),
        ShippingPrice::EstimatedRange(range),
        ShippingPrice::ExternallyCalculated(None),
        ShippingPrice::OnRequest(None),
    ];
    let mut input = destination(CountryCode::DEU, None);
    input.quantity = Some(count(2));
    let expected = [
        Some(AssessedCharge::Fixed(amount(0))),
        Some(AssessedCharge::Fixed(amount(2000))),
        Some(AssessedCharge::From(amount(2000))),
        Some(AssessedCharge::EstimatedRange(
            PriceRange::new(amount(2000), amount(4000)).unwrap(),
        )),
        None,
        None,
    ];
    for (price, expected) in prices.into_iter().zip(expected) {
        let services = vec![service(
            "rate",
            Some(Eligibility::Offered),
            Some(price),
            Some(ChargeBasis::PerItem),
            Applicability::default(),
        )];
        let result = assess_policy(
            coverage(
                CoverageCompleteness::Partial,
                vec![],
                vec![zone(country("DE"), services)],
                None,
            ),
            &input,
        );
        assert_eq!(Some(Eligibility::Offered), result.eligibility);
        assert_eq!(expected, result.services[0].charge);
        assert_eq!(price, result.services[0].service.rate().unwrap().price());
    }
}

#[test]
fn monetary_bounds_and_arithmetic_fail_closed() {
    assert_eq!(
        Err(InvalidShippingValue::Bounds),
        PriceRange::new(amount(2), amount(1))
    );
    assert_eq!(
        Err(InvalidShippingValue::CurrencyMismatch),
        PriceRange::new(amount(1), Price::new(2_u64.into(), Currency::Usd))
    );
    let mut input = destination(CountryCode::DEU, None);
    input.quantity = Some(count(2));
    for price in [
        ShippingPrice::Fixed(amount(u64::MAX)),
        ShippingPrice::From(amount(u64::MAX)),
        ShippingPrice::EstimatedRange(PriceRange::new(amount(1), amount(u64::MAX)).unwrap()),
    ] {
        let services = vec![service(
            "rate",
            Some(Eligibility::Offered),
            Some(price),
            Some(ChargeBasis::PerItem),
            Applicability::default(),
        )];
        let result = assess_policy(
            coverage(
                CoverageCompleteness::Complete,
                vec![],
                vec![zone(country("DE"), services)],
                None,
            ),
            &input,
        );
        assert_eq!(None, result.services[0].charge);
        assert!(result.reasons.contains(&Reason::ArithmeticOverflow));
    }
}

#[test]
fn service_roles_features_components_and_time_ranges_are_preserved_independently() {
    let handling = TimeRange::new(
        1,
        2,
        TimeUnit::Days,
        Some(TimeBasis::Business),
        Some(TimeTrigger::Payment),
    )
    .unwrap();
    let transit = TimeRange::new(
        3,
        5,
        TimeUnit::Days,
        Some(TimeBasis::Calendar),
        Some(TimeTrigger::Dispatch),
    )
    .unwrap();
    let total = TimeRange::new(
        1,
        2,
        TimeUnit::Weeks,
        None,
        Some(TimeTrigger::OrderConfirmation),
    )
    .unwrap();
    assert_eq!(
        Err(InvalidShippingValue::Bounds),
        TimeRange::new(5, 1, TimeUnit::Days, None, None)
    );
    assert_eq!(
        0,
        TimeRange::new(0, 0, TimeUnit::Hours, None, None)
            .unwrap()
            .minimum()
    );
    let performer = DeliveryActor::Party("pty_01h455vb4pex5vy7enb1p677v2".parse().unwrap());
    let components = BTreeMap::from([
        (ChargeComponent::Packing, ComponentInclusion::Included),
        (ChargeComponent::ImportDuty, ComponentInclusion::Excluded),
    ]);
    let service = ShippingService::new(
        ShippingServiceCode::new("white-glove".to_owned()).unwrap(),
        ShippingServiceTerms {
            eligibility: Some(Eligibility::Offered),
            rate: Some(ShippingRate::new(
                ShippingPrice::Fixed(amount(8000)),
                Some(ChargeBasis::PerItem),
                Some(QualifyingScope::SellerOrder),
                components.clone(),
            )),
            lead_times: LeadTimes {
                handling: Some(handling),
                transit: Some(transit),
                total: Some(total),
            },
            features: BTreeSet::from([ServiceFeature::WhiteGlove]),
            arranger: Some(DeliveryActor::Buyer),
            performer: Some(performer),
            ..Default::default()
        },
    );
    let result = assess_policy(
        coverage(
            CoverageCompleteness::Partial,
            vec![],
            vec![zone(country("DE"), vec![service])],
            None,
        ),
        &destination(CountryCode::DEU, None),
    );
    let service = &result.services[0].service;
    assert_eq!(Some(DeliveryActor::Buyer), service.arranger());
    assert_eq!(Some(performer), service.performer());
    assert_eq!(Some(handling), service.lead_times().handling);
    assert_eq!(Some(transit), service.lead_times().transit);
    assert_eq!(Some(total), service.lead_times().total);
    assert_eq!(&components, service.rate().unwrap().components());
    assert_eq!(
        Some(QualifyingScope::SellerOrder),
        service.rate().unwrap().qualifying_scope()
    );
    assert!(service.features().contains(&ServiceFeature::WhiteGlove));
}

#[test]
fn domestic_requires_an_explicit_usable_origin_and_does_not_infer_one_from_a_warehouse() {
    let mut warehouse =
        location(GeographicDescription::new(None, Some(CountryCode::DEU), None, None).unwrap());
    let input = destination(CountryCode::DEU, None);
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(MissingInput::OriginCountry)]),
        evaluate_area(
            &DeliveryArea::Domestic,
            ReferenceRelease::CURRENT,
            &input,
            None,
            std::slice::from_ref(&warehouse)
        )
    );
    let origin = DeliverySite::PartyLocation(warehouse.id());
    assert_eq!(
        PredicateResult::Match,
        evaluate_area(
            &DeliveryArea::Domestic,
            ReferenceRelease::CURRENT,
            &input,
            Some(&origin),
            std::slice::from_ref(&warehouse)
        )
    );
    warehouse.set_lifecycle(PartyLocationLifecycle::Retired);
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::LocationRetired(warehouse.id())]),
        evaluate_area(
            &DeliveryArea::Domestic,
            ReferenceRelease::CURRENT,
            &input,
            Some(&origin),
            std::slice::from_ref(&warehouse)
        )
    );
    // An explicit country rule does not depend on origin, even if the supplied origin is retired.
    let independent = coverage(
        CoverageCompleteness::Partial,
        vec![],
        vec![zone(country("DE"), vec![offered("road")])],
        Some(origin),
    );
    assert_eq!(
        Some(Eligibility::Offered),
        assess_terms(
            DeliveryTerms::new(Some(independent), None),
            ListingRateInputs::default(),
            &input,
            &[warehouse]
        )
        .eligibility
    );
    let point_only = DeliverySite::Asserted(
        SiteAssertions::new(
            None,
            Some(
                SpatialPosition::new(
                    GeoPoint::new(0.0, 0.0).unwrap(),
                    SpatialPrecision::Premises,
                    None,
                )
                .unwrap(),
            ),
        )
        .unwrap(),
    );
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(MissingInput::OriginCountry)]),
        evaluate_area(
            &DeliveryArea::Domestic,
            ReferenceRelease::CURRENT,
            &input,
            Some(&point_only),
            &[]
        )
    );
}

#[test]
fn whole_location_assertions_never_fill_each_other_and_collection_sites_must_be_eligible() {
    let warehouse =
        location(GeographicDescription::new(None, Some(CountryCode::DEU), None, None).unwrap());
    let partial = DeliverySite::Asserted(
        SiteAssertions::new(
            GeographicDescription::new(
                Some(AddressText::new("Synthetic street only").unwrap()),
                None,
                None,
                None,
            )
            .unwrap(),
            None,
        )
        .unwrap(),
    );
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(MissingInput::OriginCountry)]),
        evaluate_area(
            &DeliveryArea::Domestic,
            ReferenceRelease::CURRENT,
            &destination(CountryCode::DEU, None),
            Some(&partial),
            std::slice::from_ref(&warehouse)
        )
    );
    let collection = CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Independent,
        Some(Eligibility::Offered),
        Some(DeliverySite::PartyLocation(warehouse.id())),
        Some(CollectionArrangement::Window(
            TimeRange::new(
                1,
                3,
                TimeUnit::Days,
                Some(TimeBasis::Calendar),
                Some(TimeTrigger::ReadyForCollection),
            )
            .unwrap(),
        )),
    );
    let terms = DeliveryTerms::new(Some(ShippingPolicy::Prohibited), Some(collection));
    let result = assess_terms(
        terms.clone(),
        ListingRateInputs::default(),
        &AssessmentInput::default(),
        &[],
    );
    assert_eq!(None, result.collection.eligibility);
    assert_eq!(
        vec![Reason::LocationAbsent(warehouse.id())],
        result.collection.reasons
    );
    let mut retired = warehouse.clone();
    retired.set_lifecycle(PartyLocationLifecycle::Retired);
    assert!(
        assess_terms(
            terms.clone(),
            ListingRateInputs::default(),
            &AssessmentInput::default(),
            &[retired]
        )
        .collection
        .reasons
        .contains(&Reason::LocationRetired(warehouse.id()))
    );
    assert_eq!(
        Some(Eligibility::Offered),
        assess_terms(
            terms,
            ListingRateInputs::default(),
            &AssessmentInput::default(),
            &[warehouse]
        )
        .collection
        .eligibility
    );
    assert_eq!(
        Err(InvalidShippingValue::EmptySite),
        SiteAssertions::new(None, None)
    );
}

#[test]
fn suppression_clearing_prohibition_and_removing_override_are_distinct_and_noops_are_explicit() {
    let fixture = gallery();
    let mut config = fixture.painting.clone();
    let evaluate = |config: &ListingDeliveryConfiguration| {
        EffectiveDeliveryTerms::resolve(
            config,
            Some(&fixture.source_default),
            std::slice::from_ref(&fixture.profile),
        )
        .assess(&fixture.assessment_input(), &[])
    };
    assert!(
        !config
            .select(ProfileSelection::InheritSourceDefault)
            .unwrap()
            .changed()
    );
    assert!(
        config
            .select(ProfileSelection::SuppressInheritance)
            .unwrap()
            .changed()
    );
    let suppressed = evaluate(&config);
    assert_eq!(None, suppressed.eligibility);
    assert_eq!(None, suppressed.collection.eligibility);
    assert!(suppressed.reasons.contains(&Reason::SuppressedInheritance));
    config
        .select(ProfileSelection::InheritSourceDefault)
        .unwrap();
    config
        .replace_overrides(DeliveryOverrides {
            shipping: ComponentOverride::Clear,
            collection: ComponentOverride::Inherit,
        })
        .unwrap();
    let cleared = evaluate(&config);
    assert_eq!(None, cleared.eligibility);
    assert!(cleared.reasons.contains(&Reason::AssertionCleared));
    assert_eq!(Some(Eligibility::Offered), cleared.collection.eligibility);
    config
        .replace_overrides(DeliveryOverrides {
            shipping: ComponentOverride::Replace(ShippingPolicy::Prohibited),
            collection: ComponentOverride::Inherit,
        })
        .unwrap();
    assert_eq!(Some(Eligibility::NotOffered), evaluate(&config).eligibility);
    assert!(
        config
            .replace_overrides(DeliveryOverrides::default())
            .unwrap()
            .changed()
    );
    assert_eq!(Some(Eligibility::Offered), evaluate(&config).eligibility);
    assert!(
        !config
            .replace_overrides(DeliveryOverrides::default())
            .unwrap()
            .changed()
    );
    assert!(
        !config
            .replace_rate_inputs(ListingRateInputs::default())
            .unwrap()
            .changed()
    );
}

#[test]
fn a_shipping_override_preserves_only_independently_applicable_collection() {
    let fixture = gallery();
    let dependent = CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Shipping,
        Some(Eligibility::Offered),
        None,
        Some(CollectionArrangement::ByAppointment),
    );
    let mut config = ListingDeliveryConfiguration::new(
        source(),
        ProfileSelection::inline(DeliveryTerms::new(
            Some(ShippingPolicy::Prohibited),
            Some(dependent.clone()),
        )),
        ListingRateInputs::default(),
        DeliveryOverrides::default(),
    )
    .unwrap();
    assert_eq!(
        None,
        EffectiveDeliveryTerms::resolve(&config, None, &[])
            .assess(&AssessmentInput::default(), &[])
            .collection
            .eligibility
    );
    config
        .replace_overrides(DeliveryOverrides {
            shipping: ComponentOverride::Replace(ShippingPolicy::Prohibited),
            collection: ComponentOverride::Inherit,
        })
        .unwrap();
    let result = EffectiveDeliveryTerms::resolve(&config, None, &[])
        .assess(&AssessmentInput::default(), &[]);
    assert!(result.collection.policy.is_none());
    assert!(
        result
            .collection
            .reasons
            .contains(&Reason::ShippingDependencyReplaced)
    );
    let limited = CollectionPolicy::new(
        Applicability::new(
            None,
            None,
            Some(QuantityRange::new(count(2), count(3)).unwrap()),
            None,
        )
        .unwrap(),
        CollectionDependency::Independent,
        Some(Eligibility::Offered),
        None,
        None,
    );
    config
        .replace_overrides(DeliveryOverrides {
            shipping: ComponentOverride::Replace(ShippingPolicy::Prohibited),
            collection: ComponentOverride::Replace(limited),
        })
        .unwrap();
    assert_eq!(
        None,
        EffectiveDeliveryTerms::resolve(&config, None, &[])
            .assess(&fixture.assessment_input(), &[])
            .collection
            .eligibility
    );
}

#[test]
fn absent_retired_and_cross_source_profiles_are_deterministic_without_fallback() {
    let mut fixture = gallery();
    let config = &fixture.painting;
    let missing = EffectiveDeliveryTerms::resolve(config, Some(&fixture.source_default), &[])
        .assess(&fixture.assessment_input(), &[]);
    assert_eq!(None, missing.eligibility);
    assert_eq!(
        vec![Reason::ProfileAbsent(fixture.profile.id())],
        missing.reasons
    );
    let absent_default =
        EffectiveDeliveryTerms::resolve(config, None, std::slice::from_ref(&fixture.profile))
            .assess(&fixture.assessment_input(), &[]);
    assert_eq!(vec![Reason::NoSourceDefault], absent_default.reasons);
    fixture.profile.retire().unwrap();
    let retired = EffectiveDeliveryTerms::resolve(
        config,
        Some(&fixture.source_default),
        std::slice::from_ref(&fixture.profile),
    )
    .assess(&fixture.assessment_input(), &[]);
    assert_eq!(
        vec![Reason::ProfileRetired(fixture.profile.id())],
        retired.reasons
    );
    // An independent complete override remains assessable despite the retired base.
    let cabinet = EffectiveDeliveryTerms::resolve(
        &fixture.cabinet,
        Some(&fixture.source_default),
        std::slice::from_ref(&fixture.profile),
    )
    .assess(&fixture.assessment_input(), &[]);
    assert_eq!(Some(Eligibility::NotOffered), cabinet.eligibility);
    assert_eq!(None, cabinet.collection.eligibility);
    fixture.profile.restore().unwrap();
    let other = ShippingProfile::create(
        fixture.profile.id(),
        other_source(),
        fixture.profile.content().clone(),
    )
    .unwrap();
    assert_eq!(
        vec![Reason::SourceMismatch],
        EffectiveDeliveryTerms::resolve(config, Some(&fixture.source_default), &[other])
            .assess(&fixture.assessment_input(), &[])
            .reasons
    );
    let wrong_default = SourceDefaultProfile::rehydrate(other_source(), Some(fixture.profile.id()));
    assert_eq!(
        &[Reason::SourceMismatch],
        EffectiveDeliveryTerms::resolve(
            config,
            Some(&wrong_default),
            std::slice::from_ref(&fixture.profile)
        )
        .shipping_reasons()
    );
    let mut source_default = SourceDefaultProfile::empty(other_source());
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        source_default.select(Some(&fixture.profile))
    );
}

#[test]
fn same_names_party_and_content_do_not_share_profile_identity_or_source_defaults() {
    let fixture = gallery();
    let other_id: ShippingProfileId = "shp_01h455vb4pex5vy7enb1p677v6".parse().unwrap();
    let other =
        ShippingProfile::create(other_id, other_source(), fixture.profile.content().clone())
            .unwrap();
    assert_eq!(fixture.profile.content(), other.content());
    assert_ne!(fixture.profile.id(), other.id());
    let config = ListingDeliveryConfiguration::new(
        other_source(),
        ProfileSelection::InheritSourceDefault,
        ListingRateInputs::default(),
        DeliveryOverrides::default(),
    )
    .unwrap();
    assert_eq!(
        &[Reason::NoSourceDefault],
        EffectiveDeliveryTerms::resolve(&config, None, &[fixture.profile, other])
            .shipping_reasons()
    );
}

#[test]
fn constructors_and_rehydration_enforce_source_invariants_and_failed_mutations_are_atomic() {
    let class = SourceShippingClass::new(other_source(), "furniture".to_owned()).unwrap();
    let applicability = Applicability::new(Some(class.clone()), None, None, None).unwrap();
    let invalid_terms = DeliveryTerms::new(
        Some(coverage(
            CoverageCompleteness::Complete,
            vec![],
            vec![zone(
                country("DE"),
                vec![service(
                    "road",
                    Some(Eligibility::Offered),
                    None,
                    None,
                    applicability,
                )],
            )],
            None,
        )),
        None,
    );
    let mut fixture = gallery();
    let mut content = fixture.profile.content().clone();
    content.terms = invalid_terms.clone();
    assert_eq!(
        Err(ShippingProfileError::Invalid(
            InvalidShippingValue::SourceMismatch
        )),
        ShippingProfile::create(fixture.profile.id(), source(), content.clone())
    );
    assert_eq!(
        Err(ShippingProfileError::Invalid(
            InvalidShippingValue::SourceMismatch
        )),
        ShippingProfile::rehydrate(
            fixture.profile.id(),
            source(),
            ShippingProfileRevision::INITIAL,
            ShippingProfileLifecycle::Active,
            content.clone()
        )
    );
    let before = fixture.profile.clone();
    assert!(fixture.profile.replace_content(content).is_err());
    assert_eq!(before, fixture.profile);
    let before = fixture.painting.clone();
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        fixture
            .painting
            .select(ProfileSelection::inline(invalid_terms))
    );
    assert_eq!(before, fixture.painting);
    let facts = ListingRateInputs::new(
        Some(ClassMembership::new(vec![class], FactCompleteness::Complete).unwrap()),
        None,
    )
    .unwrap();
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        fixture.painting.replace_rate_inputs(facts)
    );
    assert_eq!(before, fixture.painting);
}

#[test]
fn profile_lifecycle_and_revision_exhaustion_allow_only_explicit_noops() {
    let mut fixture = gallery();
    let profile = &mut fixture.profile;
    assert!(
        !profile
            .replace_content(profile.content().clone())
            .unwrap()
            .changed()
    );
    assert_eq!(1, profile.revision().into_inner());
    assert!(profile.retire().unwrap().changed());
    assert!(!profile.retire().unwrap().changed());
    assert!(
        !profile
            .replace_content(profile.content().clone())
            .unwrap()
            .changed()
    );
    let mut content = profile.content().clone();
    content.name = ShippingProfileName::new("Renamed synthetic profile".to_owned()).unwrap();
    assert_eq!(
        Err(ShippingProfileError::Retired),
        profile.replace_content(content.clone())
    );
    assert_eq!(2, profile.revision().into_inner());
    assert!(profile.restore().unwrap().changed());
    assert!(profile.replace_content(content).unwrap().changed());
    assert_eq!(4, profile.revision().into_inner());
    assert!(ShippingProfileRevision::try_from(0_u64).is_err());
    let mut exhausted = ShippingProfile::rehydrate(
        profile.id(),
        source(),
        ShippingProfileRevision::try_from(u64::MAX).unwrap(),
        ShippingProfileLifecycle::Active,
        profile.content().clone(),
    )
    .unwrap();
    assert!(
        !exhausted
            .replace_content(exhausted.content().clone())
            .unwrap()
            .changed()
    );
    let mut content = exhausted.content().clone();
    content.terms = gallery_terms(9500);
    let before = exhausted.clone();
    assert_eq!(
        Err(ShippingProfileError::RevisionExhausted),
        exhausted.replace_content(content)
    );
    assert_eq!(before, exhausted);
    assert_eq!(
        Err(ShippingProfileError::RevisionExhausted),
        exhausted.retire()
    );
    assert_eq!(before, exhausted);
}

#[test]
fn postal_rules_are_versioned_bounded_country_scoped_and_preserve_leading_zeroes() {
    let de = ReferenceCountry::new("DE", ReferenceRelease::CURRENT).unwrap();
    let fr = ReferenceCountry::new("FR", ReferenceRelease::CURRENT).unwrap();
    for form in [
        PostalForm::GermanPrefix("".to_owned()),
        PostalForm::GermanPrefix("01000".to_owned()),
        PostalForm::GermanPrefix("0.*".to_owned()),
        PostalForm::GermanPrefix("é".to_owned()),
        PostalForm::GermanRange {
            first: "01000".to_owned(),
            last: "00100".to_owned(),
        },
        PostalForm::GermanRange {
            first: "1000".to_owned(),
            last: "99999".to_owned(),
        },
    ] {
        assert_eq!(
            Err(InvalidShippingValue::PostalRule),
            PostalRule::new(PostalRuleVersion::V1, de, form)
        );
    }
    assert_eq!(
        Err(InvalidShippingValue::PostalRule),
        PostalRule::new(
            PostalRuleVersion::V1,
            fr,
            PostalForm::GermanPrefix("01".to_owned())
        )
    );
    assert_eq!(None, PostalRuleVersion::from_code("POSTAL_RULE_V2"));
    let area = postal(PostalForm::GermanRange {
        first: "01000".to_owned(),
        last: "01999".to_owned(),
    });
    for code in ["01000", "01999"] {
        assert_eq!(
            PredicateResult::Match,
            evaluate_area(
                &area,
                ReferenceRelease::CURRENT,
                &destination(CountryCode::DEU, Some(code)),
                None,
                &[]
            )
        );
    }
    assert_eq!(
        PredicateResult::NoMatch(vec![Reason::PostalMismatch]),
        evaluate_area(
            &area,
            ReferenceRelease::CURRENT,
            &destination(CountryCode::DEU, Some("02000")),
            None,
            &[]
        )
    );
    for code in ["1000", "01000 ", "01-000", "０１０００"] {
        assert_eq!(
            PredicateResult::Undetermined(vec![Reason::UnsupportedPostalShape]),
            evaluate_area(
                &area,
                ReferenceRelease::CURRENT,
                &destination(CountryCode::DEU, Some(code)),
                None,
                &[]
            )
        );
    }
    assert_eq!(
        PredicateResult::NoMatch(vec![Reason::CountryMismatch]),
        evaluate_area(
            &area,
            ReferenceRelease::CURRENT,
            &destination(CountryCode::FRA, Some("01000")),
            None,
            &[]
        )
    );
    let ca = DeliveryArea::Postal(
        PostalRule::new(
            PostalRuleVersion::V1,
            ReferenceCountry::new("CA", ReferenceRelease::CURRENT).unwrap(),
            PostalForm::Exact(PostalCode::new("K1A 0B1").unwrap()),
        )
        .unwrap(),
    );
    assert_eq!(
        PredicateResult::Match,
        evaluate_area(
            &ca,
            ReferenceRelease::CURRENT,
            &destination(CountryCode::CAN, Some("k1a0b1")),
            None,
            &[]
        )
    );
}

#[test]
fn reference_releases_are_explicit_and_subdivision_matching_never_guesses_ancestry() {
    assert!(ReferenceRelease::from_code("iso-codes-future").is_err());
    assert!(ReferenceCountry::new("de", ReferenceRelease::CURRENT).is_err());
    assert!(ReferenceCountry::new("EU", ReferenceRelease::CURRENT).is_err());
    assert!(SubdivisionCode::from_code("DE-NOT-A-SUBDIVISION", ReferenceRelease::CURRENT).is_err());
    let bavaria = SubdivisionCode::from_code("DE-BY", ReferenceRelease::CURRENT).unwrap();
    let area = DeliveryArea::Subdivision(bavaria);
    let input = AssessmentInput {
        destination: GeographicDescription::new_in_release(
            ReferenceRelease::CURRENT,
            None,
            None,
            Some(bavaria),
            None,
        )
        .unwrap(),
        ..Default::default()
    };
    assert_eq!(
        PredicateResult::Match,
        evaluate_area(&area, ReferenceRelease::CURRENT, &input, None, &[])
    );
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(
            MissingInput::DestinationSubdivision
        )]),
        evaluate_area(
            &area,
            ReferenceRelease::CURRENT,
            &destination(CountryCode::DEU, None),
            None,
            &[]
        )
    );
    let england = DeliveryArea::Subdivision(SubdivisionCode::new("GB-ENG").unwrap());
    let city = AssessmentInput {
        destination: GeographicDescription::new(
            None,
            Some(CountryCode::GBR),
            Some(SubdivisionCode::new("GB-LND").unwrap()),
            None,
        )
        .unwrap(),
        ..Default::default()
    };
    assert_eq!(
        PredicateResult::NoMatch(vec![Reason::SubdivisionMismatch]),
        evaluate_area(&england, ReferenceRelease::CURRENT, &city, None, &[])
    );
}

#[test]
fn rule_and_text_constructors_reject_invalid_or_unbounded_persisted_values() {
    for text in ["", " ", "bad\ntext", "bad\u{2028}text", &"é".repeat(128)] {
        assert!(ShippingProfileName::new(text.to_owned()).is_err());
    }
    assert!(ShippingServiceCode::new("x".repeat(129)).is_err());
    assert!(SourceShippingClass::new(source(), "x\0y".to_owned()).is_err());
    assert_eq!(
        Err(InvalidShippingValue::Bounds),
        QuantityRange::new(count(2), count(1))
    );
    assert!(ShippingZone::new(vec![], Applicability::default(), vec![offered("road")]).is_err());
    assert!(
        ShippingZone::new(
            vec![country("DE")],
            Applicability::default(),
            vec![offered("same"), offered("same")]
        )
        .is_err()
    );
    assert!(
        ShippingCoverage::new(
            ReferenceRelease::CURRENT,
            CoverageCompleteness::Complete,
            Applicability::default(),
            vec![country("DE"); 65],
            vec![],
            None
        )
        .is_err()
    );
    let class = SourceShippingClass::new(source(), "same".to_owned()).unwrap();
    assert!(ClassMembership::new(vec![class.clone(), class], FactCompleteness::Complete).is_err());
    assert!(ListingRateInputs::new(None, Some(vec![])).is_err());
    assert!(Applicability::new(None, Some(vec![]), None, None).is_err());
}

#[test]
fn profile_id_public_and_storage_codecs_are_strict_and_the_shared_fixture_agrees() {
    let fixture = gallery();
    let id = fixture.profile.id();
    assert_eq!("shp", ShippingProfileId::PREFIX);
    assert_eq!(7, id.as_uuid().get_version_num());
    assert_eq!(id, id.to_string().parse().unwrap());
    assert_eq!(id, ShippingProfileId::try_from(id.into_uuid()).unwrap());
    assert!(
        id.as_uuid()
            .to_string()
            .parse::<ShippingProfileId>()
            .is_err()
    );
    assert!(
        id.to_string()
            .replace("shp_", "ls_")
            .parse::<ShippingProfileId>()
            .is_err()
    );
    assert!(
        id.to_string()
            .to_uppercase()
            .parse::<ShippingProfileId>()
            .is_err()
    );
    assert!(ShippingProfileId::try_from(uuid::Uuid::new_v4()).is_err());
    assert!(ShippingProfileId::try_from(uuid::Uuid::nil()).is_err());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(
        id,
        serde_json::from_str::<ShippingProfileId>(&json).unwrap()
    );
    assert!(serde_json::from_str::<ShippingProfileId>(&format!("\"{}\"", id.as_uuid())).is_err());
    let data: serde_json::Value = serde_json::from_str(GALLERY_JSON).unwrap();
    assert_eq!(fixture.source_id.to_string(), data["listingSourceId"]);
    assert_eq!(fixture.party_id.to_string(), data["attributedPartyId"]);
    assert_eq!(id.to_string(), data["shippingProfileId"]);
    assert_eq!(GALLERY_REFERENCE_RELEASE.as_str(), data["referenceRelease"]);
    assert_eq!(
        GALLERY_REFERENCE_RELEASE,
        fixture
            .assessment_input()
            .destination
            .unwrap()
            .reference_release()
    );
    assert_eq!(8000, data["shipping"]["initialAmountMinor"]);
    assert_eq!(
        UPDATED_GALLERY_RATE_MINOR,
        data["shipping"]["updatedAmountMinor"]
    );
    assert_eq!("EUR", data["shipping"]["currency"]);
    assert_eq!(
        Some(Eligibility::Offered),
        Eligibility::from_code("OFFERED")
    );
    assert_eq!(None, Eligibility::from_code("UNKNOWN"));
    assert_eq!(None, Eligibility::from_code("offered"));
}

#[test]
fn duplicate_dependency_snapshots_fail_closed() {
    let fixture = gallery();
    let input = fixture.assessment_input();
    let effective = EffectiveDeliveryTerms::resolve(
        &fixture.painting,
        Some(&fixture.source_default),
        &[fixture.profile.clone(), fixture.profile],
    );
    assert_eq!(&[Reason::AmbiguousDependency], effective.shipping_reasons());
    let location =
        location(GeographicDescription::new(None, Some(CountryCode::DEU), None, None).unwrap());
    let origin = DeliverySite::PartyLocation(location.id());
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::AmbiguousDependency]),
        evaluate_area(
            &DeliveryArea::Domestic,
            ReferenceRelease::CURRENT,
            &input,
            Some(&origin),
            &[location.clone(), location]
        )
    );
}

#[test]
fn explicit_and_inline_selection_use_their_own_terms_without_a_source_default() {
    let fixture = gallery();
    let mut config = fixture.painting.clone();
    let mut content = fixture.profile.content().clone();
    content.terms = gallery_terms(12000);
    let explicit = ShippingProfile::create(
        "shp_01h455vb4pex5vy7enb1p677v6".parse().unwrap(),
        source(),
        content,
    )
    .unwrap();
    config
        .select(ProfileSelection::Explicit(explicit.id()))
        .unwrap();
    let result = EffectiveDeliveryTerms::resolve(
        &config,
        Some(&fixture.source_default),
        &[fixture.profile.clone(), explicit.clone()],
    )
    .assess(&fixture.assessment_input(), &[]);
    assert_eq!(explicit.id(), result.profile_basis.unwrap().profile_id);
    assert_eq!(
        Some(AssessedCharge::Fixed(amount(12000))),
        result.services[0].charge
    );
    let missing = EffectiveDeliveryTerms::resolve(
        &config,
        Some(&fixture.source_default),
        std::slice::from_ref(&fixture.profile),
    );
    assert_eq!(
        &[Reason::ProfileAbsent(explicit.id())],
        missing.shipping_reasons()
    );
    config
        .select(ProfileSelection::inline(DeliveryTerms::new(
            Some(ShippingPolicy::Prohibited),
            None,
        )))
        .unwrap();
    let result = EffectiveDeliveryTerms::resolve(&config, None, &[])
        .assess(&AssessmentInput::default(), &[]);
    assert_eq!(Some(Eligibility::NotOffered), result.eligibility);
    assert_eq!(None, result.profile_basis);
}

#[test]
fn default_assignment_noops_and_retired_clearing_preserve_explicit_identity() {
    let mut fixture = gallery();
    let profile_id = fixture.profile.id();
    assert!(
        !fixture
            .source_default
            .select(Some(&fixture.profile))
            .unwrap()
            .changed()
    );
    fixture.profile.retire().unwrap();
    assert_eq!(
        Err(InvalidShippingValue::RetiredProfile),
        fixture.source_default.select(Some(&fixture.profile))
    );
    assert_eq!(Some(profile_id), fixture.source_default.profile_id());
    assert!(fixture.source_default.select(None).unwrap().changed());
    assert!(!fixture.source_default.select(None).unwrap().changed());
    let restored = ShippingProfile::rehydrate(
        fixture.profile.id(),
        fixture.profile.source_id(),
        fixture.profile.revision(),
        fixture.profile.lifecycle(),
        fixture.profile.content().clone(),
    )
    .unwrap();
    assert_eq!(fixture.profile, restored);
    assert_eq!(
        None,
        ShippingProfileRevision::try_from(u64::MAX)
            .unwrap()
            .checked_next()
    );
}

#[test]
fn thresholds_cannot_use_an_unrelated_subtotal_and_unselected_variants_must_agree() {
    let threshold = Applicability::new(
        None,
        None,
        None,
        Some(AmountThreshold {
            minimum: amount(10000),
            scope: QualifyingScope::SellerOrder,
        }),
    )
    .unwrap();
    let input = AssessmentInput {
        item_value: Some(amount(1000000)),
        ..Default::default()
    };
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::MissingInput(MissingInput::QualifyingAmount(
            QualifyingScope::SellerOrder
        ))]),
        evaluate_applicability(&threshold, &ListingRateInputs::default(), &input)
    );
    let small = SourceVariant::new(source(), "small".to_owned()).unwrap();
    let large = SourceVariant::new(source(), "large".to_owned()).unwrap();
    let available = ListingRateInputs::new(None, Some(vec![small.clone(), large.clone()])).unwrap();
    let shared = Applicability::new(None, Some(vec![small, large]), None, None).unwrap();
    assert_eq!(
        PredicateResult::Match,
        evaluate_applicability(&shared, &available, &AssessmentInput::default())
    );
    let other = Applicability::new(
        None,
        Some(vec![
            SourceVariant::new(source(), "other".to_owned()).unwrap(),
        ]),
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        PredicateResult::NoMatch(vec![Reason::VariantMismatch]),
        evaluate_applicability(&other, &available, &AssessmentInput::default())
    );
}

#[test]
fn known_exclusion_after_an_unresolved_one_still_establishes_a_negative() {
    let policy = coverage(
        CoverageCompleteness::Partial,
        vec![
            DeliveryArea::Unresolved(
                UnresolvedArea::new("Synthetic unreviewed area".to_owned()).unwrap(),
            ),
            country("DE"),
        ],
        vec![zone(DeliveryArea::Anywhere, vec![offered("road")])],
        None,
    );
    let result = assess_policy(policy, &destination(CountryCode::DEU, None));
    assert_eq!(Some(Eligibility::NotOffered), result.eligibility);
    assert_eq!(vec![Reason::Excluded], result.reasons);
}

#[test]
fn profile_uuid_variant_and_assessment_diagnostics_do_not_bypass_boundaries() {
    let fixture = gallery();
    let mut bytes = fixture.profile.id().into_uuid().into_bytes();
    bytes[8] &= 0x7f;
    assert!(ShippingProfileId::try_from(uuid::Uuid::from_bytes(bytes)).is_err());
    let input = AssessmentInput {
        destination: GeographicDescription::new(
            Some(AddressText::new("Private synthetic buyer street").unwrap()),
            Some(CountryCode::DEU),
            None,
            Some(PostalCode::new("01001").unwrap()),
        )
        .unwrap(),
        ..Default::default()
    };
    assert_eq!("AssessmentInput(<redacted>)", format!("{input:?}"));
}

#[test]
fn nonnumeric_prices_preserve_known_and_absent_currency_without_inventing_an_amount() {
    for price in [
        ShippingPrice::Free(None),
        ShippingPrice::OnRequest(None),
        ShippingPrice::OnRequest(Some(Currency::Usd)),
        ShippingPrice::ExternallyCalculated(None),
        ShippingPrice::ExternallyCalculated(Some(Currency::Usd)),
    ] {
        let services = vec![service(
            "rate",
            Some(Eligibility::Offered),
            Some(price),
            Some(ChargeBasis::PerItem),
            Applicability::default(),
        )];
        let result = assess_policy(
            coverage(
                CoverageCompleteness::Partial,
                vec![],
                vec![zone(country("DE"), services)],
                None,
            ),
            &destination(CountryCode::DEU, None),
        );
        assert_eq!(Some(Eligibility::Offered), result.eligibility);
        assert_eq!(None, result.services[0].charge);
        assert_eq!(price, result.services[0].service.rate().unwrap().price());
        if price == ShippingPrice::Free(None) {
            assert!(result.reasons.contains(&Reason::NoCurrencyAssertion));
        }
    }
    assert_eq!(None, ShippingPrice::Free(None).currency());
    assert_eq!(
        Some(Currency::Usd),
        ShippingPrice::OnRequest(Some(Currency::Usd)).currency()
    );
    assert_eq!(
        Some(Currency::Eur),
        ShippingPrice::Fixed(amount(1)).currency()
    );
}

#[test]
fn fact_and_applicability_constructors_reject_mixed_source_scopes() {
    let class = SourceShippingClass::new(source(), "same".to_owned()).unwrap();
    let other_class = SourceShippingClass::new(other_source(), "same".to_owned()).unwrap();
    let variant = SourceVariant::new(source(), "same".to_owned()).unwrap();
    let other_variant = SourceVariant::new(other_source(), "same".to_owned()).unwrap();
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        ClassMembership::new(vec![class.clone(), other_class], FactCompleteness::Complete)
    );
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        ListingRateInputs::new(None, Some(vec![variant.clone(), other_variant.clone()]))
    );
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        Applicability::new(None, Some(vec![variant, other_variant.clone()]), None, None)
    );
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        Applicability::new(
            Some(class.clone()),
            Some(vec![other_variant.clone()]),
            None,
            None
        )
    );
    assert_eq!(
        Err(InvalidShippingValue::SourceMismatch),
        ListingRateInputs::new(
            Some(ClassMembership::new(vec![class], FactCompleteness::Complete).unwrap()),
            Some(vec![other_variant])
        )
    );
}

#[test]
fn a_service_prohibition_does_not_complete_partial_shipping_coverage() {
    for completeness in [
        CoverageCompleteness::Partial,
        CoverageCompleteness::Complete,
    ] {
        let result = assess_policy(
            coverage(
                completeness,
                vec![],
                vec![zone(
                    country("DE"),
                    vec![service(
                        "air",
                        Some(Eligibility::NotOffered),
                        None,
                        None,
                        Applicability::default(),
                    )],
                )],
                None,
            ),
            &destination(CountryCode::DEU, None),
        );
        assert_eq!(
            Some(Eligibility::NotOffered),
            result.services[0].eligibility
        );
        if completeness == CoverageCompleteness::Complete {
            assert_eq!(Some(Eligibility::NotOffered), result.eligibility);
        } else {
            assert_eq!(None, result.eligibility);
            assert!(result.reasons.contains(&Reason::IncompleteCoverage));
        }
    }
}

#[test]
fn selected_variants_are_validated_even_without_variant_restrictions() {
    let fixture = gallery();
    let available = SourceVariant::new(source(), "available".to_owned()).unwrap();
    let facts = ListingRateInputs::new(None, Some(vec![available.clone()])).unwrap();
    let mut config = fixture.painting.clone();
    config.replace_rate_inputs(facts).unwrap();
    let effective = EffectiveDeliveryTerms::resolve(
        &config,
        Some(&fixture.source_default),
        std::slice::from_ref(&fixture.profile),
    );
    let mut input = fixture.assessment_input();
    input.selected_variant = Some(available);
    assert_eq!(
        Some(Eligibility::Offered),
        effective.assess(&input, &[]).eligibility
    );

    for (variant, reason) in [
        (
            SourceVariant::new(source(), "unavailable".to_owned()).unwrap(),
            Reason::VariantUnavailable,
        ),
        (
            SourceVariant::new(other_source(), "available".to_owned()).unwrap(),
            Reason::SourceMismatch,
        ),
    ] {
        input.selected_variant = Some(variant);
        let result = effective.assess(&input, &[]);
        assert_eq!(None, result.eligibility);
        assert_eq!(None, result.collection.eligibility);
        assert!(result.services.is_empty());
        assert_eq!(vec![reason.clone()], result.reasons);
        assert_eq!(vec![reason], result.collection.reasons);
    }

    // The configuration supplies source ownership even when no rate facts are known.
    let effective = EffectiveDeliveryTerms::resolve(
        &fixture.painting,
        Some(&fixture.source_default),
        std::slice::from_ref(&fixture.profile),
    );
    let result = effective.assess(&input, &[]);
    assert_eq!(None, result.eligibility);
    assert_eq!(vec![Reason::SourceMismatch], result.reasons);
    assert_eq!(None, result.collection.eligibility);
}

#[test]
fn cross_source_predicate_facts_remain_undetermined_instead_of_proving_nonmembership() {
    let class = SourceShippingClass::new(source(), "furniture".to_owned()).unwrap();
    let other_class = SourceShippingClass::new(other_source(), "furniture".to_owned()).unwrap();
    let rule = Applicability::new(Some(class), None, None, None).unwrap();
    let facts = ListingRateInputs::new(
        Some(ClassMembership::new(vec![other_class], FactCompleteness::Complete).unwrap()),
        None,
    )
    .unwrap();
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::SourceMismatch]),
        evaluate_applicability(&rule, &facts, &AssessmentInput::default())
    );

    let variant = SourceVariant::new(source(), "small".to_owned()).unwrap();
    let other_variant = SourceVariant::new(other_source(), "small".to_owned()).unwrap();
    let rule = Applicability::new(None, Some(vec![variant]), None, None).unwrap();
    let facts = ListingRateInputs::new(None, Some(vec![other_variant])).unwrap();
    assert_eq!(
        PredicateResult::Undetermined(vec![Reason::SourceMismatch]),
        evaluate_applicability(&rule, &facts, &AssessmentInput::default())
    );
}
