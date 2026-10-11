use application::error::box_error;
use geo::{
    AddressText, DerivedGeography, GeoPoint, GeographicDescription, SpatialPosition,
    SubdivisionCode, SubmittedGeography, country_from_code,
    data::geography_data::{GeoPointData, GeographicDescriptionData, SpatialPositionData},
    reference::ReferenceRelease,
};
use geo_service::{
    geocoding::{GeocodingRequest, GeographicConstraint},
    resolution::*,
};
use localization::Language;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub(crate) fn invalid(e: impl std::error::Error + Send + Sync + 'static) -> ResolutionError {
    ResolutionError::InvalidState(box_error(e))
}
pub(crate) fn corrupt() -> ResolutionError {
    invalid(std::io::Error::other(
        "invalid persisted resolution encoding",
    ))
}
pub(crate) fn encode<T: Serialize>(v: &T) -> Result<serde_json::Value, ResolutionError> {
    serde_json::to_value(v).map_err(invalid)
}
pub(crate) fn decode<T: serde::de::DeserializeOwned>(
    v: serde_json::Value,
) -> Result<T, ResolutionError> {
    serde_json::from_value(v).map_err(invalid)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InputData {
    query: String,
    limit: u8,
    submitted: Option<GeographicDescriptionData>,
    constraint_country: Option<String>,
    constraint_subdivision: Option<String>,
    reference_release: String,
    bias: Option<GeoPointData>,
    language: Option<String>,
    scope: String,
    owner: Option<String>,
    purpose: String,
    // Read-only compatibility with inputs persisted before disclosure was removed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retention_context: Option<String>,
}
impl From<&ResolutionInput> for InputData {
    fn from(v: &ResolutionInput) -> Self {
        let r = v.request();
        Self {
            query: r.query().as_str().into(),
            limit: r.candidate_limit() as u8,
            submitted: r.submitted().map(|s| s.description().into()),
            constraint_country: r
                .constraint()
                .and_then(|c| c.country())
                .map(|c| c.alpha2().into()),
            constraint_subdivision: r
                .constraint()
                .and_then(|c| c.subdivision())
                .map(|c| c.as_str().into()),
            reference_release: ReferenceRelease::CURRENT.as_str().into(),
            bias: r.bias_position().map(Into::into),
            language: r.language().map(|l| l.as_str().into()),
            scope: v.scope().kind().as_str().into(),
            owner: v.scope().owner().map(|id| id.to_string()),
            purpose: v.purpose().as_str().into(),
            retention_context: None,
        }
    }
}
impl InputData {
    pub(crate) fn stored_fingerprint(&self, input: &ResolutionInput, profile: &str) -> String {
        match self.retention_context.as_deref() {
            Some(context) => legacy_input_fingerprint(input, profile, context),
            None => input.fingerprint(profile),
        }
    }
}
pub(crate) fn compatible_input_fingerprints(input: &ResolutionInput, profile: &str) -> Vec<String> {
    let contexts: &[&str] = match (input.purpose(), input.scope().kind()) {
        (RetentionPurpose::UserLocation, _) => &["PRIVATE"],
        (_, ScopeKind::SharedBusiness) => &["EXACT_PUBLIC"],
        _ => &["PRIVATE", "COARSE_PUBLIC", "EXACT_PUBLIC"],
    };
    std::iter::once(input.fingerprint(profile))
        .chain(
            contexts
                .iter()
                .map(|context| legacy_input_fingerprint(input, profile, context)),
        )
        .collect()
}
// Persisted V1 keys remain valid without exposing their retired setting to callers.
fn legacy_input_fingerprint(input: &ResolutionInput, profile: &str, context: &str) -> String {
    let mut hash = Sha256::new();
    let mut field = |s: &str| {
        hash.update((s.len() as u64).to_be_bytes());
        hash.update(s.as_bytes());
    };
    field("RESOLUTION_INPUT_V1");
    field(profile);
    field(input.policy_version());
    field(geo::reference::ReferenceRelease::CURRENT.as_str());
    field(input.scope().kind().as_str());
    field(
        &input
            .scope()
            .owner()
            .map(|id| id.to_string())
            .unwrap_or_default(),
    );
    field(input.purpose().as_str());
    field(context);
    field(input.request().query().as_str());
    field(&input.request().candidate_limit().to_string());
    if let Some(s) = input.request().submitted() {
        let d = s.description();
        field("SUBMITTED");
        field(d.reference_release().as_str());
        field(d.address_text().map(|v| v.as_str()).unwrap_or_default());
        field(d.country().map(|v| v.alpha2()).unwrap_or_default());
        field(d.subdivision().map(|v| v.as_str()).unwrap_or_default());
        field(d.postal_code().map(|v| v.as_str()).unwrap_or_default());
    } else {
        field("NO_SUBMITTED");
    }
    if let Some(c) = input.request().constraint() {
        field("CONSTRAINT");
        field(c.country().map(|v| v.alpha2()).unwrap_or_default());
        field(c.subdivision().map(|v| v.as_str()).unwrap_or_default());
    } else {
        field("NO_CONSTRAINT");
    }
    if let Some(p) = input.request().bias_position() {
        field("BIAS");
        field(&p.latitude_degrees().to_bits().to_string());
        field(&p.longitude_degrees().to_bits().to_string());
    } else {
        field("NO_BIAS");
    }
    field(
        input
            .request()
            .language()
            .map(|v| v.as_str())
            .unwrap_or_default(),
    );
    hash.finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
impl TryFrom<InputData> for ResolutionInput {
    type Error = ResolutionError;
    fn try_from(v: InputData) -> Result<Self, Self::Error> {
        let release = ReferenceRelease::from_code(&v.reference_release).map_err(invalid)?;
        let mut r = GeocodingRequest::new(AddressText::new(v.query).map_err(invalid)?)
            .with_candidate_limit(v.limit)
            .map_err(invalid)?;
        if let Some(d) = v.submitted {
            r = r.with_submitted(SubmittedGeography::new(
                Option::<GeographicDescription>::try_from(d)
                    .map_err(invalid)?
                    .ok_or_else(corrupt)?,
            ));
        }
        let country = v
            .constraint_country
            .as_deref()
            .map(|c| country_from_code(c, release))
            .transpose()
            .map_err(invalid)?;
        let subdivision = v
            .constraint_subdivision
            .as_deref()
            .map(|s| SubdivisionCode::from_code(s, release))
            .transpose()
            .map_err(invalid)?;
        if country.is_some() || subdivision.is_some() {
            r = r
                .with_constraint(GeographicConstraint::new(country, subdivision).map_err(invalid)?);
        }
        if let Some(p) = v.bias {
            r = r.with_bias_position(GeoPoint::try_from(p).map_err(invalid)?);
        }
        if let Some(l) = v.language {
            r = r.with_language(Language::from_code(&l).ok_or_else(corrupt)?);
        }
        let owner = v
            .owner
            .map(|raw| {
                let id = Uuid::parse_str(&raw).map_err(invalid)?;
                if id.to_string() != raw {
                    return Err(corrupt());
                }
                Ok(id)
            })
            .transpose()?;
        let scope =
            ResolutionScope::new(ScopeKind::from_code(&v.scope).ok_or_else(corrupt)?, owner)
                .map_err(invalid)?;
        let purpose = RetentionPurpose::from_code(&v.purpose).ok_or_else(corrupt)?;
        if v.retention_context.as_deref().is_some_and(|context| {
            !matches!(context, "PRIVATE" | "COARSE_PUBLIC" | "EXACT_PUBLIC")
                || (purpose == RetentionPurpose::UserLocation && context != "PRIVATE")
                || (scope.kind() == ScopeKind::SharedBusiness && context != "EXACT_PUBLIC")
        }) {
            return Err(corrupt());
        }
        ResolutionInput::new(r, scope, purpose).map_err(invalid)
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CandidateData {
    description: Option<GeographicDescriptionData>,
    position: Option<SpatialPositionData>,
    acceptable: bool,
    provider: String,
    product: String,
    data_source: String,
    region: String,
    profile: String,
    external_reference: String,
    attribution_url: String,
}
impl From<&ResolutionCandidate> for CandidateData {
    fn from(c: &ResolutionCandidate) -> Self {
        Self {
            description: c.geography.description().map(Into::into),
            position: c.geography.position().map(Into::into),
            acceptable: c.acceptable,
            provider: c.provider.clone(),
            product: c.product.clone(),
            data_source: c.data_source.clone(),
            region: c.region.clone(),
            profile: c.profile.clone(),
            external_reference: c.external_reference.clone(),
            attribution_url: c.attribution_url.clone(),
        }
    }
}
impl TryFrom<CandidateData> for ResolutionCandidate {
    type Error = ResolutionError;
    fn try_from(c: CandidateData) -> Result<Self, Self::Error> {
        for s in [
            &c.provider,
            &c.product,
            &c.data_source,
            &c.region,
            &c.profile,
            &c.external_reference,
            &c.attribution_url,
        ] {
            if s.is_empty() || s.len() > 2048 || s.chars().any(char::is_control) {
                return Err(corrupt());
            }
        }
        let d = c
            .description
            .map(Option::<GeographicDescription>::try_from)
            .transpose()
            .map_err(invalid)?
            .flatten();
        let p = c
            .position
            .map(SpatialPosition::try_from)
            .transpose()
            .map_err(invalid)?;
        if c.acceptable && p.is_some_and(|p| p.precision() != geo::SpatialPrecision::Premises) {
            return Err(corrupt());
        }
        Ok(Self {
            geography: DerivedGeography::new(d, p).ok_or_else(corrupt)?,
            acceptable: c.acceptable,
            provider: c.provider,
            product: c.product,
            data_source: c.data_source,
            region: c.region,
            profile: c.profile,
            external_reference: c.external_reference,
            attribution_url: c.attribution_url,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceData {
    status: String,
    candidates: Vec<CandidateData>,
    accepted_index: Option<usize>,
}
impl From<&ResolutionEvidence> for EvidenceData {
    fn from(e: &ResolutionEvidence) -> Self {
        Self {
            status: e.status.as_str().into(),
            candidates: e.candidates.iter().map(Into::into).collect(),
            accepted_index: e.accepted_index,
        }
    }
}
impl TryFrom<EvidenceData> for ResolutionEvidence {
    type Error = ResolutionError;
    fn try_from(e: EvidenceData) -> Result<Self, Self::Error> {
        let status = ResolutionStatus::from_code(&e.status).ok_or_else(corrupt)?;
        let candidates = e
            .candidates
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<ResolutionCandidate>, _>>()?;
        if candidates.len() > 11
            || match status {
                ResolutionStatus::Accepted => e.accepted_index.is_none_or(|i| {
                    candidates.get(i).is_none_or(|c| {
                        !c.acceptable
                            || c.geography
                                .position()
                                .is_some_and(|p| p.precision() != geo::SpatialPrecision::Premises)
                            || (c.geography.position().is_none()
                                && c.geography.description().is_none_or(|d| {
                                    d.address_text().is_some()
                                        || d.postal_code().is_some()
                                        || (d.country().is_none() && d.subdivision().is_none())
                                }))
                    })
                }),
                ResolutionStatus::Ambiguous => e.accepted_index.is_some(),
                ResolutionStatus::NoMatch => e.accepted_index.is_some() || !candidates.is_empty(),
                _ => true,
            }
        {
            return Err(corrupt());
        }
        Ok(Self {
            status,
            candidates,
            accepted_index: e.accepted_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ResolutionInput {
        ResolutionInput::new(
            GeocodingRequest::new(AddressText::new("123 Street, unit 1").unwrap()),
            ResolutionScope::new(
                ScopeKind::Party,
                Some(Uuid::parse_str("019cb180-0000-7000-8000-000000000001").unwrap()),
            )
            .unwrap(),
            RetentionPurpose::PartyLocation,
        )
        .unwrap()
    }

    #[test]
    fn current_inputs_omit_removed_context_and_round_trip() {
        let input = input();
        let stored = encode(&InputData::from(&input)).unwrap();
        assert!(stored.get("retention_context").is_none());
        let decoded: ResolutionInput = decode::<InputData>(stored).unwrap().try_into().unwrap();
        assert_eq!(input.fingerprint("TEST_V1"), decoded.fingerprint("TEST_V1"));
    }

    #[test]
    fn legacy_keys_are_validated_and_reusable_without_restoring_the_setting() {
        let input = input();
        let legacy = "3d5e3b124d0287ffaee50ce7ca9357a34e3e8c9feb6fe93308189634e5751500";
        let mut stored = encode(&InputData::from(&input)).unwrap();
        stored["retention_context"] = serde_json::json!("PRIVATE");
        let data = decode::<InputData>(stored.clone()).unwrap();
        let decoded: ResolutionInput = data.clone().try_into().unwrap();
        assert_eq!(legacy, data.stored_fingerprint(&decoded, "TEST_V1"));
        assert!(compatible_input_fingerprints(&input, "TEST_V1").contains(&legacy.to_owned()));
        assert!(
            encode(&InputData::from(&decoded))
                .unwrap()
                .get("retention_context")
                .is_none()
        );
        stored["retention_context"] = serde_json::json!("INVALID");
        assert!(ResolutionInput::try_from(decode::<InputData>(stored).unwrap()).is_err());
    }
}
