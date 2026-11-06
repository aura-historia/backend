use crate::client_identifier::OAuthClientIdentifier;
use crate::resource::AuraResource;
use credential_core::scope::Scope;
use domain_primitives::{string_newtype, uuid_v7_newtype};
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::user_id::UserId;

uuid_v7_newtype!(OAuthAuthorizationCode);
string_newtype!(
    OAuthCodeChallenge,
    derives(serde::Serialize, serde::Deserialize)
);
string_newtype!(OAuthCodeVerifier);

#[derive(Debug, Clone, PartialEq)]
pub struct AuthorizationCode {
    code: OAuthAuthorizationCode,
    client_id: OAuthClientIdentifier,
    user_id: UserId,
    redirect_uri: url::Url,
    resource: Option<AuraResource>,
    scopes: HashSet<Scope>,
    code_challenge: OAuthCodeChallenge,
    code_challenge_method: CodeChallengeMethod,
    expires: OffsetDateTime,
}

#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct RehydratedAuthorizationCodeState {
    pub code: OAuthAuthorizationCode,
    pub client_id: OAuthClientIdentifier,
    pub user_id: UserId,
    pub redirect_uri: url::Url,
    /// Absent only on outstanding codes issued before resource binding.
    pub resource: Option<AuraResource>,
    pub scopes: HashSet<Scope>,
    pub code_challenge: OAuthCodeChallenge,
    pub code_challenge_method: CodeChallengeMethod,
    pub expires: OffsetDateTime,
}

impl AuthorizationCode {
    pub fn create(state: RehydratedAuthorizationCodeState) -> Self {
        Self::rehydrate(state)
    }

    #[doc(hidden)]
    pub fn rehydrate(state: RehydratedAuthorizationCodeState) -> Self {
        Self {
            code: state.code,
            client_id: state.client_id,
            user_id: state.user_id,
            redirect_uri: state.redirect_uri,
            resource: state.resource,
            scopes: state.scopes,
            code_challenge: state.code_challenge,
            code_challenge_method: state.code_challenge_method,
            expires: state.expires,
        }
    }

    pub fn code(&self) -> OAuthAuthorizationCode {
        self.code
    }

    /// The client the code was issued to; only that client may exchange it.
    pub fn client_id(&self) -> &OAuthClientIdentifier {
        &self.client_id
    }

    pub fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn redirect_uri(&self) -> &url::Url {
        &self.redirect_uri
    }

    pub fn resource(&self) -> Option<AuraResource> {
        self.resource
    }

    pub fn scopes(&self) -> &HashSet<Scope> {
        &self.scopes
    }

    pub fn code_challenge(&self) -> &OAuthCodeChallenge {
        &self.code_challenge
    }

    pub fn expires(&self) -> OffsetDateTime {
        self.expires
    }

    pub fn is_expired_at(&self, now: OffsetDateTime) -> bool {
        self.expires < now
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum CodeChallengeMethod {
    S256,
}
