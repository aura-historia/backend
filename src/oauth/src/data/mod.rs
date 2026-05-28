use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use user::data::access_token_data::{AccessTokenTypeData, ScopeData};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TokenResponseData {
    pub access_token: String,
    pub token_type: AccessTokenTypeData,
    pub expires_in: Option<i64>,
    pub scope: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntrospectionResponseData {
    pub active: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iat: Option<i64>,
}

pub fn scope_string(scopes: &HashSet<user::core::access_token::Scope>) -> String {
    let mut values = scopes
        .iter()
        .copied()
        .map(ScopeData::from)
        .map(|scope| scope.as_str().to_owned())
        .collect::<Vec<_>>();
    values.sort();
    values.join(" ")
}
