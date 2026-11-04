use application::error::{box_error, static_error};
use aws_sdk_geoplaces::{
    operation::geocode::GeocodeOutput,
    types::{Country, GeocodeResultItem},
};
use geo::{
    CountryCode, DerivedGeography, GeoPoint, GeographicDescription, PostalCode, SpatialPosition,
    SpatialPrecision, SubdivisionCode, country_from_code, reference::ReferenceRelease,
};
use geo_service::geocoding::{
    CandidateIssue, CandidateSet, GeocodingCandidate, GeocodingError, GeocodingFailure,
    GeocodingOutcome, GeocodingProvenance, GeocodingRequest, GeocodingResult, ProviderMatchScore,
    ResultRetention,
};

pub(crate) fn map_response(
    response: GeocodeOutput,
    request: &GeocodingRequest,
    retention: ResultRetention,
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
        .iter()
        .map(|item| {
            let candidate = map_candidate(item, request)?;
            Ok(match retention {
                ResultRetention::SingleUse => candidate,
                ResultRetention::Storage => retain_or_withhold(candidate, item),
            })
        })
        .collect::<Result<Vec<_>, GeocodingError>>()?;
    if !candidates.is_empty()
        && candidates
            .iter()
            .all(GeocodingCandidate::is_withheld_from_storage)
    {
        return Err(GeocodingError {
            kind: GeocodingFailure::StorageNotPermitted,
            source: static_error("no returned candidate permits retained geocoding"),
        });
    }
    let outcome = if candidates.is_empty() {
        GeocodingOutcome::NoMatch
    } else {
        GeocodingOutcome::Candidates(CandidateSet::new(candidates, request.candidate_limit())?)
    };
    Ok(GeocodingResult { outcome, retention })
}

fn map_candidate(
    item: &GeocodeResultItem,
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
    let country = country_data.and_then(returned_country);
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
    let postal = item
        .address
        .as_ref()
        .and_then(|a| a.postal_code.as_ref())
        .map(|value| PostalCode::new(value.clone()))
        .transpose()
        .map_err(|source| GeocodingError {
            kind: GeocodingFailure::InvalidResponse,
            source: box_error(source),
        })?;
    let description =
        GeographicDescription::new(None, country, subdivision, postal).map_err(|source| {
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
            external_reference: item.place_id.clone(),
            attribution_url: "https://docs.aws.amazon.com/location/latest/developerguide/data-attribution.html",
        },
    ))
}

/// Japanese data cannot be retained, and an unverifiable country cannot prove a candidate
/// is not Japanese, including when the provider ignored a country filter. Withholding only
/// such candidates keeps their verified alternatives retainable.
fn retain_or_withhold(
    candidate: GeocodingCandidate,
    item: &GeocodeResultItem,
) -> GeocodingCandidate {
    let Some(reason) = withholding_reason(&candidate) else {
        return candidate;
    };
    let fields = item.address.as_ref().and_then(|a| a.country.as_ref());
    tracing::info!(
        metric = "geocoding_candidate_withheld",
        reason,
        code2 = country_code_diagnostic(fields.and_then(Country::code2), 2, country_from_alpha2),
        code3 = country_code_diagnostic(fields.and_then(Country::code3), 3, country_from_alpha3),
    );
    candidate.withhold_from_storage()
}

fn withholding_reason(candidate: &GeocodingCandidate) -> Option<&'static str> {
    let country = candidate
        .geography()
        .and_then(DerivedGeography::description)
        .and_then(GeographicDescription::country);
    match country {
        Some(CountryCode::JPN) => Some("JAPAN"),
        Some(_) => None,
        None if candidate
            .issues()
            .contains(&CandidateIssue::InconsistentComponents) =>
        {
            Some("CONTRADICTORY_COUNTRY")
        }
        None => Some("UNVERIFIABLE_COUNTRY"),
    }
}

/// Reports a provider country field without echoing a recognized code: only a well-formed
/// code that the pinned reference release lacks is named verbatim, never arbitrary text.
fn country_code_diagnostic(
    code: Option<&str>,
    length: usize,
    recognize: fn(&str) -> Option<CountryCode>,
) -> &str {
    match code {
        None => "ABSENT",
        Some(code) if !is_code_shaped(code, length) => "MALFORMED",
        Some(code) if recognize(code).is_some() => "RECOGNIZED",
        Some(code) => code,
    }
}

/// Code2 and Code3 are documented ISO country fields. Only use Code3 on actual Code2
/// absence, never to conceal an invalid present Code2.
fn returned_country(country: &Country) -> Option<CountryCode> {
    match country.code2.as_deref() {
        Some(code) => country_from_alpha2(code),
        None => country.code3.as_deref().and_then(country_from_alpha3),
    }
}

fn country_from_alpha2(code: &str) -> Option<CountryCode> {
    country_from_code(code, ReferenceRelease::CURRENT).ok()
}

/// Strict ISO alpha-3 lookup limited to countries of the pinned reference release.
fn country_from_alpha3(code: &str) -> Option<CountryCode> {
    if !is_code_shaped(code, 3) {
        return None;
    }
    CountryCode::for_alpha3(code)
        .ok()
        .and_then(|country| country_from_alpha2(country.alpha2()))
}

