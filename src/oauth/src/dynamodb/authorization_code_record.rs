use crate::core::authorization_code::{
    AuthorizationCode, CodeChallengeMethod, OAuthAuthorizationCode, OAuthCodeChallenge,
};
use crate::core::client::OAuthRedirectUri;
use common::oauth_client_id::OAuthClientId;
use serde::{Deserialize, Serialize};
use serde_fields::SerdeField;
use std::collections::HashSet;
use time::OffsetDateTime;
use user::dynamodb::access_token_record::ScopeRecord;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, SerdeField)]
pub struct AuthorizationCodeRecord {
    pub pk: String,
    pub sk: String,
    pub code: OAuthAuthorizationCode,
    pub client_id: OAuthClientId,
    pub user_id: common::user_id::UserId,
    pub redirect_uri: OAuthRedirectUri,
    pub scopes: HashSet<ScopeRecord>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
    pub expires: i64,
    pub ttl: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
}

pub fn mk_pk(code: &OAuthAuthorizationCode) -> String {
    format!("oauth_authorization_code#{code}")
}

pub fn mk_sk() -> &'static str {
    "oauth_authorization_code"
}

impl From<AuthorizationCode> for AuthorizationCodeRecord {
    fn from(code: AuthorizationCode) -> Self {
        Self {
            pk: mk_pk(&code.code),
            sk: mk_sk().to_owned(),
            code: code.code,
            client_id: code.client_id,
            user_id: code.user_id,
            redirect_uri: code.redirect_uri,
            scopes: code.scopes.into_iter().map(Into::into).collect(),
            code_challenge: code.code_challenge,
            code_challenge_method: code.code_challenge_method,
            expires: code.expires.unix_timestamp(),
            ttl: code.expires.unix_timestamp(),
            created: code.created,
        }
    }
}

impl From<AuthorizationCodeRecord> for AuthorizationCode {
    fn from(record: AuthorizationCodeRecord) -> Self {
        Self {
            code: record.code,
            client_id: record.client_id,
            user_id: record.user_id,
            redirect_uri: record.redirect_uri,
            scopes: record.scopes.into_iter().map(Into::into).collect(),
            code_challenge: record.code_challenge,
            code_challenge_method: record.code_challenge_method,
            expires: OffsetDateTime::from_unix_timestamp(record.expires)
                .unwrap_or(OffsetDateTime::UNIX_EPOCH),
            created: record.created,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::authorization_code::OAuthCodeChallenge;
    use common::{oauth_client_id::OAuthClientId, user_id::UserId};
    use std::collections::HashSet;
    use time::OffsetDateTime;

    #[test]
    fn should_round_trip_through_serde_dynamo() {
        let now = OffsetDateTime::from_unix_timestamp(1_700_000_000).unwrap();
        let code_val = OAuthAuthorizationCode::new();
        let record = AuthorizationCodeRecord {
            pk: format!("oauth_authorization_code#{}", code_val),
            sk: "oauth_authorization_code".to_owned(),
            code: code_val,
            client_id: OAuthClientId::new(),
            user_id: UserId::new(),
            redirect_uri: OAuthRedirectUri::from("https://example.com/callback"),
            scopes: HashSet::from([
                user::dynamodb::access_token_record::ScopeRecord::ProductsWrite,
            ]),
            code_challenge: OAuthCodeChallenge::from("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"),
            code_challenge_method: CodeChallengeMethod::S256,
            expires: now.unix_timestamp() + 600,
            ttl: now.unix_timestamp() + 600,
            created: now,
        };

        let item: serde_dynamo::Item = serde_dynamo::to_item(record.clone()).unwrap();

        let back: AuthorizationCodeRecord = serde_dynamo::from_item(item).unwrap();

        assert_eq!(record.code, back.code, "code mismatch");
        assert_eq!(record.client_id, back.client_id, "client_id mismatch");
        assert_eq!(
            record.redirect_uri, back.redirect_uri,
            "redirect_uri mismatch"
        );
        assert_eq!(record.scopes, back.scopes, "scopes mismatch");
        assert_eq!(
            record.code_challenge, back.code_challenge,
            "code_challenge mismatch"
        );
        assert_eq!(
            record.code_challenge_method, back.code_challenge_method,
            "code_challenge_method mismatch"
        );
        assert_eq!(record.expires, back.expires, "expires mismatch");
    }
}
