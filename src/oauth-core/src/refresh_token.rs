//! Rotating refresh tokens of public clients identified by a metadata document.
//! See `docs/oauth.md` for the lifetimes, rotation and reuse rules.

use crate::resource::AuraResource;
use credential_core::oauth_connection_id::OAuthConnectionId;
use credential_core::scope::Scope;
use domain_primitives::uuid_v7_newtype;
use std::collections::HashSet;
use time::{Duration, OffsetDateTime};
use user_core::access_token::{HashedRawToken, RawToken, TokenPrefix};

#[derive(Debug, Clone, PartialEq)]
pub struct RefreshTokenPrefix;

impl TokenPrefix for RefreshTokenPrefix {
    const PREFIX: &'static str = "aurahistoria_refreshtoken";
}

pub type RawRefreshToken = RawToken<RefreshTokenPrefix>;
pub type HashedRawRefreshToken = HashedRawToken<RefreshTokenPrefix>;

uuid_v7_newtype!(RefreshTokenFamilyId);

impl RefreshTokenFamilyId {
    pub fn into_uuid(self) -> uuid::Uuid {
        self.0
    }
}

/// The scope a client adds to an authorization request to ask for a refresh token. It is a
/// request flag, not a credential scope: tokens never carry it.
pub const OFFLINE_ACCESS_SCOPE: &str = "offline_access";

/// A connection keeps at most this many sign-ins refreshable; a new sign-in ends the least
/// recently used family beyond it, so abandoned families of re-signed-in installations cannot
/// pile up.
pub const MAX_REFRESH_TOKEN_FAMILIES_PER_CONNECTION: usize = 5;

/// A token replaced within this window is treated as the loser of concurrent refreshes with
/// the same token, not as reuse: the request fails, the family stays intact.
pub const REFRESH_TOKEN_REUSE_GRACE: Duration = Duration::seconds(10);

/// Lifetimes of the credentials a metadata document client receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetadataDocumentClientTokenLifetimes {
    access_token: Duration,
    refresh_token_idle: Duration,
    refresh_token_absolute: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidTokenLifetimes {
    #[error("token lifetimes must be positive")]
    NotPositive,
    #[error("the refresh-token idle lifetime must not exceed its absolute lifetime")]
    IdleExceedsAbsolute,
}

impl MetadataDocumentClientTokenLifetimes {
    pub const DEFAULT_ACCESS_TOKEN: Duration = Duration::hours(1);
    pub const DEFAULT_REFRESH_TOKEN_IDLE: Duration = Duration::days(30);
    pub const DEFAULT_REFRESH_TOKEN_ABSOLUTE: Duration = Duration::days(90);

    pub fn new(
        access_token: Duration,
        refresh_token_idle: Duration,
        refresh_token_absolute: Duration,
    ) -> Result<Self, InvalidTokenLifetimes> {
        if !access_token.is_positive()
            || !refresh_token_idle.is_positive()
            || !refresh_token_absolute.is_positive()
        {
            return Err(InvalidTokenLifetimes::NotPositive);
        }
        if refresh_token_idle > refresh_token_absolute {
            return Err(InvalidTokenLifetimes::IdleExceedsAbsolute);
        }
        Ok(Self {
            access_token,
            refresh_token_idle,
            refresh_token_absolute,
        })
    }

    pub fn access_token(&self) -> Duration {
        self.access_token
    }

    pub fn refresh_token_idle(&self) -> Duration {
        self.refresh_token_idle
    }

    pub fn refresh_token_absolute(&self) -> Duration {
        self.refresh_token_absolute
    }
}

impl Default for MetadataDocumentClientTokenLifetimes {
    fn default() -> Self {
        Self {
            access_token: Self::DEFAULT_ACCESS_TOKEN,
            refresh_token_idle: Self::DEFAULT_REFRESH_TOKEN_IDLE,
            refresh_token_absolute: Self::DEFAULT_REFRESH_TOKEN_ABSOLUTE,
        }
    }
}

/// The chain of refresh tokens one sign-in starts. Exactly one token of the chain is current;
/// each refresh replaces it. The family ends when it is idle or reaches its absolute limit.
#[derive(Debug, Clone, PartialEq)]
pub struct RefreshTokenFamily {
    id: RefreshTokenFamilyId,
    connection_id: OAuthConnectionId,
    scopes: HashSet<Scope>,
    resource: AuraResource,
    created: OffsetDateTime,
    absolute_expires: OffsetDateTime,
    current: IssuedRefreshToken,
}

