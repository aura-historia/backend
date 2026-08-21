use crate::missing_field::MissingPersistenceField;
use credential_core::oauth_client_id::OAuthClientId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::{
    AccessToken, AccessTokenId, AccessTokenOrigin, HashedRawAccessToken,
    RehydratedAccessTokenState, Scope,
};
use user_core::user_id::UserId;

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScopeRecord {
    ProductsWrite,
    ShopsRead,
    ShopsWrite,
    PartnerShopApplicationsWrite,
    PartnerShopsRead,
    PartnerShopsWrite,
    UsersRead,
    UsersWrite,
    AccessTokensRead,
    AccessTokensWrite,
    SearchFiltersWrite,
    WatchlistRead,
    WatchlistWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccessTokenOriginRecord {
    #[default]
    User,
    OAuth,
}

#[derive(Debug, thiserror::Error)]
pub enum AccessTokenRecordMappingError {
    #[error(transparent)]
    MissingField(#[from] MissingPersistenceField),
    #[error("invalid access token expiry timestamp '{timestamp}'")]
    InvalidExpiresTimestamp {
        timestamp: i64,
        #[source]
        source: time::error::ComponentRange,
    },
}

impl From<Scope> for ScopeRecord {
    fn from(value: Scope) -> Self {
        match value {
            Scope::ProductsWrite => ScopeRecord::ProductsWrite,
            Scope::ShopsRead => ScopeRecord::ShopsRead,
            Scope::ShopsWrite => ScopeRecord::ShopsWrite,
            Scope::PartnerShopApplicationsWrite => ScopeRecord::PartnerShopApplicationsWrite,
            Scope::PartnerShopsRead => ScopeRecord::PartnerShopsRead,
            Scope::PartnerShopsWrite => ScopeRecord::PartnerShopsWrite,
            Scope::UsersRead => ScopeRecord::UsersRead,
            Scope::UsersWrite => ScopeRecord::UsersWrite,
            Scope::AccessTokensRead => ScopeRecord::AccessTokensRead,
            Scope::AccessTokensWrite => ScopeRecord::AccessTokensWrite,
            Scope::SearchFiltersWrite => ScopeRecord::SearchFiltersWrite,
            Scope::WatchlistRead => ScopeRecord::WatchlistRead,
            Scope::WatchlistWrite => ScopeRecord::WatchlistWrite,
        }
    }
}

impl From<ScopeRecord> for Scope {
    fn from(value: ScopeRecord) -> Self {
        match value {
            ScopeRecord::ProductsWrite => Scope::ProductsWrite,
            ScopeRecord::ShopsRead => Scope::ShopsRead,
            ScopeRecord::ShopsWrite => Scope::ShopsWrite,
            ScopeRecord::PartnerShopApplicationsWrite => Scope::PartnerShopApplicationsWrite,
            ScopeRecord::PartnerShopsRead => Scope::PartnerShopsRead,
            ScopeRecord::PartnerShopsWrite => Scope::PartnerShopsWrite,
            ScopeRecord::UsersRead => Scope::UsersRead,
            ScopeRecord::UsersWrite => Scope::UsersWrite,
            ScopeRecord::AccessTokensRead => Scope::AccessTokensRead,
            ScopeRecord::AccessTokensWrite => Scope::AccessTokensWrite,
            ScopeRecord::SearchFiltersWrite => Scope::SearchFiltersWrite,
            ScopeRecord::WatchlistRead => Scope::WatchlistRead,
            ScopeRecord::WatchlistWrite => Scope::WatchlistWrite,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct AccessTokenRecord {
    pub pk: String,
    pub sk: String,
    pub access_token_id: AccessTokenId,
    pub user_id: UserId,
    pub name: String,
    pub scopes: HashSet<ScopeRecord>,
    pub token_prefix: String,
    pub token_short: String,
    pub token_hash: String,

    #[serde(default)]
    pub origin: AccessTokenOriginRecord,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth_client_id: Option<OAuthClientId>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl: Option<i64>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_pk: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gsi1_sk: Option<String>,

    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

pub fn mk_pk(user_id: &UserId) -> String {
    format!("user#{user_id}")
}

pub fn mk_sk(access_token_id: &AccessTokenId) -> String {
    format!("access_token#{access_token_id}")
}

pub fn mk_gsi1_pk(hashed_token: &HashedRawAccessToken) -> String {
    format!(
        "access_token#{}#{}",
        hashed_token.prefix(),
        hashed_token.short_token()
    )
}

pub fn mk_gsi1_sk(user_id: &UserId, access_token_id: &AccessTokenId) -> String {
    format!("user#{user_id}#access_token#{access_token_id}")
}

impl AccessTokenRecord {
    pub(crate) fn matches_hash(&self, hashed_token: &HashedRawAccessToken) -> bool {
        self.token_prefix == hashed_token.prefix()
            && self.token_short == hashed_token.short_token()
            && self.token_hash == hashed_token.long_token_hash()
    }

    pub(crate) fn from_access_token(
        access_token: &AccessToken,
        created: OffsetDateTime,
        updated: OffsetDateTime,
    ) -> Self {
        let expires = access_token
            .expires()
            .map(|expires| expires.unix_timestamp());
        let (origin, oauth_client_id) = match access_token.origin() {
            AccessTokenOrigin::User => (AccessTokenOriginRecord::User, None),
            AccessTokenOrigin::OAuth { client_id } => {
                (AccessTokenOriginRecord::OAuth, Some(client_id))
            }
        };
        AccessTokenRecord {
            pk: mk_pk(&access_token.user_id()),
            sk: mk_sk(&access_token.id()),
            access_token_id: access_token.id(),
            user_id: access_token.user_id(),
            name: access_token.name().clone().into(),
            scopes: access_token
                .scopes()
                .iter()
                .copied()
                .map(Into::into)
                .collect(),
            token_prefix: access_token.hashed_token().prefix().to_owned(),
            token_short: access_token.hashed_token().short_token().to_owned(),
            token_hash: access_token.hashed_token().long_token_hash().to_owned(),
            origin,
            oauth_client_id: oauth_client_id.copied(),
            expires,
            ttl: expires,
            gsi1_pk: Some(mk_gsi1_pk(access_token.hashed_token())),
            gsi1_sk: Some(mk_gsi1_sk(&access_token.user_id(), &access_token.id())),
            created,
            updated,
        }
    }
}

impl TryFrom<AccessTokenRecord> for AccessToken {
    type Error = AccessTokenRecordMappingError;

    fn try_from(record: AccessTokenRecord) -> Result<Self, Self::Error> {
        let origin = match record.origin {
            AccessTokenOriginRecord::User => AccessTokenOrigin::User,
            AccessTokenOriginRecord::OAuth => AccessTokenOrigin::OAuth {
                client_id: record
                    .oauth_client_id
                    .ok_or_else(|| MissingPersistenceField::new("oauth_client_id"))?,
            },
        };
        let expires = record
            .expires
            .map(|timestamp| {
                OffsetDateTime::from_unix_timestamp(timestamp).map_err(|source| {
                    AccessTokenRecordMappingError::InvalidExpiresTimestamp { timestamp, source }
                })
            })
            .transpose()?;

        Ok(AccessToken::rehydrate(RehydratedAccessTokenState {
            id: record.access_token_id,
            hashed_token: HashedRawAccessToken::new(record.token_short, record.token_hash),
            user_id: record.user_id,
            name: record.name.into(),
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            origin,
            expires,
        }))
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Fake, Faker, RngExt};

    impl fake::Dummy<Faker> for AccessTokenRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            let access_token = config.fake_with_rng::<AccessToken, R>(rng);
            let now = OffsetDateTime::now_utc();
            AccessTokenRecord::from_access_token(&access_token, now, now)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use user_core::access_token::{AccessTokenName, NewAccessToken, RawAccessToken};

    fn assert_ok<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected ok, got {error:?}"),
        }
    }

    fn now() -> OffsetDateTime {
        assert_ok(OffsetDateTime::from_unix_timestamp(
            OffsetDateTime::now_utc().unix_timestamp(),
        ))
    }

    fn record_for(token: &AccessToken) -> AccessTokenRecord {
        let now = now();
        AccessTokenRecord::from_access_token(token, now, now)
    }

    fn token() -> AccessToken {
        let raw = RawAccessToken::new();
        let now = now();
        AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw.into(),
            user_id: UserId::new(),
            name: AccessTokenName::from("token"),
            scopes: HashSet::from([Scope::ProductsWrite, Scope::ShopsWrite]),
            origin: AccessTokenOrigin::User,
            expires: Some(now + time::Duration::days(1)),
        })
    }

    #[test]
    fn should_build_keys() {
        let token = token();

        assert_eq!(format!("user#{}", token.user_id()), mk_pk(&token.user_id()));
        assert_eq!(format!("access_token#{}", token.id()), mk_sk(&token.id()));
        assert_eq!(
            format!(
                "access_token#{}#{}",
                token.hashed_token().prefix(),
                token.hashed_token().short_token()
            ),
            mk_gsi1_pk(token.hashed_token())
        );
        assert_eq!(
            format!("user#{}#access_token#{}", token.user_id(), token.id()),
            mk_gsi1_sk(&token.user_id(), &token.id())
        );
    }

    #[test]
    fn should_map_all_scope_records() {
        for (scope, record) in [
            (Scope::ProductsWrite, ScopeRecord::ProductsWrite),
            (Scope::ShopsRead, ScopeRecord::ShopsRead),
            (Scope::ShopsWrite, ScopeRecord::ShopsWrite),
            (
                Scope::PartnerShopApplicationsWrite,
                ScopeRecord::PartnerShopApplicationsWrite,
            ),
            (Scope::PartnerShopsRead, ScopeRecord::PartnerShopsRead),
            (Scope::PartnerShopsWrite, ScopeRecord::PartnerShopsWrite),
            (Scope::UsersRead, ScopeRecord::UsersRead),
            (Scope::UsersWrite, ScopeRecord::UsersWrite),
            (Scope::AccessTokensRead, ScopeRecord::AccessTokensRead),
            (Scope::AccessTokensWrite, ScopeRecord::AccessTokensWrite),
            (Scope::SearchFiltersWrite, ScopeRecord::SearchFiltersWrite),
            (Scope::WatchlistRead, ScopeRecord::WatchlistRead),
            (Scope::WatchlistWrite, ScopeRecord::WatchlistWrite),
        ] {
            assert_eq!(record, ScopeRecord::from(scope));
            assert_eq!(scope, Scope::from(record));
        }
    }

    #[test]
    fn should_map_access_token_to_record_and_back() {
        let token = token();
        let record = record_for(&token);

        assert_eq!(mk_pk(&token.user_id()), record.pk);
        assert_eq!(mk_sk(&token.id()), record.sk);
        assert_eq!(Some(mk_gsi1_pk(token.hashed_token())), record.gsi1_pk);
        assert_eq!(
            Some(mk_gsi1_sk(&token.user_id(), &token.id())),
            record.gsi1_sk
        );
        assert_eq!(
            token.expires().map(|expires| expires.unix_timestamp()),
            record.ttl
        );
        assert!(record.matches_hash(token.hashed_token()));

        let mapped = assert_ok(AccessToken::try_from(record));
        assert_eq!(token, mapped);
    }

    #[test]
    fn should_map_oauth_origin() {
        let client_id = OAuthClientId::new();
        let raw = RawAccessToken::new();
        let token = AccessToken::create(NewAccessToken {
            id: AccessTokenId::new(),
            hashed_token: raw.into(),
            user_id: UserId::new(),
            name: AccessTokenName::from("oauth token"),
            scopes: HashSet::from([Scope::ProductsWrite]),
            origin: AccessTokenOrigin::OAuth { client_id },
            expires: None,
        });

        let record = record_for(&token);

        assert_eq!(AccessTokenOriginRecord::OAuth, record.origin);
        assert_eq!(Some(client_id), record.oauth_client_id);
        assert_eq!(token, assert_ok(AccessToken::try_from(record)));
    }

    #[test]
    fn should_reject_oauth_record_without_client_id() {
        let token = token();
        let mut record = record_for(&token);
        record.origin = AccessTokenOriginRecord::OAuth;
        record.oauth_client_id = None;

        assert!(matches!(
            AccessToken::try_from(record),
            Err(AccessTokenRecordMappingError::MissingField(_))
        ));
    }

    #[test]
    fn should_reject_invalid_expires_timestamp() {
        let token = token();
        let mut record = record_for(&token);
        record.expires = Some(i64::MAX);

        assert!(matches!(
            AccessToken::try_from(record),
            Err(AccessTokenRecordMappingError::InvalidExpiresTimestamp { .. })
        ));
    }

    #[test]
    fn should_match_hash_only_when_full_hash_matches() {
        let token = token();
        let mut record = record_for(&token);
        assert!(record.matches_hash(token.hashed_token()));

        record.token_hash = "different".to_owned();
        assert!(!record.matches_hash(token.hashed_token()));
    }
}
