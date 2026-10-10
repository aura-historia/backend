//! Provider-neutral geocoding evidence. No candidate is an automatic acceptance decision.
use application::error::BoxError;
use geo::{
    AddressText, CountryCode, DerivedGeography, GeoPoint, SubdivisionCode, SubmittedGeography,
};
use localization::Language;
use std::{fmt, sync::Arc};

/// Provider permission for retaining derived results. The consuming service owns
/// authorization, user isolation, cache identity and the decision to persist evidence.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ResultRetention {
    #[default]
    SingleUse,
    Storage,
}

impl ResultRetention {
    pub fn permits_storage(self) -> bool {
        self == Self::Storage
    }
}

/// Hard requirements are checked against returned evidence as well as sent to providers.
/// A subdivision retains its country association without inventing a country assertion.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct GeographicConstraint {
    country: Option<CountryCode>,
    subdivision: Option<SubdivisionCode>,
}

impl GeographicConstraint {
    pub fn new(
        country: Option<CountryCode>,
        subdivision: Option<SubdivisionCode>,
    ) -> Result<Self, InvalidGeocodingRequest> {
        if country.is_none() && subdivision.is_none() {
            return Err(InvalidGeocodingRequest::EmptyConstraint);
        }
        if let (Some(country), Some(subdivision)) = (country, subdivision)
            && country != subdivision.country()
        {
            return Err(InvalidGeocodingRequest::IncompatibleConstraint);
        }
        Ok(Self {
            country,
            subdivision,
        })
    }
    pub fn country(self) -> Option<CountryCode> {
        self.country
    }
    pub fn subdivision(self) -> Option<SubdivisionCode> {
        self.subdivision
    }
    pub fn associated_country(self) -> CountryCode {
        self.country
            .or_else(|| self.subdivision.map(SubdivisionCode::country))
            .expect("constructor requires country or subdivision")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidGeocodingRequest {
    #[error("candidate limit must be between 1 and 10")]
    CandidateLimit,
    #[error("geographic constraint must have a country or subdivision")]
    EmptyConstraint,
    #[error("constraint country and subdivision contradict each other")]
    IncompatibleConstraint,
}

#[derive(Clone)]
pub struct GeocodingRequest {
    query: AddressText,
    candidate_limit: u8,
    submitted: Option<SubmittedGeography>,
    constraint: Option<GeographicConstraint>,
    bias_position: Option<GeoPoint>,
    language: Option<Language>,
}

impl GeocodingRequest {
    pub const MAX_CANDIDATES: usize = 10;
    pub const DEFAULT_CANDIDATES: u8 = 5;

    pub fn new(query: AddressText) -> Self {
        Self {
            query,
            candidate_limit: Self::DEFAULT_CANDIDATES,
            submitted: None,
            constraint: None,
            bias_position: None,
            language: None,
        }
    }
    pub fn with_candidate_limit(mut self, limit: u8) -> Result<Self, InvalidGeocodingRequest> {
        if !(1..=Self::MAX_CANDIDATES as u8).contains(&limit) {
            return Err(InvalidGeocodingRequest::CandidateLimit);
        }
        self.candidate_limit = limit;
        Ok(self)
    }
    pub fn with_submitted(mut self, submitted: SubmittedGeography) -> Self {
        self.submitted = Some(submitted);
        self
    }
    pub fn with_constraint(mut self, constraint: GeographicConstraint) -> Self {
        self.constraint = Some(constraint);
        self
    }
    /// Ranking hint only. Never establishes a canonical point or a verified constraint.
    pub fn with_bias_position(mut self, point: GeoPoint) -> Self {
        self.bias_position = Some(point);
        self
    }
    /// Presentation language only; it cannot change asserted country/subdivision.
    pub fn with_language(mut self, language: Language) -> Self {
        self.language = Some(language);
        self
    }
    pub fn query(&self) -> &AddressText {
        &self.query
    }
    pub fn candidate_limit(&self) -> usize {
        self.candidate_limit.into()
    }
    pub fn submitted(&self) -> Option<&SubmittedGeography> {
        self.submitted.as_ref()
    }
    pub fn constraint(&self) -> Option<GeographicConstraint> {
        self.constraint
    }
    pub fn bias_position(&self) -> Option<GeoPoint> {
        self.bias_position
    }
    pub fn language(&self) -> Option<Language> {
        self.language
    }
}

impl fmt::Debug for GeocodingRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeocodingRequest")
            .field("candidate_limit", &self.candidate_limit)
            .finish_non_exhaustive()
    }
}

/// Safe diagnostic vocabulary, never persisted as geography or filled with provider strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateIssue {
    UnsupportedCountry,
    UnsupportedSubdivision,
    UnsupportedPrecision,
    MissingPosition,
    ApproximatePosition,
    InconsistentComponents,
    CountryConstraintMismatch,
    SubdivisionConstraintMismatch,
    UnverifiedConstraint,
    SubmittedCountryMismatch,
    SubmittedSubdivisionMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateUsability {
    Usable,
    Unsupported,
    Inconsistent,
    ConstraintMismatch,
    AssertionMismatch,
    UnverifiedConstraint,
}

