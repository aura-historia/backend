use application::error::{box_error, static_error};
use aws_sdk_geoplaces::{operation::geocode::GeocodeOutput, types::GeocodeResultItem};
use geo::{
    CountryCode, DerivedGeography, GeoPoint, GeographicDescription, SpatialPosition,
    SpatialPrecision, SubdivisionCode, country_from_code, reference::ReferenceRelease,
};
use geo_service::geocoding::{
    CandidateIssue, CandidateSet, GeocodingCandidate, GeocodingError, GeocodingFailure,
    GeocodingOutcome, GeocodingProvenance, GeocodingRequest, GeocodingResult, GeocodingUsage,
    ProviderMatchScore,
};

pub(crate) fn map_response(
    response: GeocodeOutput,
    request: &GeocodingRequest,
    review: Option<&str>,
) -> Result<GeocodingResult, GeocodingError> {
    let Some(items) = response.result_items else {
        return Err(invalid_response(
            "successful geocoding response omitted its result collection",
        ));
    };
    if items.len() > request.candidate_limit() {
        return Err(invalid_response(
            "provider exceeded requested candidate limit",
        ));
    }
    // AWS exposes this header as a string, not a closed enum. Request options determine
    // the documented pricing category; do not guess wire values from category names.
    if response.pricing_bucket.trim().is_empty() {
        return Err(invalid_response(
            "geocoding response omitted its pricing metadata",
        ));
    }
    let candidates = items
        .into_iter()
        .map(|item| map_candidate(item, request))
        .collect::<Result<Vec<_>, _>>()?;
    if request.purpose().requires_storage() {
        // Check returned country too: missing, contradictory or outside the reviewed market
        // must not acquire a persistence permission even when the provider ignored its filter.
        let expected = request.constraint().map(|c| c.associated_country());
        if candidates.iter().any(|candidate| {
            let country = candidate
                .geography()
                .and_then(DerivedGeography::description)
                .and_then(GeographicDescription::country);
            country.is_none() || country == Some(CountryCode::JPN) || country != expected
        }) {
            return Err(GeocodingError {
                kind: GeocodingFailure::StorageNotPermitted,
                source: static_error("returned geography is outside reviewed retention rights"),
            });
        }
    }
    let usage = match review {
        Some(review) => GeocodingUsage::retained(request.purpose(), review.to_owned()),
        None => GeocodingUsage::single_use(request.purpose()),
    };
    let outcome = if candidates.is_empty() {
        GeocodingOutcome::NoMatch
    } else {
        GeocodingOutcome::Candidates(CandidateSet::new(candidates, request.candidate_limit())?)
    };
    Ok(GeocodingResult { outcome, usage })
}

