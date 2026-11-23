//! Durable resolution use cases. Inputs, scopes and review authority are supplied by
//! the owning service; adapters own codecs, leases and database uniqueness.
use crate::geocoding::*;
use application::{
    error::{BoxError, FailureKind, static_error},
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use geo::{DerivedGeography, GeographicDescription, SpatialPrecision, SubmittedGeography};
use sha2::{Digest, Sha256};
use std::{fmt, time::Duration};
use uuid::Uuid;

domain_primitives::object_id_newtype!(ResolutionId, "gres");
pub const POLICY_VERSION: &str = "PRECISION_FIRST_V1";
pub const USER_BROWSING_POLICY_VERSION: &str = "AUTOMATIC_USER_BROWSING_V1";
/// The caller picked one place from address suggestions; see [`evaluate_selected_place`].
pub const SELECTED_PLACE_POLICY_VERSION: &str = "SELECTED_PLACE_V1";
pub const MAX_ATTEMPTS: u32 = 5;
/// Maximum subjects in one coherent resolution read. Larger callers batch within their snapshot.
pub const MAX_RESOLUTION_SUBJECTS: usize = 100;

pub fn retry_delay(attempt: u32) -> time::Duration {
    time::Duration::seconds(30 * 2_i64.pow(attempt.saturating_sub(1).min(MAX_ATTEMPTS - 1)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeKind {
    Party,
    User,
    Source,
    SharedBusiness,
}
impl ScopeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Party => "PARTY",
            Self::User => "USER",
            Self::Source => "SOURCE",
            Self::SharedBusiness => "SHARED_BUSINESS",
        }
    }
    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "PARTY" => Some(Self::Party),
            "USER" => Some(Self::User),
            "SOURCE" => Some(Self::Source),
            "SHARED_BUSINESS" => Some(Self::SharedBusiness),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ResolutionScope {
    kind: ScopeKind,
    owner: Option<Uuid>,
}
impl ResolutionScope {
    /// Shared business reuse must be explicitly authorized by the owning service.
    pub fn new(kind: ScopeKind, owner: Option<Uuid>) -> Result<Self, ResolutionError> {
        if (kind == ScopeKind::SharedBusiness) != owner.is_none()
            || owner.is_some_and(|id| {
                id.get_version_num() != 7 || id.get_variant() != uuid::Variant::RFC4122
            })
        {
            return Err(ResolutionError::InvalidInput);
        }
        Ok(Self { kind, owner })
    }
    pub fn kind(self) -> ScopeKind {
        self.kind
    }
    pub fn owner(self) -> Option<Uuid> {
        self.owner
    }
}
impl fmt::Debug for ResolutionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResolutionScope(<redacted>)")
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionPurpose {
    PartyLocation,
    UserLocation,
}
impl RetentionPurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PartyLocation => "PARTY_LOCATION",
            Self::UserLocation => "USER_LOCATION",
        }
    }
    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "PARTY_LOCATION" => Some(Self::PartyLocation),
            "USER_LOCATION" => Some(Self::UserLocation),
            _ => None,
        }
    }
}
#[derive(Clone)]
pub struct ResolutionInput {
    request: GeocodingRequest,
    scope: ResolutionScope,
    purpose: RetentionPurpose,
    selected_place: Option<PlaceReference>,
}
impl ResolutionInput {
    pub fn new(
        request: GeocodingRequest,
        scope: ResolutionScope,
        purpose: RetentionPurpose,
    ) -> Result<Self, ResolutionError> {
        // Private user data can never be admitted to business/shared cache namespaces.
        if (purpose == RetentionPurpose::UserLocation && scope.kind != ScopeKind::User)
            || (purpose == RetentionPurpose::PartyLocation && scope.kind == ScopeKind::User)
            || request.candidate_limit() < 2
        {
            return Err(ResolutionError::InvalidInput);
        }
        Ok(Self {
            request,
            scope,
            purpose,
            selected_place: None,
        })
    }
    /// Resolve the place the caller picked instead of searching for the request's query. The
    /// request stays the fallback when the provider no longer knows the reference.
    pub fn with_selected_place(mut self, place: PlaceReference) -> Self {
        self.selected_place = Some(place);
        self
    }
    pub fn request(&self) -> &GeocodingRequest {
        &self.request
    }
    pub fn selected_place(&self) -> Option<&PlaceReference> {
        self.selected_place.as_ref()
    }
    pub fn scope(&self) -> ResolutionScope {
        self.scope
    }
    pub fn purpose(&self) -> RetentionPurpose {
        self.purpose
    }
    /// A selected place has one policy for every purpose. Otherwise browsing addresses accept
    /// eligible premises evidence without a review step, and business-location review retains
    /// its independent, existing policy.
    pub fn policy_version(&self) -> &'static str {
        if self.selected_place.is_some() {
            return SELECTED_PLACE_POLICY_VERSION;
        }
        self.free_text_policy_version()
    }
    /// The purpose's policy for free-text results, including a selection's fallback search.
    pub fn free_text_policy_version(&self) -> &'static str {
        match self.purpose {
            RetentionPurpose::UserLocation => USER_BROWSING_POLICY_VERSION,
            RetentionPurpose::PartyLocation => POLICY_VERSION,
        }
    }
    /// Length-prefixed exact bytes: preserve units, punctuation, Unicode and whitespace.
    /// A digest remains sensitive and is never a public identifier or metric dimension.
    pub fn fingerprint(&self, profile: &str) -> String {
        let mut hash = Sha256::new();
        let mut field = |s: &str| {
            hash.update((s.len() as u64).to_be_bytes());
            hash.update(s.as_bytes());
        };
        field("RESOLUTION_INPUT_V1");
        field(profile);
        field(self.policy_version());
        field(geo::reference::ReferenceRelease::CURRENT.as_str());
        field(self.scope.kind.as_str());
        field(
            &self
                .scope
                .owner
                .map(|id| id.to_string())
                .unwrap_or_default(),
        );
        field(self.purpose.as_str());
        field(self.request.query().as_str());
        field(&self.request.candidate_limit().to_string());
        if let Some(s) = self.request.submitted() {
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
        if let Some(c) = self.request.constraint() {
            field("CONSTRAINT");
            field(c.country().map(|v| v.alpha2()).unwrap_or_default());
            field(c.subdivision().map(|v| v.as_str()).unwrap_or_default());
        } else {
            field("NO_CONSTRAINT");
        }
        if let Some(p) = self.request.bias_position() {
            field("BIAS");
            field(&p.latitude_degrees().to_bits().to_string());
            field(&p.longitude_degrees().to_bits().to_string());
        } else {
            field("NO_BIAS");
        }
        field(
            self.request
                .language()
                .map(|v| v.as_str())
                .unwrap_or_default(),
        );
        // Appended only for selections so free-text fingerprints, and their reuse, stay stable.
        if let Some(place) = &self.selected_place {
            field("SELECTED_PLACE");
            field(place.as_str());
        }
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn scope_key(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(self.scope.kind.as_str());
        if let Some(id) = self.scope.owner {
            hash.update(id.as_bytes());
        }
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}
impl fmt::Debug for ResolutionInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResolutionInput(<redacted>)")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ResolutionSubject {
    kind: RetentionPurpose,
    id: Uuid,
}
impl ResolutionSubject {
    pub fn new(kind: RetentionPurpose, id: Uuid) -> Result<Self, ResolutionError> {
        if id.get_version_num() != 7 || id.get_variant() != uuid::Variant::RFC4122 {
            return Err(ResolutionError::InvalidInput);
        }
        Ok(Self { kind, id })
    }
    pub fn kind(self) -> RetentionPurpose {
        self.kind
    }
    pub fn id(self) -> Uuid {
        self.id
    }
}
impl fmt::Debug for ResolutionSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ResolutionSubject(<redacted>)")
    }
}
#[derive(Debug, Clone, Copy)]
pub struct ResolutionBinding {
    pub subject: ResolutionSubject,
    pub input_revision: u64,
    pub guard: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionWork {
    pub id: ResolutionId,
    pub generation: u64,
}
pub struct ResolutionClaim {
    pub work: ResolutionWork,
    pub token: Uuid,
    pub input: ResolutionInput,
    pub profile: String,
    pub attempt: u32,
}
/// Why a wake-up did or did not reserve a provider call for its generation.
pub enum ClaimOutcome {
    Claimed(Box<ResolutionClaim>),
    /// Nothing remains for this generation: it is erased, unbound or superseded, has current
    /// evidence, reached a durable status other than `PENDING`, or exhausted its attempts.
    /// Only a newer generation, which publishes its own wake-up, can call the provider again.
    Complete,
    /// The generation is runnable, but the provider circuit is open after consecutive
    /// provider-health failures. Cleanup recovery republishes the work once it closes.
    ProviderGated,
    /// A retry can succeed soon: another claim holds the lease or the retry backoff has not
    /// elapsed.
    Deferred,
}
/// What a fenced completion did with its claim's provider reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionOutcome {
    /// The reply was recorded and left the resolution in this status.
    Recorded(ResolutionStatus),
    /// The generation no longer needs the reply because it was erased, unbound or superseded.
    Stale,
    /// The claim's lease expired or was replaced; the generation may still need a call.
    LeaseLost,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolutionStatus {
    Pending,
    Accepted,
    Ambiguous,
    NoMatch,
    Failed,
    Blocked,
}
impl ResolutionStatus {
    /// Whether this outcome records provider evidence. Evidence has no time limit: the provider
    /// terms set none, so it stays until input, Retry, storage rights or bindings change.
    pub fn carries_evidence(self) -> bool {
        matches!(self, Self::Accepted | Self::Ambiguous | Self::NoMatch)
    }
    pub fn after_failure(kind: GeocodingFailure, attempt: u32) -> Self {
        match kind {
            GeocodingFailure::Authentication
            | GeocodingFailure::Configuration
            | GeocodingFailure::StorageNotPermitted
            | GeocodingFailure::UnsupportedInput
            | GeocodingFailure::InvalidResponse => Self::Blocked,
            _ if attempt >= MAX_ATTEMPTS => Self::Failed,
            _ => Self::Pending,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Accepted => "ACCEPTED",
            Self::Ambiguous => "AMBIGUOUS",
            Self::NoMatch => "NO_MATCH",
            Self::Failed => "FAILED",
            Self::Blocked => "BLOCKED",
        }
    }
    pub fn from_code(s: &str) -> Option<Self> {
        match s {
            "PENDING" => Some(Self::Pending),
            "ACCEPTED" => Some(Self::Accepted),
            "AMBIGUOUS" => Some(Self::Ambiguous),
            "NO_MATCH" => Some(Self::NoMatch),
            "FAILED" => Some(Self::Failed),
            "BLOCKED" => Some(Self::Blocked),
            _ => None,
        }
    }
}
/// Permitted, normalized evidence only; never the raw provider response.
#[derive(Clone)]
pub struct ResolutionCandidate {
    pub geography: DerivedGeography,
    pub acceptable: bool,
    pub provider: String,
    pub product: String,
    pub data_source: String,
    pub region: String,
    pub profile: String,
    pub external_reference: String,
    pub attribution_url: String,
}
impl fmt::Debug for ResolutionCandidate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolutionCandidate")
            .field("acceptable", &self.acceptable)
            .finish_non_exhaustive()
    }
}
#[derive(Debug, Clone)]
pub struct ResolutionEvidence {
    pub status: ResolutionStatus,
    pub candidates: Vec<ResolutionCandidate>,
    pub accepted_index: Option<usize>,
}
#[derive(Debug, Clone)]
pub struct ResolutionView {
    pub scope: ResolutionScope,
    pub work: ResolutionWork,
    pub input_revision: u64,
    pub guard: u64,
    pub status: ResolutionStatus,
    pub evidence: Option<ResolutionEvidence>,
    pub selected_candidate: Option<usize>,
    pub last_failure: Option<String>,
    pub attempts: u32,
    /// The place the subject's caller picked for this input, if any.
    pub selected_place: Option<PlaceReference>,
}
impl ResolutionView {
    /// Readers expose only evidence that is still eligible for this binding. Submitted
    /// geography remains a separate value owned by the caller's bounded context.
    pub fn accepted_geography(&self) -> Option<&DerivedGeography> {
        let e = self.evidence.as_ref()?;
        let index = self.selected_candidate.or(e.accepted_index)?;
        e.candidates.get(index).map(|c| &c.geography)
    }
}
/// The input to bind for a subject. A newly picked place wins; otherwise the currently bound
/// pick is kept while the subject's input revision is unchanged, so label, lifecycle and other
/// edits that do not touch the address keep the pick. A changed address is free text again.
pub fn carry_selected_place(
    input: ResolutionInput,
    binding: &ResolutionBinding,
    current: Option<&ResolutionView>,
    picked: Option<PlaceReference>,
) -> ResolutionInput {
    let kept = || {
        current
            .filter(|view| view.input_revision == binding.input_revision)
            .and_then(|view| view.selected_place.clone())
    };
    match picked.or_else(kept) {
        Some(place) => input.with_selected_place(place),
        None => input,
    }
}
/// `picked` when it differs from the place currently bound for the subject. Re-sending the
/// current pick is not an input change.
pub fn newly_picked(
    picked: Option<PlaceReference>,
    current: Option<&ResolutionView>,
) -> Option<PlaceReference> {
    picked.filter(|place| current.and_then(|view| view.selected_place.as_ref()) != Some(place))
}
pub enum ResolutionCompletion {
    Evidence(ResolutionEvidence),
    Failure(GeocodingFailure),
}