/// Provider ranking evidence, NOT a probability or a calibrated acceptance threshold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProviderMatchScore(f64);

impl ProviderMatchScore {
    pub fn new(value: f64) -> Option<Self> {
        (value.is_finite() && (0.0..=1.0).contains(&value)).then_some(Self(value))
    }
    pub fn value(self) -> f64 {
        self.0
    }
}

/// Provider identity and external reference. The reference is never an Aura entity ID.
#[derive(Clone)]
pub struct GeocodingProvenance {
    pub provider: &'static str,
    pub product: &'static str,
    pub data_source: &'static str,
    pub region: String,
    /// Immutable adapter interpretation/terms profile, required for retained cache identity.
    pub interpretation_profile: &'static str,
    pub external_reference: String,
    pub attribution_url: &'static str,
}

#[derive(Clone)]
pub struct GeocodingCandidate {
    geography: Option<DerivedGeography>,
    issues: Vec<CandidateIssue>,
    score: Option<ProviderMatchScore>,
    provenance: GeocodingProvenance,
}

impl GeocodingCandidate {
    pub fn new(
        geography: Option<DerivedGeography>,
        issues: Vec<CandidateIssue>,
        score: Option<ProviderMatchScore>,
        provenance: GeocodingProvenance,
    ) -> Self {
        Self {
            geography,
            issues,
            score,
            provenance,
        }
    }
    pub fn geography(&self) -> Option<&DerivedGeography> {
        self.geography.as_ref()
    }
    pub fn issues(&self) -> &[CandidateIssue] {
        &self.issues
    }
    pub fn score(&self) -> Option<ProviderMatchScore> {
        self.score
    }
    pub fn provenance(&self) -> &GeocodingProvenance {
        &self.provenance
    }
    pub fn usability(&self) -> CandidateUsability {
        if self
            .issues
            .contains(&CandidateIssue::InconsistentComponents)
        {
            CandidateUsability::Inconsistent
        } else if self.issues.iter().any(|i| {
            matches!(
                i,
                CandidateIssue::CountryConstraintMismatch
                    | CandidateIssue::SubdivisionConstraintMismatch
            )
        }) {
            CandidateUsability::ConstraintMismatch
        } else if self.issues.contains(&CandidateIssue::UnverifiedConstraint) {
            CandidateUsability::UnverifiedConstraint
        } else if self.issues.iter().any(|i| {
            matches!(
                i,
                CandidateIssue::SubmittedCountryMismatch
                    | CandidateIssue::SubmittedSubdivisionMismatch
            )
        }) {
            CandidateUsability::AssertionMismatch
        } else if self.geography.is_none() {
            CandidateUsability::Unsupported
        } else {
            CandidateUsability::Usable
        }
    }
}

impl fmt::Debug for GeocodingCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeocodingCandidate")
            .field("usability", &self.usability())
            .field("issues", &self.issues)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone)]
pub enum GeocodingOutcome {
    NoMatch,
    Candidates(CandidateSet),
}

/// A nonempty bounded set of evidence. One candidate does not prove global uniqueness.
#[derive(Debug, Clone)]
pub struct CandidateSet(Vec<GeocodingCandidate>);

impl CandidateSet {
    pub fn new(
        candidates: Vec<GeocodingCandidate>,
        requested_limit: usize,
    ) -> Result<Self, GeocodingError> {
        if candidates.is_empty()
            || candidates.len() > requested_limit
            || requested_limit > GeocodingRequest::MAX_CANDIDATES
        {
            return Err(GeocodingError {
                kind: GeocodingFailure::InvalidResponse,
                source: application::error::static_error("invalid geocoding candidate count"),
            });
        }
        Ok(Self(candidates))
    }
    pub fn as_slice(&self) -> &[GeocodingCandidate] {
        &self.0
    }
    pub fn has_multiple_candidates(&self) -> bool {
        self.0.len() > 1
    }
}

#[derive(Debug, Clone)]
pub struct GeocodingResult {
    pub outcome: GeocodingOutcome,
    /// Applies to all provider-derived components, including country-only evidence.
    pub retention: ResultRetention,
}

/// Static, safe classifications. Causes are for controlled diagnosis, never ordinary logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeocodingFailure {
    Timeout,
    Throttled,
    Unavailable,
    Authentication,
    Configuration,
    UnsupportedInput,
    InvalidResponse,
    StorageNotPermitted,
}

#[derive(thiserror::Error)]
#[error("geocoding failed: {kind:?}")]
pub struct GeocodingError {
    pub kind: GeocodingFailure,
    #[source]
    pub source: BoxError,
}