fn is_code_shaped(code: &str, length: usize) -> bool {
    code.len() == length && code.bytes().all(|b| b.is_ascii_uppercase())
}

fn invalid_response(message: &'static str) -> GeocodingError {
    GeocodingError {
        kind: GeocodingFailure::InvalidResponse,
        source: static_error(message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_geoplaces::types::{Address, PlaceType};
    use geo::AddressText;

    fn request() -> GeocodingRequest {
        GeocodingRequest::new(AddressText::new("Synthetic dealer address").unwrap())
    }

    fn item(place_id: &str, code2: Option<&str>, code3: Option<&str>) -> GeocodeResultItem {
        let country = Country::builder()
            .set_code2(code2.map(Into::into))
            .set_code3(code3.map(Into::into))
            .build();
        GeocodeResultItem::builder()
            .place_id(place_id)
            .place_type(PlaceType::Locality)
            .title("synthetic")
            .set_position(Some(vec![13.4, 52.5]))
            .address(Address::builder().country(country).build())
            .build()
            .unwrap()
    }

    fn map(
        items: Vec<GeocodeResultItem>,
        retention: ResultRetention,
    ) -> Result<Vec<GeocodingCandidate>, GeocodingError> {
        let response = GeocodeOutput::builder()
            .pricing_bucket("Stored")
            .set_result_items(Some(items))
            .build()
            .unwrap();
        Ok(
            match map_response(response, &request(), retention)?.outcome {
                GeocodingOutcome::Candidates(set) => set.as_slice().to_vec(),
                GeocodingOutcome::NoMatch => vec![],
            },
        )
    }

    /// Kosovo's user-assigned `XK` is outside ISO 3166-1 and therefore the reference release.
    fn mixed() -> Vec<GeocodeResultItem> {
        vec![
            item("synthetic-de", Some("DE"), Some("DEU")),
            item("synthetic-xk", Some("XK"), Some("XKX")),
            item("synthetic-jp", Some("JP"), Some("JPN")),
            item("synthetic-contradiction", Some("CA"), Some("USA")),
        ]
    }

    #[test]
    fn should_withhold_only_unretainable_candidates_when_a_retained_result_is_mixed() {
        let withheld = |candidates: Vec<GeocodingCandidate>| {
            candidates
                .iter()
                .map(GeocodingCandidate::is_withheld_from_storage)
                .collect::<Vec<_>>()
        };
        let retained = map(mixed(), ResultRetention::Storage).unwrap();
        assert_eq!(vec![false, true, true, true], withheld(retained.clone()));
        assert_eq!(
            Some(CountryCode::DEU),
            retained[0]
                .geography()
                .and_then(DerivedGeography::description)
                .and_then(GeographicDescription::country)
        );
        let single_use = map(mixed(), ResultRetention::SingleUse).unwrap();
        assert_eq!(vec![false; 4], withheld(single_use));
    }

    #[test]
    fn should_refuse_storage_when_no_returned_candidate_is_retainable() {
        let unretainable = mixed().split_off(1);
        assert_eq!(
            GeocodingFailure::StorageNotPermitted,
            map(unretainable.clone(), ResultRetention::Storage)
                .unwrap_err()
                .kind
        );
        assert_eq!(
            3,
            map(unretainable, ResultRetention::SingleUse).unwrap().len()
        );
        assert!(map(vec![], ResultRetention::Storage).unwrap().is_empty());
    }

    #[test]
    fn should_classify_why_a_candidate_is_withheld() {
        for (code2, code3, expected) in [
            (Some("DE"), Some("DEU"), None),
            (Some("JP"), Some("JPN"), Some("JAPAN")),
            (Some("XK"), Some("XKX"), Some("UNVERIFIABLE_COUNTRY")),
            (Some("ca"), Some("CAN"), Some("UNVERIFIABLE_COUNTRY")),
            (None, None, Some("UNVERIFIABLE_COUNTRY")),
            (Some("CA"), Some("USA"), Some("CONTRADICTORY_COUNTRY")),
        ] {
            let candidate = map_candidate(&item("synthetic", code2, code3), &request()).unwrap();
            assert_eq!(
                expected,
                withholding_reason(&candidate),
                "{code2:?}/{code3:?}"
            );
        }
    }

    #[test]
    fn should_name_only_unrecognized_codes_when_diagnosing_a_withheld_country() {
        for (code, expected) in [
            (None, "ABSENT"),
            (Some("ca"), "MALFORMED"),
            (Some("CAN"), "MALFORMED"),
            (Some("Ä"), "MALFORMED"),
            (Some("CA"), "RECOGNIZED"),
            (Some("JP"), "RECOGNIZED"),
            (Some("XK"), "XK"),
        ] {
            assert_eq!(
                expected,
                country_code_diagnostic(code, 2, country_from_alpha2)
            );
        }
        for (code, expected) in [
            (None, "ABSENT"),
            (Some("CA"), "MALFORMED"),
            (Some("can"), "MALFORMED"),
            (Some("CAN"), "RECOGNIZED"),
            (Some("XKX"), "XKX"),
        ] {
            assert_eq!(
                expected,
                country_code_diagnostic(code, 3, country_from_alpha3)
            );
        }
    }
}