/// A unique compatible result may add country/subdivision. Private user browsing also
/// accepts a premises point when the single returned candidate has no approximation or
/// unsupported precision. Other purposes retain explicit point review. Provider scores
/// and client autocomplete flags never establish acceptance.
pub fn evaluate(
    input: &ResolutionInput,
    result: GeocodingResult,
    profile: &str,
) -> Result<ResolutionEvidence, GeocodingError> {
    if !result.retention.permits_storage() {
        return Err(GeocodingError {
            kind: GeocodingFailure::StorageNotPermitted,
            source: static_error("retained resolution requires storage permission"),
        });
    }
    let GeocodingOutcome::Candidates(set) = result.outcome else {
        return Ok(ResolutionEvidence {
            status: ResolutionStatus::NoMatch,
            candidates: vec![],
            accepted_index: None,
        });
    };
    let (mut candidates, compatible_count) = normalize_candidates(input, &set, profile)?;
    // Count all returned alternatives, including ones we cannot normalize.
    if set.as_slice().len() == 1 && compatible_count == 1 {
        if input.purpose == RetentionPurpose::UserLocation
            && candidates[0].acceptable
            && candidates[0].geography.position().is_some()
            && candidates[0]
                .geography
                .description()
                .and_then(|d| d.country())
                .is_some()
            && !set.as_slice()[0]
                .issues()
                .contains(&CandidateIssue::UnsupportedCountry)
        {
            return Ok(ResolutionEvidence {
                status: ResolutionStatus::Accepted,
                accepted_index: Some(0),
                candidates,
            });
        }
        if let Some(accepted) = administrative_only(&candidates[0])? {
            candidates.push(accepted);
            return Ok(ResolutionEvidence {
                status: ResolutionStatus::Accepted,
                accepted_index: Some(candidates.len() - 1),
                candidates,
            });
        }
    }
    Ok(ResolutionEvidence {
        status: ResolutionStatus::Ambiguous,
        candidates,
        accepted_index: None,
    })
}
/// The caller picked this place, so one usable candidate compatible with the submitted
/// assertions is accepted for every purpose. Its point is accepted only at precise premises
/// precision with a known country; a coarser pick contributes administrative geography only,
/// as an automatic coarse acceptance does. An incompatible pick stays ambiguous.
pub fn evaluate_selected_place(
    input: &ResolutionInput,
    result: GeocodingResult,
    profile: &str,
) -> Result<ResolutionEvidence, GeocodingError> {
    if !result.retention.permits_storage() {
        return Err(GeocodingError {
            kind: GeocodingFailure::StorageNotPermitted,
            source: static_error("retained resolution requires storage permission"),
        });
    }
    let GeocodingOutcome::Candidates(set) = result.outcome else {
        return Err(GeocodingError {
            kind: GeocodingFailure::InvalidResponse,
            source: static_error("place lookup found no place"),
        });
    };
    if set.has_multiple_candidates() {
        return Err(GeocodingError {
            kind: GeocodingFailure::InvalidResponse,
            source: static_error("place lookup returned several places"),
        });
    }
    let (mut candidates, compatible_count) = normalize_candidates(input, &set, profile)?;
    if compatible_count != 1 {
        return Ok(ResolutionEvidence {
            status: ResolutionStatus::Ambiguous,
            candidates,
            accepted_index: None,
        });
    }
    let picked = &candidates[0];
    let known_country = picked
        .geography
        .description()
        .and_then(GeographicDescription::country)
        .is_some()
        && !set.as_slice()[0]
            .issues()
            .contains(&CandidateIssue::UnsupportedCountry);
    if picked.acceptable && picked.geography.position().is_some() && known_country {
        return Ok(ResolutionEvidence {
            status: ResolutionStatus::Accepted,
            accepted_index: Some(0),
            candidates,
        });
    }
    let Some(accepted) = administrative_only(picked)? else {
        return Ok(ResolutionEvidence {
            status: ResolutionStatus::Ambiguous,
            candidates,
            accepted_index: None,
        });
    };
    candidates.push(accepted);
    Ok(ResolutionEvidence {
        status: ResolutionStatus::Accepted,
        accepted_index: Some(candidates.len() - 1),
        candidates,
    })
}
/// Permitted, normalized evidence of every returned alternative that has retainable geography,
/// and how many of them are usable and compatible with the input's assertions.
fn normalize_candidates(
    input: &ResolutionInput,
    set: &CandidateSet,
    profile: &str,
) -> Result<(Vec<ResolutionCandidate>, usize), GeocodingError> {
    if set.as_slice().len() > input.request.candidate_limit() {
        return Err(GeocodingError {
            kind: GeocodingFailure::InvalidResponse,
            source: static_error("candidate bound exceeded"),
        });
    }
    let mut candidates = vec![];
    let mut compatible_count = 0;
    for c in set.as_slice() {
        let p = c.provenance();
        if p.interpretation_profile != profile {
            return Err(GeocodingError {
                kind: GeocodingFailure::Configuration,
                source: static_error("provider interpretation profile changed"),
            });
        }
        let Some(g) = c.geography() else {
            continue;
        };
        let compatible =
            candidate_compatible(input, g) && c.usability() == CandidateUsability::Usable;
        if compatible {
            compatible_count += 1;
        }
        let precise = g
            .position()
            .is_some_and(|p| p.precision() == SpatialPrecision::Premises)
            && !c.issues().contains(&CandidateIssue::ApproximatePosition)
            && !c.issues().contains(&CandidateIssue::UnsupportedPrecision);
        candidates.push(ResolutionCandidate {
            geography: g.clone(),
            acceptable: compatible && (g.position().is_none() || precise),
            provider: p.provider.into(),
            product: p.product.into(),
            data_source: p.data_source.into(),
            region: p.region.clone(),
            profile: p.interpretation_profile.into(),
            external_reference: p.external_reference.clone(),
            attribution_url: p.attribution_url.into(),
        });
    }
    Ok((candidates, compatible_count))
}
/// An acceptable copy of `candidate` that keeps only its country and subdivision, or `None`
/// when it has neither. The complete candidate stays in the evidence for review; the copy
/// retains its provenance.
fn administrative_only(
    candidate: &ResolutionCandidate,
) -> Result<Option<ResolutionCandidate>, GeocodingError> {
    let coarse = candidate
        .geography
        .description()
        .map(|d| {
            GeographicDescription::new_in_release(
                d.reference_release(),
                None,
                d.country(),
                d.subdivision(),
                None,
            )
        })
        .transpose()
        .map_err(|e| GeocodingError {
            kind: GeocodingFailure::InvalidResponse,
            source: Box::new(e),
        })?
        .flatten();
    Ok(
        DerivedGeography::new(coarse, None).map(|geography| ResolutionCandidate {
            geography,
            acceptable: true,
            ..candidate.clone()
        }),
    )
}
/// Whether the input's assertions and search constraint leave `geography` a possible answer.
pub fn candidate_compatible(input: &ResolutionInput, geography: &DerivedGeography) -> bool {
    let derived = geography.description();
    let submitted = input
        .request
        .submitted()
        .map(SubmittedGeography::description);
    let assertions_agree = submitted.is_none_or(|asserted| {
        administrative_matches(derived, asserted.country(), asserted.subdivision())
    });
    let constraint_holds = input
        .request
        .constraint()
        .is_none_or(|c| administrative_matches(derived, c.country(), c.subdivision()));
    let postal_code_holds = input.purpose != RetentionPurpose::UserLocation
        || submitted.is_none_or(|asserted| postal_code_matches(derived, asserted.postal_code()));
    assertions_agree && constraint_holds && postal_code_holds
}

