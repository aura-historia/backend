//! Shared synthetic source assertions for normalization, provider, projection and API tests.
//! No buyer data, random IDs, source payloads or external infrastructure are required.
//! The JSON is illustrative fixture vocabulary, not a transport or persistence codec.

use crate::{
    coverage::{
        CoverageCompleteness, DeliveryArea, ReferenceCountry, ShippingCoverage, ShippingZone,
    },
    inputs::{Applicability, AssessmentInput, ListingRateInputs},
    pricing::{ChargeBasis, ShippingPrice, ShippingRate},
    profile::{ShippingProfile, ShippingProfileContent, ShippingProfileId, ShippingProfileName},
    selection::{
        ComponentOverride, DeliveryOverrides, ListingDeliveryConfiguration, ProfileSelection,
        SourceDefaultProfile,
    },
    terms::{
        CollectionDependency, CollectionPolicy, DeliveryTerms, ShippingEligibility, ShippingPolicy,
        ShippingService, ShippingServiceCode, ShippingServiceTerms,
    },
    time::CollectionArrangement,
};
use geo::{CountryCode, GeographicDescription, reference::ReferenceRelease};
use listing_source_core::ListingSourceId;
use money::{Currency, Price};
use party_core::party_id::PartyId;
use std::{collections::BTreeMap, num::NonZeroU64};

pub const GALLERY_JSON: &str = include_str!("../fixtures/gallery.json");
pub const GALLERY_REFERENCE_RELEASE: ReferenceRelease = ReferenceRelease::IsoCodes4_20_1;
pub const INITIAL_GALLERY_RATE_MINOR: u64 = 8000;
pub const UPDATED_GALLERY_RATE_MINOR: u64 = 9500;

pub struct GalleryFixture {
    pub source_id: ListingSourceId,
    pub party_id: PartyId,
    pub profile: ShippingProfile,
    pub source_default: SourceDefaultProfile,
    pub painting: ListingDeliveryConfiguration,
    pub cabinet: ListingDeliveryConfiguration,
}
impl GalleryFixture {
    /// Synthetic country-only assessment; callers may supply their own private input.
    pub fn assessment_input(&self) -> AssessmentInput {
        AssessmentInput {
            destination: GeographicDescription::new_in_release(
                GALLERY_REFERENCE_RELEASE,
                None,
                Some(CountryCode::DEU),
                None,
                None,
            )
            .unwrap(),
            quantity: NonZeroU64::new(1),
            ..Default::default()
        }
    }
}

pub fn gallery_terms(amount_minor: u64) -> DeliveryTerms {
    let service = ShippingService::new(
        ShippingServiceCode::new("gallery-delivery".to_owned()).unwrap(),
        ShippingServiceTerms {
            eligibility: Some(ShippingEligibility::Offered),
            rate: Some(ShippingRate::new(
                ShippingPrice::Fixed(Price::new(amount_minor.into(), Currency::Eur)),
                Some(ChargeBasis::PerItem),
                None,
                BTreeMap::new(),
            )),
            ..Default::default()
        },
    );
    let zone = ShippingZone::new(
        vec![DeliveryArea::Country(
            ReferenceCountry::new("DE", GALLERY_REFERENCE_RELEASE).unwrap(),
        )],
        Applicability::default(),
        vec![service],
    )
    .unwrap();
    DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                GALLERY_REFERENCE_RELEASE,
                CoverageCompleteness::Partial,
                Applicability::default(),
                Vec::new(),
                vec![zone],
                None,
            )
            .unwrap(),
        )),
        Some(CollectionPolicy::new(
            Applicability::default(),
            CollectionDependency::Independent,
            Some(ShippingEligibility::Offered),
            None,
            Some(CollectionArrangement::ByAppointment),
        )),
    )
}

pub fn gallery() -> GalleryFixture {
    let source_id = "ls_01h455vb4pex5vy7enb1p677v1".parse().unwrap();
    let party_id = "pty_01h455vb4pex5vy7enb1p677v2".parse().unwrap();
    let profile_id: ShippingProfileId = "shp_01h455vb4pex5vy7enb1p677v3".parse().unwrap();
    let profile = ShippingProfile::create(
        profile_id,
        source_id,
        ShippingProfileContent {
            name: ShippingProfileName::new("Synthetic gallery delivery".to_owned()).unwrap(),
            attributed_party: Some(party_id),
            terms: gallery_terms(INITIAL_GALLERY_RATE_MINOR),
        },
    )
    .unwrap();
    let mut source_default = SourceDefaultProfile::empty(source_id);
    source_default.select(Some(&profile)).unwrap();
    let painting = ListingDeliveryConfiguration::new(
        source_id,
        ProfileSelection::InheritSourceDefault,
        ListingRateInputs::default(),
        DeliveryOverrides::default(),
    )
    .unwrap();
    let cabinet = ListingDeliveryConfiguration::new(
        source_id,
        ProfileSelection::InheritSourceDefault,
        ListingRateInputs::default(),
        DeliveryOverrides {
            shipping: ComponentOverride::Replace(ShippingPolicy::Prohibited),
            collection: ComponentOverride::Inherit,
        },
    )
    .unwrap();
    GalleryFixture {
        source_id,
        party_id,
        profile,
        source_default,
        painting,
        cabinet,
    }
}