/// The family's current token: the only one a refresh accepts.
#[derive(Debug, Clone, PartialEq)]
pub struct IssuedRefreshToken {
    pub hashed_token: HashedRawRefreshToken,
    pub issued: OffsetDateTime,
    /// The idle limit after `issued`, capped by the family's absolute limit.
    pub expires: OffsetDateTime,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedRefreshTokenFamilyState {
    pub id: RefreshTokenFamilyId,
    pub connection_id: OAuthConnectionId,
    pub scopes: HashSet<Scope>,
    pub resource: AuraResource,
    pub created: OffsetDateTime,
    pub absolute_expires: OffsetDateTime,
    pub current: IssuedRefreshToken,
}

/// The state of the token a client presented, within its family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentedRefreshToken {
    Current,
    Replaced { at: OffsetDateTime },
}

/// What a refresh with a presented token must do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenAssessment {
    /// Issue new credentials and replace the current token.
    Rotate,
    /// The family is past its idle or absolute limit.
    Expired,
    /// The token was replaced moments ago by a concurrent refresh with the same token.
    ConcurrentRefresh,
    /// An already replaced token came back: someone else may hold the family.
    Reused,
}

/// A rotation's result: the token handed to the client and the one it replaces.
#[derive(Debug, Clone, PartialEq)]
pub struct RefreshTokenRotation {
    pub raw_token: RawRefreshToken,
    pub replaced: HashedRawRefreshToken,
}

impl RefreshTokenFamily {
    /// Starts the family of one sign-in with its first token.
    pub fn start(
        connection_id: OAuthConnectionId,
        scopes: HashSet<Scope>,
        resource: AuraResource,
        lifetimes: &MetadataDocumentClientTokenLifetimes,
        now: OffsetDateTime,
    ) -> (Self, RawRefreshToken) {
        let absolute_expires = now + lifetimes.refresh_token_absolute();
        let raw_token = RawRefreshToken::new();
        let family = Self {
            id: RefreshTokenFamilyId::new(),
            connection_id,
            scopes,
            resource,
            created: now,
            absolute_expires,
            current: issue(&raw_token, now, absolute_expires, lifetimes),
        };
        (family, raw_token)
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedRefreshTokenFamilyState) -> Self {
        Self {
            id: state.id,
            connection_id: state.connection_id,
            scopes: state.scopes,
            resource: state.resource,
            created: state.created,
            absolute_expires: state.absolute_expires,
            current: state.current,
        }
    }

    /// Decides a refresh with `presented`. A replaced token is judged before expiry, so a
    /// reused token of an expired family still revokes what the family issued.
    pub fn assess(
        &self,
        presented: PresentedRefreshToken,
        now: OffsetDateTime,
    ) -> RefreshTokenAssessment {
        match presented {
            PresentedRefreshToken::Replaced { at } if now - at <= REFRESH_TOKEN_REUSE_GRACE => {
                RefreshTokenAssessment::ConcurrentRefresh
            }
            PresentedRefreshToken::Replaced { .. } => RefreshTokenAssessment::Reused,
            PresentedRefreshToken::Current if self.is_expired_at(now) => {
                RefreshTokenAssessment::Expired
            }
            PresentedRefreshToken::Current => RefreshTokenAssessment::Rotate,
        }
    }

    /// Replaces the current token. The new token's idle limit starts now; the family's
    /// absolute limit never moves.
    pub fn rotate(
        &mut self,
        lifetimes: &MetadataDocumentClientTokenLifetimes,
        now: OffsetDateTime,
    ) -> RefreshTokenRotation {
        let raw_token = RawRefreshToken::new();
        let next = issue(&raw_token, now, self.absolute_expires, lifetimes);
        let replaced = std::mem::replace(&mut self.current, next).hashed_token;
        RefreshTokenRotation {
            raw_token,
            replaced,
        }
    }

    pub fn is_expired_at(&self, now: OffsetDateTime) -> bool {
        self.current.expires < now
    }

    pub fn id(&self) -> RefreshTokenFamilyId {
        self.id
    }

    pub fn connection_id(&self) -> OAuthConnectionId {
        self.connection_id
    }

    /// The scopes of the sign-in that started the family. Each refresh issues them, or a
    /// requested subset.
    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }

    /// The resource the sign-in's authorization code was bound to.
    pub fn resource(&self) -> AuraResource {
        self.resource
    }

    pub fn created(&self) -> OffsetDateTime {
        self.created
    }

    pub fn absolute_expires(&self) -> OffsetDateTime {
        self.absolute_expires
    }

    pub fn current(&self) -> &IssuedRefreshToken {
        &self.current
    }
}

