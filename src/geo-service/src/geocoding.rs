//! Provider-neutral geocoding evidence. No candidate is an automatic acceptance decision.
use application::error::BoxError;
use geo::{
    AddressText, CountryCode, DerivedGeography, GeoPoint, SubdivisionCode, SubmittedGeography,
};
use localization::Language;
use std::{fmt, sync::Arc};
use user_core::user_id::UserId;

/// Trusted use-case purpose, never inferred from the text or supplied by an anonymous caller.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GeocodingPurpose {
    DealerPreview,
    DealerReusable,
    PrivatePreview { user_id: UserId },
    PrivateRetained { user_id: UserId },
}

impl GeocodingPurpose {
    pub fn requires_storage(self) -> bool {
        matches!(self, Self::DealerReusable | Self::PrivateRetained { .. })
    }

    pub fn storage_scope(self) -> Option<GeocodingStorageScope> {
        match self {
            Self::DealerReusable => Some(GeocodingStorageScope::DealerShared),
            Self::PrivateRetained { user_id } => {
                Some(GeocodingStorageScope::PrivateUser { user_id })
            }
            _ => None,
        }
    }
}

impl fmt::Debug for GeocodingPurpose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DealerPreview => "DealerPreview",
            Self::DealerReusable => "DealerReusable",
            Self::PrivatePreview { .. } => "PrivatePreview { user_id: [redacted] }",
            Self::PrivateRetained { .. } => "PrivateRetained { user_id: [redacted] }",
        })
    }
}

/// Cache/persistence owners must check the exact scope before writing or reusing evidence.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum GeocodingStorageScope {
    DealerShared,
    PrivateUser { user_id: UserId },
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
    purpose: GeocodingPurpose,
    candidate_limit: u8,
    submitted: Option<SubmittedGeography>,
    constraint: Option<GeographicConstraint>,
    bias_position: Option<GeoPoint>,
    language: Option<Language>,
}

impl GeocodingRequest {
    pub const MAX_CANDIDATES: usize = 10;

