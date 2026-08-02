use crate::scope_record::ScopeRecord;
use oauth_core::third_party_exchange_code::{ThirdPartyExchangeCode, ThirdPartyExchangeCodeGrant};
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::RawAccessToken;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct ThirdPartyExchangeCodeRecord {
    pub pk: String,
    pub sk: String,
    pub code: ThirdPartyExchangeCode,
    #[serde(with = "raw_access_token_serde")]
    pub access_token: RawAccessToken,
    pub access_token_expires: Option<i64>,
    pub scopes: HashSet<ScopeRecord>,
    pub expires: i64,
    pub ttl: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

pub fn mk_pk(code: &ThirdPartyExchangeCode) -> String {
    format!("oauth_third_party_exchange_code#{code}")
}

pub fn mk_sk() -> &'static str {
    "oauth_third_party_exchange_code"
}

impl From<ThirdPartyExchangeCodeGrant> for ThirdPartyExchangeCodeRecord {
    fn from(grant: ThirdPartyExchangeCodeGrant) -> Self {
        Self {
            pk: mk_pk(&grant.code),
            sk: mk_sk().to_owned(),
            code: grant.code,
            access_token: grant.access_token,
            access_token_expires: grant
                .access_token_expires
                .map(|expires| expires.unix_timestamp()),
            scopes: grant.scopes.into_iter().map(Into::into).collect(),
            expires: grant.expires.unix_timestamp(),
            ttl: grant.expires.unix_timestamp(),
            created: grant.created,
        }
    }
}

impl From<ThirdPartyExchangeCodeRecord> for ThirdPartyExchangeCodeGrant {
    fn from(record: ThirdPartyExchangeCodeRecord) -> Self {
        Self {
            code: record.code,
            access_token: record.access_token,
            access_token_expires: record
                .access_token_expires
                .and_then(|expires| OffsetDateTime::from_unix_timestamp(expires).ok()),
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            expires: OffsetDateTime::from_unix_timestamp(record.expires)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
            created: record.created,
        }
    }
}

mod raw_access_token_serde {
    use super::*;
    use serde::{Deserializer, Serializer, de::Error as _};

    pub fn serialize<S>(access_token: &RawAccessToken, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&String::from(access_token.clone()))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<RawAccessToken, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        RawAccessToken::try_from(value).map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use time::OffsetDateTime;
    use user_core::access_token::Scope;

    #[test]
    fn should_round_trip_through_serde_dynamo() {
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let code = ThirdPartyExchangeCode::new();
        let access_token = RawAccessToken::new();
        let record = ThirdPartyExchangeCodeRecord {
            pk: format!("oauth_third_party_exchange_code#{}", code),
            sk: "oauth_third_party_exchange_code".to_owned(),
            code,
            access_token,
            access_token_expires: Some(now.unix_timestamp() + 3_600),
            scopes: HashSet::from([ScopeRecord::ProductsWrite]),
            expires: now.unix_timestamp() + 60,
            ttl: now.unix_timestamp() + 60,
            created: now,
        };

        let item: serde_dynamo::Item = serde_dynamo::to_item(record.clone()).unwrap();

        let back: ThirdPartyExchangeCodeRecord = serde_dynamo::from_item(item).unwrap();

        assert_eq!(record.code, back.code, "code mismatch");
        assert_eq!(record.access_token, back.access_token, "token mismatch");
        assert_eq!(record.scopes, back.scopes, "scopes mismatch");
        assert_eq!(
            record.access_token_expires, back.access_token_expires,
            "access_token_expires mismatch"
        );
        assert_eq!(record.expires, back.expires, "expires mismatch");
        assert_eq!(record.ttl, back.ttl, "ttl mismatch");
    }

    #[test]
    fn should_convert_between_domain_and_record() {
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let grant = ThirdPartyExchangeCodeGrant {
            code: ThirdPartyExchangeCode::new(),
            access_token: RawAccessToken::new(),
            access_token_expires: Some(now + time::Duration::hours(1)),
            scopes: HashSet::from([Scope::ProductsWrite]),
            expires: now + time::Duration::seconds(60),
            created: now,
        };

        let actual =
            ThirdPartyExchangeCodeGrant::from(ThirdPartyExchangeCodeRecord::from(grant.clone()));

        assert_eq!(grant, actual);
    }
}
