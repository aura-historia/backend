//! Address suggestions for interactive address entry. Providers forbid storing or caching
//! suggestion results, so nothing here is persisted except the caller's quota use. A picked
//! suggestion's [`PlaceReference`] is redeemed later by durable resolution, never here.
use crate::geocoding::{GeocodingError, GeocodingFailure, PlaceReference};
use application::{
    error::BoxError,
    operation_context::{OperationContext, Principal},
    transaction::{Transaction, TransactionError, UnitOfWork},
};
use localization::Language;
use std::fmt;
use time::{Duration, OffsetDateTime, Time};
use uuid::Uuid;

/// Suggestions returned per query. Fixed so callers cannot raise the billed result size.
pub const MAX_SUGGESTIONS: usize = 5;
/// Suggestion requests one user may make per minute and per UTC day. Approved by the product
/// owner; see [address suggestions](../../../docs/geography.md#address-suggestions).
pub const MAX_PER_MINUTE: u32 = 60;
pub const MAX_PER_DAY: u32 = 600;

/// Partly typed address text: trimmed, 3 to 200 characters, without control characters.
#[derive(Clone, PartialEq, Eq)]
pub struct SuggestionQuery(String);

impl SuggestionQuery {
    pub const MIN_CHARS: usize = 3;
    pub const MAX_CHARS: usize = 200;

    pub fn new(raw: &str) -> Result<Self, InvalidSuggestionQuery> {
        let value = raw.trim();
        let chars = value.chars().count();
        if !(Self::MIN_CHARS..=Self::MAX_CHARS).contains(&chars)
            || value.chars().any(char::is_control)
        {
            return Err(InvalidSuggestionQuery);
        }
        Ok(Self(value.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SuggestionQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SuggestionQuery(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("suggestion query must be 3..=200 characters without control characters")]
pub struct InvalidSuggestionQuery;

/// What a suggestion names, so a form can tell a complete address from a street or town that
/// still needs a house number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionKind {
    Address,
    Street,
    Locality,
    PostalCode,
    Region,
    Other,
}

impl SuggestionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Address => "ADDRESS",
            Self::Street => "STREET",
            Self::Locality => "LOCALITY",
            Self::PostalCode => "POSTAL_CODE",
            Self::Region => "REGION",
            Self::Other => "OTHER",
        }
    }
}

#[derive(Clone)]
pub struct AddressSuggestion {
    pub place: PlaceReference,
    /// Display text in the requested language where the provider has it.
    pub label: String,
    pub kind: SuggestionKind,
}

impl fmt::Debug for AddressSuggestion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AddressSuggestion")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

/// The data attribution shown alongside suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionAttribution {
    pub provider: &'static str,
    pub attribution_url: &'static str,
}

#[derive(Debug, Clone)]
pub struct AddressSuggestions {
    pub suggestions: Vec<AddressSuggestion>,
    pub attribution: SuggestionAttribution,
}

#[async_trait::async_trait]
pub trait AddressSuggestionProvider: Send + Sync {
    /// At most [`MAX_SUGGESTIONS`] display-only suggestions, worldwide.
    async fn suggest(
        &self,
        query: &SuggestionQuery,
        language: Language,
    ) -> Result<AddressSuggestions, GeocodingError>;
}

#[async_trait::async_trait]
impl<P: AddressSuggestionProvider + ?Sized> AddressSuggestionProvider for std::sync::Arc<P> {
    async fn suggest(
        &self,
        query: &SuggestionQuery,
        language: Language,
    ) -> Result<AddressSuggestions, GeocodingError> {
        self.as_ref().suggest(query, language).await
    }
}

/// One user's suggestion requests in the current minute and UTC day windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionUsage {
    pub minute_start: OffsetDateTime,
    pub minute_uses: u32,
    pub day_start: OffsetDateTime,
    pub day_uses: u32,
}