    pub fn new(
        query: AddressText,
        purpose: GeocodingPurpose,
        candidate_limit: u8,
    ) -> Result<Self, InvalidGeocodingRequest> {
        if !(1..=Self::MAX_CANDIDATES as u8).contains(&candidate_limit) {
            return Err(InvalidGeocodingRequest::CandidateLimit);
        }
        Ok(Self {
            query,
            purpose,
            candidate_limit,
            submitted: None,
            constraint: None,
            bias_position: None,
            language: None,
        })
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
    pub fn purpose(&self) -> GeocodingPurpose {
        self.purpose
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
            .field("purpose", &self.purpose)
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

/// Permission accompanies all derived components, including country-only evidence.
/// It is required at persistence/cache boundaries, not merely for raw provider payloads.
#[derive(Clone)]
pub struct GeocodingUsage {
    purpose: GeocodingPurpose,
    storage_review: Option<String>,
}

impl GeocodingUsage {
    pub fn single_use(purpose: GeocodingPurpose) -> Self {
        Self {
            purpose,
            storage_review: None,
        }
    }
    /// Adapters call this only after checking their reviewed provider/product/region policy.
    pub fn retained(purpose: GeocodingPurpose, review: String) -> Self {
        Self {
            purpose,
            storage_review: Some(review),
        }
    }
    pub fn purpose(&self) -> GeocodingPurpose {
        self.purpose
    }
    pub fn storage_review(&self) -> Option<&str> {
        self.storage_review.as_deref()
    }
    pub fn permits_storage_in(&self, scope: GeocodingStorageScope) -> bool {
        self.storage_review
            .as_ref()
            .is_some_and(|r| !r.trim().is_empty())
            && self.purpose.storage_scope() == Some(scope)
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

impl fmt::Debug for GeocodingUsage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GeocodingUsage")
            .field("purpose", &self.purpose)
            .field("storage_reviewed", &self.storage_review.is_some())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct GeocodingResult {
    pub outcome: GeocodingOutcome,
    pub usage: GeocodingUsage,
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
        if result.usage.purpose() != request.purpose() {
            return Err(GeocodingError {
                kind: GeocodingFailure::InvalidResponse,
                source: application::error::static_error(
                    "provider changed the requested privacy purpose",
                ),
            });
        }
        if let Some(scope) = request.purpose().storage_scope()
            && !result.usage.permits_storage_in(scope)
        {
            return Err(GeocodingError {
                kind: GeocodingFailure::StorageNotPermitted,
                source: application::error::static_error(
                    "provider did not grant the requested retention scope",
                ),
            });
        }
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
    async fn handler_preserves_ambiguity_and_checks_provider_bounds_and_privacy() {
        let request = GeocodingRequest::new(
            AddressText::new("test").unwrap(),
            GeocodingPurpose::PrivatePreview {
                user_id: UserId::new(),
            },
            2,
        )
        .unwrap();
        let result = GeocodingResult {
            outcome: GeocodingOutcome::Candidates(
                CandidateSet::new(vec![candidate(), candidate()], 2).unwrap(),
            ),
            usage: GeocodingUsage::single_use(request.purpose()),
        };
        let handler = GeocodeHandler::new(Arc::new(StubProvider(result)));
        let result = handler.geocode(&request).await.unwrap();
        assert!(
            matches!(result.outcome, GeocodingOutcome::Candidates(c) if c.has_multiple_candidates())
        );
        let smaller_request =
            GeocodingRequest::new(request.query().clone(), request.purpose(), 1).unwrap();
        assert_eq!(
            GeocodingFailure::InvalidResponse,
            handler.geocode(&smaller_request).await.unwrap_err().kind
        );
        let changed_scope = GeocodeHandler::new(StubProvider(GeocodingResult {
            outcome: GeocodingOutcome::NoMatch,
            usage: GeocodingUsage::retained(GeocodingPurpose::DealerReusable, "review".into()),
        }));
        assert_eq!(
            GeocodingFailure::InvalidResponse,
            changed_scope.geocode(&request).await.unwrap_err().kind
        );
        let retained = GeocodingRequest::new(
            request.query().clone(),
            GeocodingPurpose::PrivateRetained {
                user_id: UserId::new(),
            },
            2,
        )
        .unwrap();
        let missing_permission = GeocodeHandler::new(StubProvider(GeocodingResult {
            outcome: GeocodingOutcome::NoMatch,
            usage: GeocodingUsage::single_use(retained.purpose()),
        }));
        assert_eq!(
            GeocodingFailure::StorageNotPermitted,
            missing_permission
                .geocode(&retained)
                .await
                .unwrap_err()
                .kind
        );
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
                GeocodingRequest::new(
                    AddressText::new("Berlin").unwrap(),
                    GeocodingPurpose::DealerPreview,
                    n
                )
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
    fn retention_never_crosses_user_or_public_scopes() {
        let alice = UserId::new();
        let bob = UserId::new();
        let scope = GeocodingStorageScope::PrivateUser { user_id: alice };
        for purpose in [
            GeocodingPurpose::DealerPreview,
            GeocodingPurpose::PrivatePreview { user_id: alice },
            GeocodingPurpose::PrivateRetained { user_id: alice },
            GeocodingPurpose::DealerReusable,
        ] {
            assert!(!GeocodingUsage::single_use(purpose).permits_storage_in(scope));
            assert!(
                !GeocodingUsage::single_use(purpose)
                    .permits_storage_in(GeocodingStorageScope::DealerShared)
            );
        }
        let private = GeocodingUsage::retained(
            GeocodingPurpose::PrivateRetained { user_id: alice },
            "review".into(),
        );
        assert!(private.permits_storage_in(scope));
        assert!(!private.permits_storage_in(GeocodingStorageScope::PrivateUser { user_id: bob }));
        assert!(!private.permits_storage_in(GeocodingStorageScope::DealerShared));
        let dealer = GeocodingUsage::retained(GeocodingPurpose::DealerReusable, "review".into());
        assert!(dealer.permits_storage_in(GeocodingStorageScope::DealerShared));
        assert!(!dealer.permits_storage_in(scope));
    }

    #[test]
    fn score_is_validated_and_debug_is_safe() {
        for n in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(ProviderMatchScore::new(n).is_none());
        }
        let request = GeocodingRequest::new(
            AddressText::new("sensitive address").unwrap(),
            GeocodingPurpose::PrivatePreview {
                user_id: UserId::new(),
            },
            5,
        )
        .unwrap()
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
