use credential_core::scope::Scope;
use domain_primitives::uuid_v7_newtype;
use std::collections::HashSet;
use time::OffsetDateTime;
use user_core::access_token::RawAccessToken;

uuid_v7_newtype!(ThirdPartyExchangeCode);

#[derive(Debug, Clone, PartialEq)]
pub struct ThirdPartyExchangeCodeGrant {
    pub code: ThirdPartyExchangeCode,
    pub access_token: RawAccessToken,
    pub access_token_expires: Option<OffsetDateTime>,
    pub scopes: HashSet<Scope>,
    pub expires: OffsetDateTime,
    pub created: OffsetDateTime,
}

impl ThirdPartyExchangeCodeGrant {
    pub fn is_expired(&self) -> bool {
        self.expires < OffsetDateTime::now_utc()
    }
}