impl fmt::Debug for GeocodingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeocodingError")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
pub trait GeocodingProvider: Send + Sync {
    async fn geocode(&self, request: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError>;
}

#[async_trait::async_trait]
pub trait Geocode: Send + Sync {
    async fn geocode(&self, request: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError>;
}

pub struct GeocodeHandler<P> {
    provider: P,
}

impl<P: GeocodingProvider> GeocodeHandler<P> {
    pub fn new(provider: P) -> Self {
        Self { provider }
    }
}

#[async_trait::async_trait]
impl<P: GeocodingProvider> Geocode for GeocodeHandler<P> {
    async fn geocode(&self, request: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError> {
        let result = self.provider.geocode(request).await?;
        if let GeocodingOutcome::Candidates(candidates) = &result.outcome
            && candidates.as_slice().len() > request.candidate_limit()
        {
            return Err(GeocodingError {
                kind: GeocodingFailure::InvalidResponse,
                source: application::error::static_error(
                    "provider exceeded requested candidate count",
                ),
            });
        }
        Ok(result)
    }
}

#[async_trait::async_trait]
impl<P: GeocodingProvider + ?Sized> GeocodingProvider for Arc<P> {
    async fn geocode(&self, request: &GeocodingRequest) -> Result<GeocodingResult, GeocodingError> {
        self.as_ref().geocode(request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubProvider(GeocodingResult);

    #[async_trait::async_trait]
    impl GeocodingProvider for StubProvider {
        async fn geocode(
            &self,
            _request: &GeocodingRequest,
        ) -> Result<GeocodingResult, GeocodingError> {
            Ok(self.0.clone())
        }
    }

    fn candidate() -> GeocodingCandidate {
        GeocodingCandidate::new(
            None,
            vec![CandidateIssue::UnsupportedPrecision],
            None,
            GeocodingProvenance {
                provider: "TEST",
                product: "TEST",
                data_source: "TEST",
                region: "TEST".into(),
                interpretation_profile: "TEST_V1",
                external_reference: "TEST".into(),
                attribution_url: "TEST",
            },
        )
    }

    #[tokio::test]
    async fn address_only_requests_preserve_candidates_retention_and_provider_bounds() {
        let request = GeocodingRequest::new(AddressText::new("test").unwrap());
        assert_eq!(5, request.candidate_limit());
        for retention in [ResultRetention::SingleUse, ResultRetention::Storage] {
            let result = GeocodingResult {
                outcome: GeocodingOutcome::Candidates(
                    CandidateSet::new(vec![candidate(), candidate()], 2).unwrap(),
                ),
                retention,
            };
            let handler = GeocodeHandler::new(Arc::new(StubProvider(result)));
            let result = handler.geocode(&request).await.unwrap();
            assert!(
                matches!(result.outcome, GeocodingOutcome::Candidates(c) if c.has_multiple_candidates())
            );
            assert_eq!(retention, result.retention);
            let smaller_request = request.clone().with_candidate_limit(1).unwrap();
            assert_eq!(
                GeocodingFailure::InvalidResponse,
                handler.geocode(&smaller_request).await.unwrap_err().kind
            );
        }
    }

    #[test]
    fn candidate_sets_are_nonempty_and_bounded() {
        assert!(CandidateSet::new(vec![], 5).is_err());
        assert!(CandidateSet::new(vec![candidate()], 0).is_err());
        assert!(CandidateSet::new(vec![candidate()], 11).is_err());
        assert!(CandidateSet::new(vec![candidate(); 11], 10).is_err());
        assert!(CandidateSet::new(vec![candidate(); 10], 10).is_ok());
    }

    #[test]
    fn bounds_and_constraint_consistency() {
        for n in [0, 11, 255] {
            assert!(
                GeocodingRequest::new(AddressText::new("Berlin").unwrap())
                    .with_candidate_limit(n)
                    .is_err()
            );
        }
        assert!(GeographicConstraint::new(None, None).is_err());
        assert!(
            GeographicConstraint::new(
                Some(CountryCode::USA),
                Some(SubdivisionCode::new("CA-BC").unwrap())
            )
            .is_err()
        );
        let subdivision_only =
            GeographicConstraint::new(None, Some(SubdivisionCode::new("CA-BC").unwrap())).unwrap();
        assert_eq!(None, subdivision_only.country());
        assert_eq!(CountryCode::CAN, subdivision_only.associated_country());
    }

    #[test]
    fn score_is_validated_and_debug_is_safe() {
        for n in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(ProviderMatchScore::new(n).is_none());
        }
        let request = GeocodingRequest::new(AddressText::new("sensitive address").unwrap())
            .with_bias_position(GeoPoint::new(12.34, 56.78).unwrap());
        let debug = format!("{request:?}");
        assert!(!debug.contains("sensitive"));
        assert!(!debug.contains("12.34"));
        let error = GeocodingError {
            kind: GeocodingFailure::Unavailable,
            source: application::error::static_error("sensitive provider content"),
        };
        assert!(!format!("{error:?} {error}").contains("sensitive"));
        assert!(std::error::Error::source(&error).is_some());
    }
}
