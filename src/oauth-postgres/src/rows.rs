use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct OAuthClientRow {
    pub(crate) client_id: Uuid,
    pub(crate) secret_short: String,
    pub(crate) secret_hash: String,
    pub(crate) name: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) tos_uri: String,
    pub(crate) policy_uri: String,
    pub(crate) client_uri: String,
    pub(crate) logo_uri: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) version: i64,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct OAuthClientViewRow {
    pub(crate) client_id: Uuid,
    pub(crate) name: String,
    pub(crate) redirect_uris: Vec<String>,
    pub(crate) tos_uri: String,
    pub(crate) policy_uri: String,
    pub(crate) client_uri: String,
    pub(crate) logo_uri: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) created: OffsetDateTime,
    pub(crate) updated: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct AuthorizationCodeRow {
    pub(crate) code: String,
    pub(crate) client_id: Option<Uuid>,
    pub(crate) metadata_document_client_id: Option<String>,
    pub(crate) user_id: Uuid,
    pub(crate) redirect_uri: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) resource: Option<String>,
    pub(crate) code_challenge: String,
    pub(crate) code_challenge_method: String,
    pub(crate) issues_refresh_token: bool,
    pub(crate) expires: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct ThirdPartyExchangeCodeRow {
    pub(crate) code: String,
    pub(crate) access_token_id: Uuid,
    pub(crate) access_token: String,
    pub(crate) access_token_expires: Option<OffsetDateTime>,
    pub(crate) scopes: Vec<String>,
    pub(crate) expires: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct OAuthConnectionRow {
    pub(crate) connection_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) client_id: String,
    pub(crate) client_name: String,
    pub(crate) scopes: Vec<String>,
    pub(crate) created: OffsetDateTime,
    pub(crate) last_used: OffsetDateTime,
}

#[derive(Debug, sqlx::FromRow)]
pub(crate) struct RefreshTokenFamilyRow {
    pub(crate) family_id: Uuid,
    pub(crate) connection_id: Uuid,
    pub(crate) scopes: Vec<String>,
    pub(crate) resource: String,
    pub(crate) created: OffsetDateTime,
    pub(crate) absolute_expires_at: OffsetDateTime,
    pub(crate) expires_at: OffsetDateTime,
    pub(crate) current_token_short: String,
    pub(crate) current_token_hash: String,
    pub(crate) current_issued: OffsetDateTime,
    /// When the presented token stopped being current; `None` if it still is.
    pub(crate) presented_replaced_at: Option<OffsetDateTime>,
}