impl SuggestionUsage {
    /// Counts one more request at `now`, or says how long until a window admits one. Refused
    /// requests are not counted, so retrying after a refusal does not extend it.
    pub fn consume(
        previous: Option<Self>,
        now: OffsetDateTime,
    ) -> Result<Self, SuggestionRetryAfter> {
        let now = now.to_offset(time::UtcOffset::UTC);
        let minute_start = now.replace_time(
            Time::from_hms(now.hour(), now.minute(), 0).expect("valid wall-clock minute"),
        );
        let day_start = now.replace_time(Time::MIDNIGHT);
        let (minute_uses, day_uses) = match previous {
            Some(p) => (
                if p.minute_start == minute_start {
                    p.minute_uses
                } else {
                    0
                },
                if p.day_start == day_start {
                    p.day_uses
                } else {
                    0
                },
            ),
            None => (0, 0),
        };
        if day_uses >= MAX_PER_DAY {
            return Err(SuggestionRetryAfter::until(now, day_start + Duration::DAY));
        }
        if minute_uses >= MAX_PER_MINUTE {
            return Err(SuggestionRetryAfter::until(
                now,
                minute_start + Duration::MINUTE,
            ));
        }
        Ok(Self {
            minute_start,
            minute_uses: minute_uses + 1,
            day_start,
            day_uses: day_uses + 1,
        })
    }
}

/// Whole seconds until a rate-limited user can request suggestions again, at least one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SuggestionRetryAfter(u64);

impl SuggestionRetryAfter {
    fn until(now: OffsetDateTime, end: OffsetDateTime) -> Self {
        let remaining = end - now;
        let seconds = remaining.whole_seconds() + i64::from(remaining.subsec_nanoseconds() > 0);
        Self(u64::try_from(seconds).unwrap_or(0).max(1))
    }
    pub const fn seconds(self) -> u64 {
        self.0
    }
}

#[async_trait::async_trait]
pub trait SuggestionUsageRepository: Send {
    /// The user's usage, locked until the transaction ends so concurrent requests count once
    /// each.
    async fn lock(&mut self, user: Uuid) -> Result<Option<SuggestionUsage>, BoxError>;
    async fn save(&mut self, user: Uuid, usage: SuggestionUsage) -> Result<(), BoxError>;
}

pub trait SuggestionUsageRepositoryFactory<Tx>: Send + Sync {
    fn usage<'tx>(&'tx self, tx: &'tx mut Tx) -> impl SuggestionUsageRepository + 'tx;
}

