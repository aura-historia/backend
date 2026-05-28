use crate::core::access_token::{
    AccessToken, AccessTokenId, AccessTokenOrigin, HashedRawAccessToken,
};
use common::{error::missing_field::MissingPersistenceField, user_id::UserId};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScopeRecord {
    ShopsManage,
    ProductsWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AccessTokenOriginRecord {
    #[default]
    User,
    OAuth,
}

impl From<crate::core::access_token::Scope> for ScopeRecord {
    fn from(value: crate::core::access_token::Scope) -> Self {
        match value {
            crate::core::access_token::Scope::ShopsManage => ScopeRecord::ShopsManage,
            crate::core::access_token::Scope::ProductsWrite => ScopeRecord::ProductsWrite,
        }
    }
}

impl From<ScopeRecord> for crate::core::access_token::Scope {
    fn from(value: ScopeRecord) -> Self {
        match value {
            ScopeRecord::ShopsManage => crate::core::access_token::Scope::ShopsManage,
            ScopeRecord::ProductsWrite => crate::core::access_token::Scope::ProductsWrite,
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
    pub oauth_client_id: Option<String>,

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
    crate::dynamodb::user_record::mk_pk(user_id)
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

impl From<AccessToken> for AccessTokenRecord {
    fn from(access_token: AccessToken) -> Self {
        let expires = access_token.expires.map(|expires| expires.unix_timestamp());
        let (origin, oauth_client_id) = match access_token.origin {
            AccessTokenOrigin::User => (AccessTokenOriginRecord::User, None),
            AccessTokenOrigin::OAuth { client_id } => {
                (AccessTokenOriginRecord::OAuth, Some(client_id))
            }
        };
        AccessTokenRecord {
            pk: mk_pk(&access_token.user_id),
            sk: mk_sk(&access_token.id),
            access_token_id: access_token.id,
            user_id: access_token.user_id,
            name: access_token.name.into(),
            scopes: access_token.scopes.into_iter().map(Into::into).collect(),
            token_prefix: access_token.hashed_token.prefix().to_owned(),
            token_short: access_token.hashed_token.short_token().to_owned(),
            token_hash: access_token.hashed_token.long_token_hash().to_owned(),
            origin,
            oauth_client_id,
            expires,
            ttl: expires,
            gsi1_pk: Some(mk_gsi1_pk(&access_token.hashed_token)),
            gsi1_sk: Some(mk_gsi1_sk(&access_token.user_id, &access_token.id)),
            created: access_token.created,
            updated: access_token.updated,
        }
    }
}

impl TryFrom<AccessTokenRecord> for AccessToken {
    type Error = MissingPersistenceField;

    fn try_from(record: AccessTokenRecord) -> Result<Self, Self::Error> {
        let origin = match record.origin {
            AccessTokenOriginRecord::User => AccessTokenOrigin::User,
            AccessTokenOriginRecord::OAuth => AccessTokenOrigin::OAuth {
                client_id: record
                    .oauth_client_id
                    .ok_or_else(|| MissingPersistenceField::new("oauth_client_id"))?,
            },
        };
        Ok(AccessToken {
            id: record.access_token_id,
            hashed_token: HashedRawAccessToken::new(record.token_short, record.token_hash),
            user_id: record.user_id,
            name: record.name.into(),
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            origin,
            expires: record
                .expires
                .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok()),
            created: record.created,
            updated: record.updated,
        })
    }
}

#[cfg(feature = "test-data")]
mod faker {
    use super::*;
    use fake::{Fake, Faker, RngExt};

    impl fake::Dummy<Faker> for AccessTokenRecord {
        fn dummy_with_rng<R: RngExt + ?Sized>(config: &Faker, rng: &mut R) -> Self {
            config.fake_with_rng::<AccessToken, R>(rng).into()
        }
    }
}