/// Every asserted country and subdivision is present in `derived` and equal to it.
fn administrative_matches(
    derived: Option<&GeographicDescription>,
    country: Option<geo::CountryCode>,
    subdivision: Option<geo::SubdivisionCode>,
) -> bool {
    country.is_none_or(|c| derived.and_then(GeographicDescription::country) == Some(c))
        && subdivision
            .is_none_or(|s| derived.and_then(GeographicDescription::subdivision) == Some(s))
}

/// The asserted postal code, if any, is present in `derived` and equivalent to it under the
/// derived country's rules, or equal to it when that country is unknown.
fn postal_code_matches(
    derived: Option<&GeographicDescription>,
    asserted: Option<&geo::PostalCode>,
) -> bool {
    asserted.is_none_or(|postal| {
        derived
            .and_then(GeographicDescription::postal_code)
            .is_some_and(
                |derived_postal| match derived.and_then(GeographicDescription::country) {
                    Some(country) => derived_postal.equivalent_in(postal, country),
                    None => derived_postal == postal,
                },
            )
    })
}

#[derive(thiserror::Error, Debug)]
pub enum ResolutionError {
    #[error("invalid resolution input")]
    InvalidInput,
    #[error("resolution authorization required")]
    Forbidden,
    #[error("resolution input or candidate changed")]
    Conflict,
    #[error("resolution source is missing")]
    Missing,
    #[error("resolution provider failed")]
    Provider(#[source] GeocodingError),
    #[error("resolution storage unavailable")]
    Storage(#[source] BoxError),
    #[error("invalid persisted resolution")]
    InvalidState(#[source] BoxError),
    #[error("resolution transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
}
impl ResolutionError {
    /// The bounded kind every worker boundary records for this failure.
    pub fn failure_kind(&self) -> FailureKind {
        match self {
            Self::Provider(provider) => FailureKind::Provider(provider.kind.diagnostic_category()),
            Self::Storage(_) | Self::Transaction(_) => FailureKind::Storage,
            Self::InvalidState(_) => FailureKind::InvalidPersistedState,
            Self::InvalidInput => FailureKind::Rejected("invalid_input"),
            Self::Forbidden => FailureKind::Rejected("forbidden"),
            Self::Conflict => FailureKind::Rejected("conflict"),
            Self::Missing => FailureKind::Rejected("missing"),
        }
    }
}
#[async_trait::async_trait]
pub trait ResolutionRepository: Send {
    async fn bind(
        &mut self,
        input: &ResolutionInput,
        binding: ResolutionBinding,
    ) -> Result<ResolutionId, ResolutionError>;
    async fn detach(&mut self, subject: ResolutionSubject) -> Result<(), ResolutionError>;
    async fn claim(
        &mut self,
        work: ResolutionWork,
        profile: &str,
    ) -> Result<ClaimOutcome, ResolutionError>;
    async fn complete(
        &mut self,
        claim: &ResolutionClaim,
        completion: ResolutionCompletion,
    ) -> Result<CompletionOutcome, ResolutionError>;
    /// Publish fresh fenced wake-ups for runnable work without resetting paid attempts.
    async fn recover_pending(&mut self, limit: u32) -> Result<u32, ResolutionError>;
    /// Purge provider derivatives that revoked storage rights or orphaning made ineligible, and
    /// commit invalidation before requeue. Evidence never expires by time.
    async fn expire(&mut self, limit: u32) -> Result<u32, ResolutionError>;
    async fn select(
        &mut self,
        binding: ResolutionBinding,
        work: ResolutionWork,
        index: usize,
        actor: &str,
    ) -> Result<(), ResolutionError>;
    async fn requeue(
        &mut self,
        binding: ResolutionBinding,
        expected: ResolutionWork,
        force: bool,
    ) -> Result<(), ResolutionError>;
}
#[async_trait::async_trait]
pub trait ResolutionReader: Send {
    async fn inspect(
        &mut self,
        subject: ResolutionSubject,
    ) -> Result<Option<ResolutionView>, ResolutionError>;
    /// At most `MAX_RESOLUTION_SUBJECTS`; preserve order and avoid one read per address.
    async fn inspect_many(
        &mut self,
        subjects: &[ResolutionSubject],
    ) -> Result<Vec<Option<ResolutionView>>, ResolutionError>;
}
pub trait ResolutionRepositoryFactory<Tx>: Send + Sync {
    fn resolutions<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ResolutionRepository + 'tx;
    fn resolution_reader<'tx>(&'tx self, tx: &'tx mut Tx) -> impl ResolutionReader + 'tx;
}
/// How one wake-up left its resolution. Durable state is authoritative; the variant says
/// whether redelivering the same wake-up could still achieve anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The wake-up's generation is settled, whether resolved, terminal or stale.
    Complete,
    /// The provider circuit refuses calls for runnable work; see [`ClaimOutcome::ProviderGated`].
    ProviderGated,
    /// Contention or a scheduled retry; a redelivery can succeed.
    Deferred,
}
impl ResolveOutcome {
    /// Transient provider failures, recorded as `PENDING`, keep their error so redelivery
    /// retries them after backoff. A recorded terminal status or a stale reply settles the
    /// wake-up regardless.
    fn after_completion(
        completion: CompletionOutcome,
        evidence: Result<ResolutionEvidence, GeocodingError>,
    ) -> Result<Self, ResolutionError> {
        match completion {
            CompletionOutcome::Stale => Ok(Self::Complete),
            CompletionOutcome::Recorded(status) if status != ResolutionStatus::Pending => {
                Ok(Self::Complete)
            }
            CompletionOutcome::Recorded(_) | CompletionOutcome::LeaseLost => {
                evidence.map_err(ResolutionError::Provider)?;
                Ok(Self::Deferred)
            }
        }
    }
}
#[async_trait::async_trait]
pub trait ResolveUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        work: ResolutionWork,
    ) -> Result<ResolveOutcome, ResolutionError>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionMaintenanceCounts {
    pub expired: u32,
    pub requeued: u32,
}

#[async_trait::async_trait]
pub trait MaintainResolutionsUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        limit: u32,
    ) -> Result<ResolutionMaintenanceCounts, ResolutionError>;
}