#[derive(Debug, thiserror::Error)]
pub enum SuggestAddressesError {
    #[error("authentication required")]
    AuthenticationRequired,
    #[error("operation not permitted")]
    Forbidden,
    #[error("address suggestion rate limit reached")]
    RateLimited(SuggestionRetryAfter),
    #[error("address suggestion provider is unavailable")]
    Unavailable(#[source] GeocodingError),
    #[error("address suggestion provider failed")]
    Provider(#[source] GeocodingError),
    #[error("address suggestion usage storage failed")]
    Storage(#[source] BoxError),
    #[error("address suggestion transaction failed")]
    Transaction(
        #[from]
        #[source]
        TransactionError,
    ),
}

impl From<GeocodingError> for SuggestAddressesError {
    /// Transient outages let the caller fall back to manual entry; the rest are our faults.
    fn from(error: GeocodingError) -> Self {
        match error.kind {
            GeocodingFailure::Timeout
            | GeocodingFailure::Throttled
            | GeocodingFailure::Unavailable => Self::Unavailable(error),
            _ => Self::Provider(error),
        }
    }
}

#[async_trait::async_trait]
pub trait SuggestAddressesUseCase: Send + Sync {
    async fn execute(
        &self,
        context: &OperationContext,
        query: SuggestionQuery,
        language: Language,
    ) -> Result<AddressSuggestions, SuggestAddressesError>;
}

pub struct SuggestAddressesHandler<U, R, P> {
    uow: U,
    usage: R,
    provider: P,
}

impl<U, R, P> SuggestAddressesHandler<U, R, P> {
    pub fn new(uow: U, usage: R, provider: P) -> Self {
        Self {
            uow,
            usage,
            provider,
        }
    }
}

/// Only signed-in people entering an address themselves; delegated credentials, such as MCP
/// agents, never spend the suggestion budget.
fn requesting_user(principal: &Principal) -> Result<Uuid, SuggestAddressesError> {
    match principal {
        Principal::User(id) => Ok(id.into_uuid()),
        Principal::Anonymous => Err(SuggestAddressesError::AuthenticationRequired),
        _ => Err(SuggestAddressesError::Forbidden),
    }
}

impl<U: UnitOfWork, R: SuggestionUsageRepositoryFactory<U::Tx>, P>
    SuggestAddressesHandler<U, R, P>
{
    async fn consume_quota(
        &self,
        user: Uuid,
        now: OffsetDateTime,
    ) -> Result<(), SuggestAddressesError> {
        let mut tx = self.uow.begin().await?;
        let mut usage = self.usage.usage(&mut tx);
        let previous = usage
            .lock(user)
            .await
            .map_err(SuggestAddressesError::Storage)?;
        let consumed =
            SuggestionUsage::consume(previous, now).map_err(SuggestAddressesError::RateLimited)?;
        usage
            .save(user, consumed)
            .await
            .map_err(SuggestAddressesError::Storage)?;
        drop(usage);
        tx.commit().await?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl<U, R, P> SuggestAddressesUseCase for SuggestAddressesHandler<U, R, P>
where
    U: UnitOfWork,
    R: SuggestionUsageRepositoryFactory<U::Tx>,
    P: AddressSuggestionProvider,
{
    async fn execute(
        &self,
        context: &OperationContext,
        query: SuggestionQuery,
        language: Language,
    ) -> Result<AddressSuggestions, SuggestAddressesError> {
        let user = requesting_user(&context.principal)?;
        // Count before the paid call and commit before it: no transaction spans provider I/O.
        self.consume_quota(user, OffsetDateTime::now_utc()).await?;
        let result = self.provider.suggest(&query, language).await;
        tracing::info!(
            metric = "address_suggestions",
            outcome = match &result {
                Ok(s) if s.suggestions.is_empty() => "EMPTY",
                Ok(_) => "OK",
                Err(e) => e.kind.as_str(),
            }
        );
        let mut suggestions = result?;
        suggestions.suggestions.truncate(MAX_SUGGESTIONS);
        Ok(suggestions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn should_trim_and_bound_the_query_when_constructing_it() {
        assert_eq!(
            "Haupt",
            SuggestionQuery::new("  Haupt \n").unwrap().as_str()
        );
        assert!(SuggestionQuery::new(&"ä".repeat(SuggestionQuery::MAX_CHARS)).is_ok());
        for invalid in [
            "",
            "  ab  ",
            "a\u{0}bc",
            &"x".repeat(SuggestionQuery::MAX_CHARS + 1),
        ] {
            assert_eq!(Err(InvalidSuggestionQuery), SuggestionQuery::new(invalid));
        }
        assert!(
            !format!("{:?}", SuggestionQuery::new("secret street").unwrap()).contains("secret")
        );
    }

    fn usage_after(requests: u32, now: OffsetDateTime) -> Option<SuggestionUsage> {
        (0..requests).try_fold(None, |previous, _| {
            SuggestionUsage::consume(previous, now).ok().map(Some)
        })?
    }

    #[test]
    fn should_refuse_until_the_next_minute_when_the_minute_budget_is_spent() {
        let now = datetime!(2026-11-16 10:15:20.5 UTC);
        let spent = usage_after(MAX_PER_MINUTE, now).unwrap();
        assert_eq!(MAX_PER_MINUTE, spent.minute_uses);
        assert_eq!(
            Err(SuggestionRetryAfter(40)),
            SuggestionUsage::consume(Some(spent), now)
        );
        let next =
            SuggestionUsage::consume(Some(spent), datetime!(2026-11-16 10:16:00 UTC)).unwrap();
        assert_eq!((1, MAX_PER_MINUTE + 1), (next.minute_uses, next.day_uses));
    }

    #[test]
    fn should_refuse_until_utc_midnight_when_the_day_budget_is_spent() {
        let now = datetime!(2026-11-16 23:59:30 UTC);
        let spent = SuggestionUsage {
            minute_start: datetime!(2026-11-16 23:59 UTC),
            minute_uses: 0,
            day_start: datetime!(2026-11-16 0:00 UTC),
            day_uses: MAX_PER_DAY,
        };
        assert_eq!(
            Err(SuggestionRetryAfter(30)),
            SuggestionUsage::consume(Some(spent), now)
        );
        let next =
            SuggestionUsage::consume(Some(spent), datetime!(2026-11-17 0:00:01 UTC)).unwrap();
        assert_eq!((1, 1), (next.minute_uses, next.day_uses));
    }

    #[test]
    fn should_refuse_anonymous_and_non_user_principals_when_requesting_suggestions() {
        assert!(matches!(
            requesting_user(&Principal::Anonymous),
            Err(SuggestAddressesError::AuthenticationRequired)
        ));
        assert!(matches!(
            requesting_user(&Principal::System),
            Err(SuggestAddressesError::Forbidden)
        ));
    }
}
