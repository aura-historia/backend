//! Durable resolution use cases. Inputs, scopes and review authority are supplied by
//! the owning service; adapters own codecs, leases and database uniqueness.
use crate::geocoding::*;
use application::{
    error::{BoxError, FailureKind, static_error},
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use geo::{DerivedGeography, GeographicDescription, SpatialPrecision};
use sha2::{Digest, Sha256};
use std::{fmt, time::Duration};
use time::OffsetDateTime;
use uuid::Uuid;

domain_primitives::object_id_newtype!(ResolutionId, "gres");
pub const POLICY_VERSION: &str = "PRECISION_FIRST_V1";
pub const USER_BROWSING_POLICY_VERSION: &str = "AUTOMATIC_USER_BROWSING_V1";
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
        })
    }
    pub fn request(&self) -> &GeocodingRequest {
        &self.request
    }
    pub fn scope(&self) -> ResolutionScope {
        self.scope
    }
    pub fn purpose(&self) -> RetentionPurpose {
        self.purpose
    }
    /// Browsing addresses accept eligible premises evidence without a review step.
    /// Business-location review retains its independent, existing policy.
    pub fn policy_version(&self) -> &'static str {
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
        hash.finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn budget_key(&self) -> String {
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
pub enum ClaimOutcome {
    Claimed(Box<ResolutionClaim>),
    Complete,
    Deferred,
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
    pub fn retention_period(self) -> Option<time::Duration> {
        match self {
            Self::Accepted | Self::NoMatch => Some(time::Duration::days(30)),
            Self::Ambiguous => Some(time::Duration::days(7)),
            _ => None,
        }
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
    pub valid_until: Option<OffsetDateTime>,
    pub selected_candidate: Option<usize>,
    pub last_failure: Option<String>,
    pub attempts: u32,
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
        let description = candidates[0].geography.description();
        let coarse = description
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
        if let Some(geography) = DerivedGeography::new(coarse, None) {
            // Keep the complete candidate for review; the automatically accepted item
            // contains only administrative geography and retains its provenance.
            let mut accepted = candidates[0].clone();
            accepted.geography = geography;
            accepted.acceptable = true;
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
pub fn candidate_compatible(input: &ResolutionInput, geography: &DerivedGeography) -> bool {
    let d = geography.description();
    let country = d.and_then(|d| d.country());
    let subdivision = d.and_then(|d| d.subdivision());
    let matches = |c: Option<geo::CountryCode>, s: Option<geo::SubdivisionCode>| {
        c.is_none_or(|c| country == Some(c)) && s.is_none_or(|s| subdivision == Some(s))
    };
    let administrative_match = input
        .request
        .submitted()
        .is_none_or(|s| matches(s.description().country(), s.description().subdivision()))
        && input
            .request
            .constraint()
            .is_none_or(|c| matches(c.country(), c.subdivision()));
    administrative_match
        && (input.purpose != RetentionPurpose::UserLocation
            || input.request.submitted().is_none_or(|s| {
                s.description().postal_code().is_none_or(|postal| {
                    d.and_then(|d| d.postal_code()).is_some_and(|derived| {
                        country.map_or_else(
                            || derived == postal,
                            |country| derived.equivalent_in(postal, country),
                        )
                    })
                })
            }))
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
    ) -> Result<bool, ResolutionError>;
    /// Publish fresh fenced wake-ups for runnable work without resetting paid attempts.
    async fn recover_pending(&mut self, limit: u32) -> Result<u32, ResolutionError>;
    /// Purge every expired provider derivative and commit invalidation before requeue.
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveOutcome {
    Complete,
    Deferred,
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
            ClaimOutcome::Complete => return Ok(ResolveOutcome::Complete),
            ClaimOutcome::Deferred => return Ok(ResolveOutcome::Deferred),
            ClaimOutcome::Claimed(c) => c,
        };
        // No database transaction or connection survives across provider I/O.
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            self.provider.geocode(claim.input.request()),
        )
        .await;
        let evidence = match result {
            Ok(Ok(result)) => evaluate(&claim.input, result, &claim.profile),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(GeocodingError {
                kind: GeocodingFailure::Timeout,
                source: Box::new(e),
            }),
        };
        let failure = evidence.as_ref().err().map(|e| e.kind.as_str());
        let status = evidence
            .as_ref()
            .map(|e| e.status.as_str())
            .unwrap_or("PROVIDER_FAILURE");
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
        tracing::info!(
            metric = "geo_resolution_outcome",
            method = "GEOCODER",
            policy = claim.input.policy_version(),
            outcome = if completed { status } else { "UNCOMPLETED" },
            failure,
            attempt = claim.attempt,
            fenced_completion = completed
        );
        if let Err(error) = evidence {
            return Err(ResolutionError::Provider(error));
        }
        Ok(if completed {
            ResolveOutcome::Complete
        } else {
            ResolveOutcome::Deferred
        })
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
        assert_eq!(
            Some(time::Duration::days(30)),
            ResolutionStatus::NoMatch.retention_period()
        );
        assert_eq!(
            Some(time::Duration::days(7)),
            ResolutionStatus::Ambiguous.retention_period()
        );
        assert_eq!(None, ResolutionStatus::Failed.retention_period());
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
}
