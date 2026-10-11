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

#[derive(Serialize, Deserialize)]
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
    retention_context: String,
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
            retention_context: v.retention_context().into(),
        }
    }
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
        ResolutionInput::new(
            r,
            scope,
            RetentionPurpose::from_code(&v.purpose).ok_or_else(corrupt)?,
        )
        .map_err(invalid)?
        .with_retention_context(&v.retention_context)
        .map_err(invalid)
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
                            || c.geography.position().is_some()
                            || c.geography.description().is_none_or(|d| {
                                d.address_text().is_some()
                                    || d.postal_code().is_some()
                                    || (d.country().is_none() && d.subdivision().is_none())
                            })
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
