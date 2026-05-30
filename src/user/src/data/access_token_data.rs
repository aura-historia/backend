use crate::core::access_token::{AccessToken, AccessTokenId, RawAccessToken, Scope};
use common::actor::data::ActorData;
use serde::{
    Deserialize, Serialize,
    de::{self, Visitor},
};
use std::collections::HashSet;
use time::OffsetDateTime;

#[cfg_attr(feature = "test-data", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScopeData {
    ShopsManage,
    ProductsWrite,
}

impl ScopeData {
    /// Returns the OAuth scope string representation, e.g. `"shops:manage"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScopeData::ShopsManage => "shops:manage",
            ScopeData::ProductsWrite => "products:write",
        }
    }

    /// All known scope strings, used for error messages.
    const VARIANTS: &'static [&'static str] = &["shops:manage", "products:write"];
}

impl TryFrom<&str> for ScopeData {
    type Error = String;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "shops:manage" => Ok(ScopeData::ShopsManage),
            "products:write" => Ok(ScopeData::ProductsWrite),
            unknown => Err(format!(
                "unknown scope `{unknown}`, expected one of: {}",
                ScopeData::VARIANTS.join(", ")
            )),
        }
    }
}

impl Serialize for ScopeData {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

struct ScopeDataVisitor;

impl<'de> Visitor<'de> for ScopeDataVisitor {
    type Value = ScopeData;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(
            formatter,
            "an OAuth scope string, one of: {}",
            ScopeData::VARIANTS.join(", ")
        )
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        ScopeData::try_from(value).map_err(de::Error::custom)
    }
}

impl<'de> Deserialize<'de> for ScopeData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_str(ScopeDataVisitor)
    }
}

impl From<Scope> for ScopeData {
    fn from(value: Scope) -> Self {
        match value {
            Scope::ShopsManage => ScopeData::ShopsManage,
            Scope::ProductsWrite => ScopeData::ProductsWrite,
        }
    }
}