fn map_candidate(
    item: GeocodeResultItem,
    request: &GeocodingRequest,
) -> Result<GeocodingCandidate, GeocodingError> {
    if item.place_id.is_empty()
        || item.place_id.len() > 500
        || item.place_id.chars().any(char::is_control)
    {
        return Err(invalid_response("invalid provider external reference"));
    }
    let mut issues = Vec::new();
    let score = item
        .match_scores
        .as_ref()
        .map(|scores| {
            ProviderMatchScore::new(scores.overall)
                .ok_or_else(|| invalid_response("invalid provider match score"))
        })
        .transpose()?
        // This SDK models optional Overall as f64 and defaults an absent/null value to
        // zero. It cannot distinguish an actual zero from omitted evidence, so do not
        // report either as a supplied score. Nonzero scores still retain their meaning.
        .filter(|score| score.value() != 0.0);
    let country_data = item.address.as_ref().and_then(|a| a.country.as_ref());
    // Code2 and Code3 are documented ISO country fields. Only use Code3 on actual
    // Code2 absence, never to conceal an invalid present Code2.
    let country = country_data.and_then(|c| match c.code2.as_deref() {
        Some(code) => country_from_code(code, ReferenceRelease::CURRENT).ok(),
        None => c
            .code3
            .as_deref()
            .filter(|code| code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase()))
            .and_then(|code| CountryCode::for_alpha3(code).ok())
            .and_then(|country| {
                country_from_code(country.alpha2(), ReferenceRelease::CURRENT).ok()
            }),
    });
    if country.is_none() {
        issues.push(CandidateIssue::UnsupportedCountry);
    }
    if let (Some(country), Some(code3)) = (country, country_data.and_then(|c| c.code3.as_deref()))
        && code3 != country.alpha3()
    {
        issues.push(CandidateIssue::InconsistentComponents);
    }

    // AWS documents Region.Code as an abbreviation, not ISO 3166-2. The only approved
    // compatibility mapping is its documented Canadian BC example. Never prefix arbitrary
    // provider codes with the country or guess a code from a localized region name.
    let region = item.address.as_ref().and_then(|a| a.region.as_ref());
    let subdivision = match (country, region.and_then(|r| r.code.as_deref())) {
        (Some(CountryCode::CAN), Some("BC")) => Some(SubdivisionCode::new("CA-BC").map_err(
            |source| GeocodingError {
                kind: GeocodingFailure::InvalidResponse,
                source: box_error(source),
            },
        )?),
        _ => None,
    };
    if (region.is_some() && subdivision.is_none())
        || item
            .address
            .as_ref()
            .is_some_and(|a| a.sub_region.is_some())
    {
        issues.push(CandidateIssue::UnsupportedSubdivision);
    }

    let point = match item.position.as_deref() {
        None => {
            issues.push(CandidateIssue::MissingPosition);
            None
        }
        Some([longitude, latitude]) => Some(GeoPoint::new(*latitude, *longitude).map_err(
            |source| GeocodingError {
                kind: GeocodingFailure::InvalidResponse,
                source: box_error(source),
            },
        )?),
        Some(_) => {
            return Err(invalid_response(
                "provider position must contain longitude then latitude",
            ));
        }
    };
    let precision = match item.place_type.as_str() {
        "PointAddress" | "SecondaryAddress" if item.estimated_point_address != Some(true) => {
            Some(SpatialPrecision::Premises)
        }
        "InterpolatedAddress" | "Street" | "Intersection" => Some(SpatialPrecision::Street),
        "Locality" | "District" | "SubDistrict" => Some(SpatialPrecision::Locality),
        "Country" | "Region" | "SubRegion" | "PostalCode" | "Block" | "SubBlock"
        | "PointOfInterest" => Some(SpatialPrecision::Representative),
        "PointAddress" | "SecondaryAddress" | "InferredSecondaryAddress" => {
            issues.push(CandidateIssue::ApproximatePosition);
            Some(SpatialPrecision::Representative)
        }
        _ => {
            issues.push(CandidateIssue::UnsupportedPrecision);
            None
        }
    };
    if item.place_type.as_str() == "InterpolatedAddress" {
        issues.push(CandidateIssue::ApproximatePosition);
    }
    let position = point
        .zip(precision)
        .map(|(point, precision)| SpatialPosition::new(point, precision, None))
        .transpose()
        .map_err(|source| GeocodingError {
            kind: GeocodingFailure::InvalidResponse,
            source: box_error(source),
        })?;
    // No accuracy is supplied by this API. Distance to a bias, bounding boxes, score and
    // coordinate decimal places are not accuracy estimates.
    let description =
        GeographicDescription::new(None, country, subdivision, None).map_err(|source| {
            GeocodingError {
                kind: GeocodingFailure::InvalidResponse,
                source: box_error(source),
            }
        })?;
    let geography = if issues.contains(&CandidateIssue::InconsistentComponents) {
        None
    } else {
        DerivedGeography::new(description, position)
    };

    if let Some(constraint) = request.constraint() {
        match country {
            Some(actual) if actual != constraint.associated_country() => {
                issues.push(CandidateIssue::CountryConstraintMismatch)
            }
            None => issues.push(CandidateIssue::UnverifiedConstraint),
            _ => {}
        }
        if let Some(expected) = constraint.subdivision() {
            match subdivision {
                Some(actual) if actual != expected => {
                    issues.push(CandidateIssue::SubdivisionConstraintMismatch)
                }
                None => issues.push(CandidateIssue::UnverifiedConstraint),
                _ => {}
            }
        }
    }
    if let Some(submitted) = request.submitted() {
        let submitted = submitted.description();
        if let (Some(expected), Some(actual)) = (submitted.country(), country)
            && expected != actual
        {
            issues.push(CandidateIssue::SubmittedCountryMismatch);
        }
        if let Some(expected) = submitted.subdivision()
            && (country.is_some_and(|country| country != expected.country())
                || subdivision.is_some_and(|actual| expected != actual))
        {
            issues.push(CandidateIssue::SubmittedSubdivisionMismatch);
        }
    }
    Ok(GeocodingCandidate::new(
        geography,
        issues,
        score,
        GeocodingProvenance {
            provider: "AMAZON_LOCATION",
            product: "PLACES_V2_GEOCODE",
            data_source: "AMAZON_LOCATION_DEFAULT",
            region: super::REGION.to_owned(),
            interpretation_profile: super::TERMS_PROFILE,
            external_reference: item.place_id,
            attribution_url: "https://docs.aws.amazon.com/location/latest/developerguide/data-attribution.html",
        },
    ))
}

fn invalid_response(message: &'static str) -> GeocodingError {
    GeocodingError {
        kind: GeocodingFailure::InvalidResponse,
        source: static_error(message),
    }
}