/// Database housekeeping creates durable wake-ups; only the queue worker calls providers.
pub struct MaintainResolutionsHandler<U, R> {
    uow: U,
    repository: R,
}

impl<U, R> MaintainResolutionsHandler<U, R> {
    pub fn new(uow: U, repository: R) -> Self {
        Self { uow, repository }
    }
}

#[async_trait::async_trait]
impl<U: UnitOfWork, R: ResolutionRepositoryFactory<U::Tx>> MaintainResolutionsUseCase
    for MaintainResolutionsHandler<U, R>
{
    async fn execute(
        &self,
        context: &OperationContext,
        limit: u32,
    ) -> Result<ResolutionMaintenanceCounts, ResolutionError> {
        authorize_worker(context)?;
        if !(1..=100).contains(&limit) {
            return Err(ResolutionError::InvalidInput);
        }
        let mut tx = self.uow.begin().await?;
        // Recover first so newly expired results receive just the expiry wake-up.
        let requeued = self
            .repository
            .resolutions(&mut tx)
            .recover_pending(limit)
            .await?;
        let expired = self.repository.resolutions(&mut tx).expire(limit).await?;
        tx.commit().await?;
        tracing::info!(metric = "geo_resolution_maintenance", expired, requeued);
        Ok(ResolutionMaintenanceCounts { expired, requeued })
    }
}
pub struct ResolveHandler<U, R, P> {
    uow: U,
    repository: R,
    provider: P,
    profile: String,
}
impl<U, R, P> ResolveHandler<U, R, P> {
    pub fn new(uow: U, repository: R, provider: P, profile: String) -> Self {
        Self {
            uow,
            repository,
            provider,
            profile,
        }
    }
}
/// Bounds one provider call; a timeout is a provider failure like any other.
async fn bounded<T>(
    call: impl std::future::Future<Output = Result<T, GeocodingError>>,
) -> Result<T, GeocodingError> {
    tokio::time::timeout(Duration::from_secs(10), call)
        .await
        .map_err(|e| GeocodingError {
            kind: GeocodingFailure::Timeout,
            source: Box::new(e),
        })?
}
impl<U, R, P: GeocodingProvider> ResolveHandler<U, R, P> {
    /// Evidence for a claim and the method that produced it. A selected place the provider no
    /// longer knows falls back to searching for the input's text under the purpose's policy.
    async fn resolve(
        &self,
        claim: &ResolutionClaim,
    ) -> (&'static str, Result<ResolutionEvidence, GeocodingError>) {
        let input = &claim.input;
        if let Some(place) = input.selected_place() {
            match bounded(self.provider.look_up_place(place, input.request())).await {
                Ok(PlaceLookup::Found(result)) => {
                    return (
                        "SELECTED_PLACE",
                        evaluate_selected_place(input, result, &claim.profile),
                    );
                }
                Ok(PlaceLookup::Unknown) => {}
                Err(e) => return ("SELECTED_PLACE", Err(e)),
            }
        }
        let evidence = bounded(self.provider.geocode(input.request()))
            .await
            .and_then(|result| evaluate(input, result, &claim.profile));
        ("GEOCODER", evidence)
    }
}
fn authorize_worker(context: &OperationContext) -> Result<(), ResolutionError> {
    match &context.principal {
        Principal::Service(_) | Principal::System => Ok(()),
        _ => Err(ResolutionError::Forbidden),
    }
}
#[async_trait::async_trait]
impl<U: UnitOfWork, R: ResolutionRepositoryFactory<U::Tx>, P: GeocodingProvider> ResolveUseCase
    for ResolveHandler<U, R, P>
{
    async fn execute(
        &self,
        context: &OperationContext,
        work: ResolutionWork,
    ) -> Result<ResolveOutcome, ResolutionError> {
        authorize_worker(context)?;
        let mut tx = self.uow.begin().await?;
        let claim = self
            .repository
            .resolutions(&mut tx)
            .claim(work, &self.profile)
            .await?;
        tx.commit().await?;
        let claim = match claim {
            ClaimOutcome::Claimed(c) => c,
            ClaimOutcome::Complete => return Ok(ResolveOutcome::Complete),
            ClaimOutcome::ProviderGated => return Ok(ResolveOutcome::ProviderGated),
            ClaimOutcome::Deferred => return Ok(ResolveOutcome::Deferred),
        };
        // No database transaction or connection survives across provider I/O.
        let (method, evidence) = self.resolve(&claim).await;
        let completion = match &evidence {
            Ok(e) => ResolutionCompletion::Evidence(e.clone()),
            Err(e) => ResolutionCompletion::Failure(e.kind),
        };
        let mut tx = self.uow.begin().await?;
        let completed = self
            .repository
            .resolutions(&mut tx)
            .complete(&claim, completion)
            .await?;
        tx.commit().await?;
        let recorded = match completed {
            CompletionOutcome::Recorded(status) => Some(status),
            CompletionOutcome::Stale | CompletionOutcome::LeaseLost => None,
        };
        tracing::info!(
            metric = "geo_resolution_outcome",
            method,
            policy = claim.input.policy_version(),
            outcome = recorded.map_or("UNCOMPLETED", ResolutionStatus::as_str),
            failure = evidence.as_ref().err().map(|e| e.kind.as_str()),
            attempt = claim.attempt,
            fenced_completion = recorded.is_some()
        );
        ResolveOutcome::after_completion(completed, evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operational_failures_do_not_become_geographic_negative_results() {
        for kind in [
            GeocodingFailure::Unavailable,
            GeocodingFailure::Timeout,
            GeocodingFailure::Throttled,
        ] {
            assert_eq!(
                ResolutionStatus::Pending,
                ResolutionStatus::after_failure(kind, 1)
            );
            assert_eq!(
                ResolutionStatus::Failed,
                ResolutionStatus::after_failure(kind, MAX_ATTEMPTS)
            );
        }
        for kind in [
            GeocodingFailure::Authentication,
            GeocodingFailure::Configuration,
            GeocodingFailure::StorageNotPermitted,
            GeocodingFailure::UnsupportedInput,
            GeocodingFailure::InvalidResponse,
        ] {
            assert_eq!(
                ResolutionStatus::Blocked,
                ResolutionStatus::after_failure(kind, 1)
            );
        }
        assert_eq!(time::Duration::seconds(30), retry_delay(1));
        assert_eq!(time::Duration::seconds(480), retry_delay(MAX_ATTEMPTS));
        for status in [
            ResolutionStatus::Accepted,
            ResolutionStatus::Ambiguous,
            ResolutionStatus::NoMatch,
        ] {
            assert!(status.carries_evidence());
        }
        for status in [
            ResolutionStatus::Pending,
            ResolutionStatus::Failed,
            ResolutionStatus::Blocked,
        ] {
            assert!(!status.carries_evidence());
        }
    }

    use geo::{
        AddressText, CountryCode, GeoPoint, SpatialPosition, SubdivisionCode, SubmittedGeography,
    };

    fn input(query: &str, owner: Uuid) -> ResolutionInput {
        ResolutionInput::new(
            GeocodingRequest::new(AddressText::new(query).unwrap()),
            ResolutionScope::new(ScopeKind::Party, Some(owner)).unwrap(),
            RetentionPurpose::PartyLocation,
        )
        .unwrap()
    }
    fn candidate(country: CountryCode, precision: Option<SpatialPrecision>) -> GeocodingCandidate {
        GeocodingCandidate::new(
            DerivedGeography::new(
                GeographicDescription::new(None, Some(country), None, None).unwrap(),
                precision.map(|p| {
                    SpatialPosition::new(GeoPoint::new(52.5, 13.4).unwrap(), p, None).unwrap()
                }),
            ),
            vec![],
            None,
            GeocodingProvenance {
                provider: "TEST",
                product: "TEST",
                data_source: "TEST",
                region: "TEST".into(),
                interpretation_profile: "TEST_V1",
                external_reference: "test-reference".into(),
                attribution_url: "https://example.test/attribution",
            },
        )
    }
    fn evidence(
        input: &ResolutionInput,
        candidates: Vec<GeocodingCandidate>,
    ) -> ResolutionEvidence {
        evaluate(
            input,
            GeocodingResult {
                outcome: GeocodingOutcome::Candidates(CandidateSet::new(candidates, 5).unwrap()),
                retention: ResultRetention::Storage,
            },
            "TEST_V1",
        )
        .unwrap()
    }
    #[test]
    fn fingerprints_preserve_units_scope_purpose_constraints_and_profiles() {
        let owner = Uuid::now_v7();
        let a = input("123 Street, unit 1", owner);
        for other in [
            input("123 Street, unit 2", owner),
            input("123 Street, unit 1 ", owner),
            input("123 Street, unit 1", Uuid::now_v7()),
        ] {
            assert_ne!(a.fingerprint("TEST_V1"), other.fingerprint("TEST_V1"));
        }
        assert_ne!(a.fingerprint("TEST_V1"), a.fingerprint("TEST_V2"));
        let constrained = ResolutionInput::new(
            a.request
                .clone()
                .with_constraint(GeographicConstraint::new(Some(CountryCode::DEU), None).unwrap()),
            a.scope,
            a.purpose,
        )
        .unwrap();
        assert_ne!(a.fingerprint("TEST_V1"), constrained.fingerprint("TEST_V1"));
        assert!(!format!("{a:?}").contains("123"));
        let user = ResolutionScope::new(ScopeKind::User, Some(owner)).unwrap();
        let private =
            ResolutionInput::new(a.request.clone(), user, RetentionPurpose::UserLocation).unwrap();
        assert_ne!(a.fingerprint("TEST_V1"), private.fingerprint("TEST_V1"));
        assert!(
            ResolutionInput::new(a.request.clone(), a.scope, RetentionPurpose::UserLocation)
                .is_err()
        );
    }
    #[test]
    fn unique_evidence_accepts_administrative_geography_without_a_point() {
        let a = input("123 Street", Uuid::now_v7());
        for precision in [
            None,
            Some(SpatialPrecision::Premises),
            Some(SpatialPrecision::Representative),
        ] {
            let e = evidence(&a, vec![candidate(CountryCode::DEU, precision)]);
            assert_eq!(ResolutionStatus::Accepted, e.status);
            assert!(
                e.candidates[e.accepted_index.unwrap()]
                    .geography
                    .position()
                    .is_none()
            );
        }
        let base = candidate(CountryCode::CAN, None);
        let subdivision = GeocodingCandidate::new(
            DerivedGeography::new(
                GeographicDescription::new(
                    None,
                    Some(CountryCode::CAN),
                    Some(SubdivisionCode::new("CA-BC").unwrap()),
                    None,
                )
                .unwrap(),
                None,
            ),
            vec![],
            None,
            base.provenance().clone(),
        );
        let e = evidence(&a, vec![subdivision]);
        assert_eq!(
            Some(SubdivisionCode::new("CA-BC").unwrap()),
            e.candidates[e.accepted_index.unwrap()]
                .geography
                .description()
                .unwrap()
                .subdivision()
        );
    }
    #[test]
    fn alternatives_contradictions_and_approximate_points_abstain() {
        let mut a = input("123 Street", Uuid::now_v7());
        let e = evidence(
            &a,
            vec![
                candidate(CountryCode::DEU, None),
                candidate(CountryCode::FRA, None),
            ],
        );
        assert_eq!(ResolutionStatus::Ambiguous, e.status);
        assert!(e.accepted_index.is_none());
        a.request = a.request.clone().with_submitted(SubmittedGeography::new(
            GeographicDescription::new(None, Some(CountryCode::DEU), None, None)
                .unwrap()
                .unwrap(),
        ));
        let e = evidence(
            &a,
            vec![candidate(
                CountryCode::FRA,
                Some(SpatialPrecision::Premises),
            )],
        );
        assert_eq!(ResolutionStatus::Ambiguous, e.status);
        assert!(!e.candidates[0].acceptable);
        let e = evidence(
            &a,
            vec![candidate(CountryCode::DEU, Some(SpatialPrecision::Street))],
        );
        assert!(!e.candidates[0].acceptable);
    }
    #[test]
    fn single_use_and_changed_provider_profiles_never_become_retained_evidence() {
        let a = input("test", Uuid::now_v7());
        let error = evaluate(
            &a,
            GeocodingResult {
                outcome: GeocodingOutcome::NoMatch,
                retention: ResultRetention::SingleUse,
            },
            "TEST_V1",
        )
        .unwrap_err();
        assert_eq!(GeocodingFailure::StorageNotPermitted, error.kind);
        let error = evaluate(
            &a,
            GeocodingResult {
                outcome: GeocodingOutcome::Candidates(
                    CandidateSet::new(vec![candidate(CountryCode::DEU, None)], 5).unwrap(),
                ),
                retention: ResultRetention::Storage,
            },
            "TEST_V2",
        )
        .unwrap_err();
        assert_eq!(GeocodingFailure::Configuration, error.kind);
    }

    fn user_input() -> ResolutionInput {
        ResolutionInput::new(
            GeocodingRequest::new(AddressText::new("Synthetic selected address").unwrap()),
            ResolutionScope::new(ScopeKind::User, Some(Uuid::now_v7())).unwrap(),
            RetentionPurpose::UserLocation,
        )
        .unwrap()
    }

    #[test]
    fn user_browsing_automatically_accepts_only_unique_compatible_premises_points() {
        let input = user_input();
        assert_eq!(USER_BROWSING_POLICY_VERSION, input.policy_version());
        for precision in [
            None,
            Some(SpatialPrecision::Premises),
            Some(SpatialPrecision::Street),
            Some(SpatialPrecision::Locality),
            Some(SpatialPrecision::Representative),
        ] {
            let e = evidence(&input, vec![candidate(CountryCode::DEU, precision)]);
            assert_eq!(ResolutionStatus::Accepted, e.status);
            assert_eq!(
                precision == Some(SpatialPrecision::Premises),
                e.candidates[e.accepted_index.unwrap()]
                    .geography
                    .position()
                    .is_some()
            );
        }
        let e = evidence(
            &input,
            vec![
                candidate(CountryCode::DEU, Some(SpatialPrecision::Premises)),
                candidate(CountryCode::DEU, Some(SpatialPrecision::Premises)),
            ],
        );
        assert_eq!(ResolutionStatus::Ambiguous, e.status);
        assert!(e.accepted_index.is_none());
        // Even an alternative we cannot normalize prevents automatic point selection.
        let base = candidate(CountryCode::DEU, None);
        let unsupported = GeocodingCandidate::new(
            None,
            vec![CandidateIssue::UnsupportedCountry],
            None,
            base.provenance().clone(),
        );
        let e = evidence(
            &input,
            vec![
                candidate(CountryCode::DEU, Some(SpatialPrecision::Premises)),
                unsupported,
            ],
        );
        assert_eq!(ResolutionStatus::Ambiguous, e.status);
        assert!(e.accepted_index.is_none());
    }

    #[test]
    fn user_browsing_rejects_approximation_unsupported_precision_and_assertion_mismatches() {
        let mut input = user_input();
        for issue in [
            CandidateIssue::ApproximatePosition,
            CandidateIssue::UnsupportedPrecision,
            CandidateIssue::UnsupportedCountry,
            CandidateIssue::InconsistentComponents,
            CandidateIssue::UnverifiedConstraint,
        ] {
            let base = candidate(CountryCode::DEU, Some(SpatialPrecision::Premises));
            let c = GeocodingCandidate::new(
                base.geography().cloned(),
                vec![issue],
                None,
                base.provenance().clone(),
            );
            let e = evidence(&input, vec![c]);
            assert!(
                e.accepted_index
                    .is_none_or(|i| e.candidates[i].geography.position().is_none())
            );
        }
        input.request = input
            .request
            .clone()
            .with_submitted(SubmittedGeography::new(
                GeographicDescription::new(
                    None,
                    Some(CountryCode::GBR),
                    None,
                    Some(geo::PostalCode::new("SW1A 1AA").unwrap()),
                )
                .unwrap()
                .unwrap(),
            ));
        for c in [
            candidate(CountryCode::DEU, Some(SpatialPrecision::Premises)),
            candidate(CountryCode::GBR, Some(SpatialPrecision::Premises)),
        ] {
            let e = evidence(&input, vec![c]);
            assert_eq!(ResolutionStatus::Ambiguous, e.status);
            assert!(e.accepted_index.is_none());
        }
    }

    fn provider_failure(kind: GeocodingFailure) -> Result<ResolutionEvidence, GeocodingError> {
        Err(GeocodingError {
            kind,
            source: static_error("synthetic provider failure"),
        })
    }

    #[test]
    fn should_settle_the_wake_up_when_completion_records_a_durable_result_or_is_stale() {
        for status in [
            ResolutionStatus::Accepted,
            ResolutionStatus::Ambiguous,
            ResolutionStatus::NoMatch,
            ResolutionStatus::Failed,
            ResolutionStatus::Blocked,
        ] {
            assert_eq!(
                ResolveOutcome::Complete,
                ResolveOutcome::after_completion(
                    CompletionOutcome::Recorded(status),
                    provider_failure(GeocodingFailure::StorageNotPermitted),
                )
                .unwrap(),
                "{status:?}"
            );
        }
        assert_eq!(
            ResolveOutcome::Complete,
            ResolveOutcome::after_completion(
                CompletionOutcome::Stale,
                provider_failure(GeocodingFailure::Timeout),
            )
            .unwrap()
        );
    }

    #[test]
    fn should_keep_the_wake_up_retryable_when_a_retry_is_scheduled_or_the_lease_was_lost() {
        for completion in [
            CompletionOutcome::Recorded(ResolutionStatus::Pending),
            CompletionOutcome::LeaseLost,
        ] {
            let error = ResolveOutcome::after_completion(
                completion,
                provider_failure(GeocodingFailure::Timeout),
            )
            .unwrap_err();
            assert!(
                matches!(&error, ResolutionError::Provider(e) if e.kind == GeocodingFailure::Timeout),
                "{completion:?} must keep the provider failure: {error:?}"
            );
        }
        assert_eq!(
            ResolveOutcome::Deferred,
            ResolveOutcome::after_completion(
                CompletionOutcome::LeaseLost,
                Ok(ResolutionEvidence {
                    status: ResolutionStatus::NoMatch,
                    candidates: vec![],
                    accepted_index: None,
                }),
            )
            .unwrap()
        );
    }

    fn place() -> PlaceReference {
        PlaceReference::new("synthetic-place").unwrap()
    }

    fn selected_evidence(
        input: &ResolutionInput,
        candidate: GeocodingCandidate,
    ) -> ResolutionEvidence {
        evaluate_selected_place(
            input,
            GeocodingResult {
                outcome: GeocodingOutcome::Candidates(
                    CandidateSet::new(vec![candidate], 5).unwrap(),
                ),
                retention: ResultRetention::Storage,
            },
            "TEST_V1",
        )
        .unwrap()
    }

    #[test]
    fn should_keep_free_text_fingerprints_and_separate_selections_when_a_place_is_selected() {
        let free_text = user_input();
        let unchanged = ResolutionInput::new(
            free_text.request.clone(),
            free_text.scope,
            free_text.purpose,
        )
        .unwrap();
        assert_eq!(
            free_text.fingerprint("TEST_V1"),
            unchanged.fingerprint("TEST_V1")
        );
        let selected = free_text.clone().with_selected_place(place());
        let other = free_text
            .clone()
            .with_selected_place(PlaceReference::new("other-place").unwrap());
        assert_eq!(SELECTED_PLACE_POLICY_VERSION, selected.policy_version());
        assert_eq!(
            USER_BROWSING_POLICY_VERSION,
            selected.free_text_policy_version()
        );
        assert_ne!(
            free_text.fingerprint("TEST_V1"),
            selected.fingerprint("TEST_V1")
        );
        assert_ne!(
            selected.fingerprint("TEST_V1"),
            other.fingerprint("TEST_V1")
        );
    }

    #[test]
    fn should_accept_a_picked_point_only_at_premises_precision_for_every_purpose() {
        for input in [
            user_input().with_selected_place(place()),
            input("123 Street", Uuid::now_v7()).with_selected_place(place()),
        ] {
            for precision in [
                None,
                Some(SpatialPrecision::Premises),
                Some(SpatialPrecision::Street),
                Some(SpatialPrecision::Locality),
            ] {
                let e = selected_evidence(&input, candidate(CountryCode::DEU, precision));
                assert_eq!(ResolutionStatus::Accepted, e.status, "{precision:?}");
                let accepted = &e.candidates[e.accepted_index.unwrap()].geography;
                assert_eq!(
                    precision == Some(SpatialPrecision::Premises),
                    accepted.position().is_some(),
                    "{precision:?}"
                );
                assert_eq!(
                    Some(CountryCode::DEU),
                    accepted
                        .description()
                        .and_then(GeographicDescription::country)
                );
            }
        }
    }

    #[test]
    fn should_leave_a_pick_ambiguous_when_it_contradicts_the_submitted_country() {
        let mut input = input("123 Street", Uuid::now_v7()).with_selected_place(place());
        input.request = input
            .request
            .clone()
            .with_submitted(SubmittedGeography::new(
                GeographicDescription::new(None, Some(CountryCode::FRA), None, None)
                    .unwrap()
                    .unwrap(),
            ));
        let e = selected_evidence(
            &input,
            candidate(CountryCode::DEU, Some(SpatialPrecision::Premises)),
        );
        assert_eq!(ResolutionStatus::Ambiguous, e.status);
        assert!(e.accepted_index.is_none());
    }

    #[test]
    fn should_reject_a_lookup_that_returns_no_place_or_several_places() {
        let input = user_input().with_selected_place(place());
        for outcome in [
            GeocodingOutcome::NoMatch,
            GeocodingOutcome::Candidates(
                CandidateSet::new(
                    vec![
                        candidate(CountryCode::DEU, None),
                        candidate(CountryCode::DEU, None),
                    ],
                    5,
                )
                .unwrap(),
            ),
        ] {
            let error = evaluate_selected_place(
                &input,
                GeocodingResult {
                    outcome,
                    retention: ResultRetention::Storage,
                },
                "TEST_V1",
            )
            .unwrap_err();
            assert_eq!(GeocodingFailure::InvalidResponse, error.kind);
        }
    }

    fn view(input_revision: u64, selected_place: Option<PlaceReference>) -> ResolutionView {
        ResolutionView {
            scope: ResolutionScope::new(ScopeKind::User, Some(Uuid::now_v7())).unwrap(),
            work: ResolutionWork {
                id: ResolutionId::new(),
                generation: 1,
            },
            input_revision,
            guard: 1,
            status: ResolutionStatus::Pending,
            evidence: None,
            selected_candidate: None,
            last_failure: None,
            attempts: 0,
            selected_place,
        }
    }

    #[test]
    fn should_keep_a_pick_only_while_the_input_revision_is_unchanged_when_rebinding() {
        let binding = ResolutionBinding {
            subject: ResolutionSubject::new(RetentionPurpose::UserLocation, Uuid::now_v7())
                .unwrap(),
            input_revision: 2,
            guard: 3,
        };
        let picked = |input: ResolutionInput| input.selected_place().cloned();
        let current = view(2, Some(place()));
        assert_eq!(
            Some(place()),
            picked(carry_selected_place(
                user_input(),
                &binding,
                Some(&current),
                None
            ))
        );
        let changed_address = view(1, Some(place()));
        assert_eq!(
            None,
            picked(carry_selected_place(
                user_input(),
                &binding,
                Some(&changed_address),
                None
            ))
        );
        let newly_picked = PlaceReference::new("new-place").unwrap();
        assert_eq!(
            Some(newly_picked.clone()),
            picked(carry_selected_place(
                user_input(),
                &binding,
                Some(&current),
                Some(newly_picked)
            ))
        );
        assert_eq!(
            None,
            picked(carry_selected_place(user_input(), &binding, None, None))
        );
    }

    #[test]
    fn should_treat_only_a_different_place_as_a_new_pick() {
        let current = view(2, Some(place()));
        assert_eq!(None, newly_picked(Some(place()), Some(&current)));
        let other = PlaceReference::new("other-place").unwrap();
        assert_eq!(
            Some(other.clone()),
            newly_picked(Some(other), Some(&current))
        );
        assert_eq!(
            Some(place()),
            newly_picked(Some(place()), Some(&view(2, None)))
        );
        assert_eq!(Some(place()), newly_picked(Some(place()), None));
        assert_eq!(None, newly_picked(None, Some(&current)));
    }
}