impl From<ScopeData> for Scope {
    fn from(value: ScopeData) -> Self {
        match value {
            ScopeData::ShopsManage => Scope::ShopsManage,
            ScopeData::ProductsWrite => Scope::ProductsWrite,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum AccessTokenTypeData {
    Bearer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RawAccessTokenData(pub String);

impl From<RawAccessToken> for RawAccessTokenData {
    fn from(value: RawAccessToken) -> Self {
        RawAccessTokenData(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetAccessTokenData {
    pub access_token_id: AccessTokenId,
    pub name: String,
    #[serde(default, skip_serializing_if = "HashSet::is_empty")]
    pub scope: HashSet<ScopeData>,
    pub token: String,
    pub token_type: AccessTokenTypeData,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<i64>,
    pub created_by: ActorData,
    pub updated_by: ActorData,
    #[serde(with = "time::serde::rfc3339")]
    pub created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PostAccessTokenData {
    pub name: String,
    #[serde(default)]
    pub scope: HashSet<ScopeData>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PatchAccessTokenData {
    pub access_token_id: AccessTokenId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<HashSet<ScopeData>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::rfc3339::option"
    )]
    pub expires_at: Option<OffsetDateTime>,
}

impl From<AccessToken> for GetAccessTokenData {
    fn from(access_token: AccessToken) -> Self {
        let now = OffsetDateTime::now_utc();
        GetAccessTokenData {
            access_token_id: access_token.id,
            name: access_token.name.into(),
            token: access_token.hashed_token.to_string(),
            scope: access_token.scopes.into_iter().map(Into::into).collect(),
            token_type: AccessTokenTypeData::Bearer,
            expires_at: access_token.expires,
            expires_in: access_token
                .expires
                .map(|expires| (expires - now).whole_seconds().max(0)),
            created_by: access_token.created_by.into(),
            updated_by: access_token.updated_by.into(),
            created: access_token.created,
            updated: access_token.updated,
        }
    }
}

impl From<(RawAccessToken, AccessToken)> for GetAccessTokenData {
    fn from((raw, access_token): (RawAccessToken, AccessToken)) -> Self {
        GetAccessTokenData {
            access_token_id: access_token.id,
            name: access_token.name.into(),
            token: raw.into(),
            scope: access_token.scopes.into_iter().map(Into::into).collect(),
            token_type: AccessTokenTypeData::Bearer,
            expires_at: access_token.expires,
            expires_in: access_token
                .expires
                .map(|expires| (expires - OffsetDateTime::now_utc()).whole_seconds().max(0)),
            created_by: access_token.created_by.into(),
            updated_by: access_token.updated_by.into(),
            created: access_token.created,
            updated: access_token.updated,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        core::access_token::AccessTokenId,
        data::access_token_data::{AccessTokenTypeData, GetAccessTokenData, ScopeData},
    };
    use common::actor::data::ActorData;
    use rstest::rstest;
    use std::collections::HashSet;
    use time::OffsetDateTime;

    #[rstest]
    #[case(ScopeData::ShopsManage, "shops:manage")]
    #[case(ScopeData::ProductsWrite, "products:write")]
    fn should_serialize_scope_data_to_oauth_string(
        #[case] scope: ScopeData,
        #[case] expected: &str,
    ) {
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(format!("\"{expected}\""), json);
    }

    #[rstest]
    #[case("shops:manage", ScopeData::ShopsManage)]
    #[case("products:write", ScopeData::ProductsWrite)]
    fn should_deserialize_scope_data_from_oauth_string(
        #[case] input: &str,
        #[case] expected: ScopeData,
    ) {
        let json = format!("\"{input}\"");
        let actual: ScopeData = serde_json::from_str(&json).unwrap();
        assert_eq!(expected, actual);
    }

    #[rstest]
    #[case(ScopeData::ShopsManage)]
    #[case(ScopeData::ProductsWrite)]
    fn should_round_trip_scope_data(#[case] scope: ScopeData) {
        let json = serde_json::to_string(&scope).unwrap();
        let actual: ScopeData = serde_json::from_str(&json).unwrap();
        assert_eq!(scope, actual);
    }

    #[rstest]
    #[case("shops_manage")]
    #[case("products_write")]
    #[case("ShopsManage")]
    #[case("ProductsWrite")]
    #[case("unknown:scope")]
    #[case("")]
    fn should_reject_invalid_scope_data_strings(#[case] input: &str) {
        let json = format!("\"{input}\"");
        let result: Result<ScopeData, _> = serde_json::from_str(&json);
        assert!(result.is_err(), "expected error for input `{input}`");
    }

    #[test]
    fn should_serialize_scope_data_set_with_all_variants() {
        let scope: HashSet<ScopeData> = [ScopeData::ShopsManage, ScopeData::ProductsWrite]
            .into_iter()
            .collect();
        let json = serde_json::to_value(&scope).unwrap();
        let arr = json.as_array().unwrap();
        let strings: HashSet<&str> = arr.iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(HashSet::from(["shops:manage", "products:write"]), strings);
    }

    #[test]
    fn should_deserialize_scope_data_set_from_json_array() {
        let json = r#"["shops:manage", "products:write"]"#;
        let actual: HashSet<ScopeData> = serde_json::from_str(json).unwrap();
        assert_eq!(
            HashSet::from([ScopeData::ShopsManage, ScopeData::ProductsWrite]),
            actual
        );
    }

    #[test]
    fn should_reject_old_snake_case_format_in_scope_data_set() {
        let json = r#"["shops_manage"]"#;
        let result: Result<HashSet<ScopeData>, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn should_serialize_get_access_token_data_with_oauth_scopes() {
        let data = GetAccessTokenData {
            access_token_id: AccessTokenId::new(),
            name: "Test Token".to_string(),
            scope: [ScopeData::ProductsWrite].into_iter().collect(),
            token: "hashed_token".to_string(),
            token_type: AccessTokenTypeData::Bearer,
            expires_at: Some(OffsetDateTime::now_utc() + time::Duration::days(30)),
            expires_in: Some(2592000),
            created_by: ActorData::System,
            updated_by: ActorData::System,
            created: OffsetDateTime::now_utc(),
            updated: OffsetDateTime::now_utc(),
        };

        let expected = serde_json::json!({
            "accessTokenId": data.access_token_id.to_string(),
            "name": "Test Token",
            "scope": ["products:write"],
            "token": "hashed_token",
            "tokenType": "BEARER",
            "expiresAt": data.expires_at.unwrap().format(&time::format_description::well_known::Rfc3339).unwrap(),
            "expiresIn": 2592000,
            "createdBy": "SYSTEM",
            "updatedBy": "SYSTEM",
            "created": data.created.format(&time::format_description::well_known::Rfc3339).unwrap(),
            "updated": data.updated.format(&time::format_description::well_known::Rfc3339).unwrap(),
        });

        let actual = serde_json::to_value(&data).unwrap();
        assert_eq!(expected, actual);
    }

    #[test]
    fn should_deserialize_get_access_token_data_with_oauth_scopes() {
        let json = serde_json::json!({
            "accessTokenId": "01970000-0000-7000-8000-000000000001",
            "name": "Test Token",
            "scope": ["shops:manage", "products:write"],
            "token": "raw_token",
            "tokenType": "BEARER",
            "createdBy": "SYSTEM",
            "updatedBy": "SYSTEM",
            "created": "2025-01-01T00:00:00Z",
            "updated": "2025-01-01T00:00:00Z",
        });

        let data: GetAccessTokenData = serde_json::from_value(json).unwrap();
        assert_eq!(
            HashSet::from([ScopeData::ShopsManage, ScopeData::ProductsWrite]),
            data.scope
        );
    }
}