fn issue(
    raw_token: &RawRefreshToken,
    now: OffsetDateTime,
    absolute_expires: OffsetDateTime,
    lifetimes: &MetadataDocumentClientTokenLifetimes,
) -> IssuedRefreshToken {
    IssuedRefreshToken {
        hashed_token: raw_token.clone().into(),
        issued: now,
        expires: (now + lifetimes.refresh_token_idle()).min(absolute_expires),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lifetimes() -> MetadataDocumentClientTokenLifetimes {
        MetadataDocumentClientTokenLifetimes::new(
            Duration::hours(1),
            Duration::days(30),
            Duration::days(90),
        )
        .unwrap()
    }

    fn family(now: OffsetDateTime) -> (RefreshTokenFamily, RawRefreshToken) {
        RefreshTokenFamily::start(
            OAuthConnectionId::new(),
            HashSet::from([Scope::UsersRead]),
            AuraResource::ProductionMcp,
            &lifetimes(),
            now,
        )
    }

    #[test]
    fn should_reject_nonpositive_or_inverted_lifetimes() {
        assert_eq!(
            Err(InvalidTokenLifetimes::NotPositive),
            MetadataDocumentClientTokenLifetimes::new(
                Duration::ZERO,
                Duration::days(1),
                Duration::days(2)
            )
        );
        assert_eq!(
            Err(InvalidTokenLifetimes::IdleExceedsAbsolute),
            MetadataDocumentClientTokenLifetimes::new(
                Duration::hours(1),
                Duration::days(3),
                Duration::days(2)
            )
        );
        assert_eq!(
            MetadataDocumentClientTokenLifetimes::default(),
            lifetimes(),
            "defaults are one hour, 30 and 90 days"
        );
    }

    #[test]
    fn should_start_a_family_whose_token_matches_the_raw_token() {
        let now = OffsetDateTime::now_utc();
        let (family, raw_token) = family(now);

        assert!(raw_token.check(&family.current().hashed_token));
        assert!(String::from(raw_token).starts_with("aurahistoria_refreshtoken_"));
        assert_eq!(now + Duration::days(30), family.current().expires);
        assert_eq!(now + Duration::days(90), family.absolute_expires());
    }

    #[test]
    fn should_rotate_the_current_token_and_restart_its_idle_limit() {
        let now = OffsetDateTime::now_utc();
        let (mut family, first) = family(now);
        let later = now + Duration::days(20);

        assert_eq!(
            RefreshTokenAssessment::Rotate,
            family.assess(PresentedRefreshToken::Current, later)
        );
        let rotation = family.rotate(&lifetimes(), later);

        assert_eq!(HashedRawRefreshToken::from(first), rotation.replaced);
        assert!(rotation.raw_token.check(&family.current().hashed_token));
        assert_eq!(later, family.current().issued);
        assert_eq!(later + Duration::days(30), family.current().expires);
        assert_eq!(now, family.created());
    }

    #[test]
    fn should_expire_when_idle() {
        let now = OffsetDateTime::now_utc();
        let (family, _) = family(now);

        assert_eq!(
            RefreshTokenAssessment::Rotate,
            family.assess(PresentedRefreshToken::Current, now + Duration::days(30))
        );
        assert_eq!(
            RefreshTokenAssessment::Expired,
            family.assess(
                PresentedRefreshToken::Current,
                now + Duration::days(30) + Duration::seconds(1)
            )
        );
    }

    #[test]
    fn should_cap_every_rotation_at_the_absolute_limit() {
        let now = OffsetDateTime::now_utc();
        let (mut family, _) = family(now);
        let mut at = now;
        while at + Duration::days(25) < now + Duration::days(90) {
            at += Duration::days(25);
            assert_eq!(
                RefreshTokenAssessment::Rotate,
                family.assess(PresentedRefreshToken::Current, at)
            );
            family.rotate(&lifetimes(), at);
        }

        assert_eq!(now + Duration::days(90), family.current().expires);
        assert_eq!(
            RefreshTokenAssessment::Expired,
            family.assess(
                PresentedRefreshToken::Current,
                now + Duration::days(90) + Duration::seconds(1)
            )
        );
    }

    #[test]
    fn should_treat_a_just_replaced_token_as_a_concurrent_refresh_and_later_as_reuse() {
        let now = OffsetDateTime::now_utc();
        let (family, _) = family(now);
        let replaced = PresentedRefreshToken::Replaced { at: now };

        assert_eq!(
            RefreshTokenAssessment::ConcurrentRefresh,
            family.assess(replaced, now + REFRESH_TOKEN_REUSE_GRACE)
        );
        assert_eq!(
            RefreshTokenAssessment::Reused,
            family.assess(
                replaced,
                now + REFRESH_TOKEN_REUSE_GRACE + Duration::seconds(1)
            )
        );
        assert_eq!(
            RefreshTokenAssessment::Reused,
            family.assess(replaced, now + Duration::days(365)),
            "reuse is judged before expiry"
        );
    }
}
