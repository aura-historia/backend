use crate::core::access_token::{AccessToken, AccessTokenId, HashedRawAccessToken};
use common::user_id::UserId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct AccessTokenRecord {
    pub pk: String,
    pub sk: String,
    pub access_token_id: AccessTokenId,
    pub user_id: UserId,
    pub name: String,
    pub scopes: HashSet<crate::core::access_token::Scope>,
    pub token_prefix: String,
    pub token_short: String,
    pub token_hash: String,

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
        AccessTokenRecord {
            pk: mk_pk(&access_token.user_id),
            sk: mk_sk(&access_token.id),
            access_token_id: access_token.id,
            user_id: access_token.user_id,
            name: access_token.name.into(),
            scopes: access_token.scopes,
            token_prefix: access_token.hashed_token.prefix().to_owned(),
            token_short: access_token.hashed_token.short_token().to_owned(),
            token_hash: access_token.hashed_token.long_token_hash().to_owned(),
            expires,
            ttl: expires,
            gsi1_pk: Some(mk_gsi1_pk(&access_token.hashed_token)),
            gsi1_sk: Some(mk_gsi1_sk(&access_token.user_id, &access_token.id)),
            created: access_token.created,
            updated: access_token.updated,
        }
    }
}

impl From<AccessTokenRecord> for AccessToken {
    fn from(record: AccessTokenRecord) -> Self {
        AccessToken {
            id: record.access_token_id,
            hashed_token: HashedRawAccessToken::new(record.token_short, record.token_hash),
            user_id: record.user_id,
            name: record.name.into(),
            scopes: record.scopes,
            expires: record
                .expires
                .and_then(|timestamp| OffsetDateTime::from_unix_timestamp(timestamp).ok()),
            created: record.created,
            updated: record.updated,
        }
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
