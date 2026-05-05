use crate::domain::UpsertNewsletterSubscription;
use common::api::error::ApiError;

#[derive(thiserror::Error, Debug)]
pub enum ZohoCampaignsError {
    #[error("Failed to obtain Zoho OAuth access token: {0}")]
    OAuthTokenError(String),

    #[error("Zoho Campaigns API request failed: {0}")]
    ApiRequestError(String),

    #[error("Zoho Campaigns API returned error status '{status}' (code {code:?}): {message:?}")]
    ApiResponseError {
        status: String,
        message: Option<String>,
        code: Option<i64>,
    },
}

impl From<ZohoCampaignsError> for ApiError {
    fn from(err: ZohoCampaignsError) -> Self {
        let zoho_code = if let ZohoCampaignsError::ApiResponseError { code, .. } = &err {
            *code
        } else {
            None
        };

        match zoho_code {
            // 2004, 2007: Invalid contact email address
            // 2005: Group email address added
            Some(2004 | 2005 | 2007) => {
                ApiError::bad_request(common::api::error_code::INVALID_EMAIL, Box::new(err))
            }
            _ => ApiError::internal_server_error(
                common::api::error_code::INTERNAL_SERVER_ERROR,
                Box::new(err),
            ),
        }
    }
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ZohoCampaignsService {
    async fn subscribe(
        &self,
        subscription: &UpsertNewsletterSubscription,
    ) -> Result<(), ZohoCampaignsError>;
}

use serde::Deserialize;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    expires_in: i64,
}

#[derive(Debug, Deserialize)]
struct ZohoApiResponse {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default, deserialize_with = "deserialize_zoho_code")]
    code: Option<i64>,
}

/// Zoho Campaigns returns `code` as either a JSON integer (`2004`) or a JSON string
/// (`"2004"`, `"SUCCESS"`) depending on the API version and the response type.
/// This deserializer accepts both forms and converts them to `Option<i64>`:
/// - JSON integer  → `Some(n)`
/// - Numeric string like `"2004"` → `Some(2004)`
/// - Non-numeric / status string like `"SUCCESS"` → `None`
/// - JSON null / field absent → `None`
fn deserialize_zoho_code<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let opt: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match opt {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Number(n)) => Ok(n.as_i64()),
        Some(serde_json::Value::String(s)) => Ok(s.parse::<i64>().ok()),
        _ => Ok(None),
    }
}

#[derive(Debug)]
struct CachedToken {
    access_token: String,
    expires_at: OffsetDateTime,
}

pub struct ZohoCampaignsServiceImpl {
    list_key: String,
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    accounts_url: String,
    campaigns_url: String,
    cached_token: Arc<RwLock<Option<CachedToken>>>,
}

impl ZohoCampaignsServiceImpl {
    pub fn new(
        list_key: String,
        client: reqwest::Client,
        client_id: String,
        client_secret: String,
        refresh_token: String,
        accounts_url: String,
        campaigns_url: String,
    ) -> Self {
        Self {
            list_key,
            client,
            client_id,
            client_secret,
            refresh_token,
            accounts_url,
            campaigns_url,
            cached_token: Arc::new(RwLock::new(None)),
        }
    }

    async fn get_access_token(&self) -> Result<String, ZohoCampaignsError> {
        {
            let cached = self.cached_token.read().await;
            if let Some(ref token) = *cached
                && token.expires_at > OffsetDateTime::now_utc()
            {
                return Ok(token.access_token.clone());
            }
        }

        let url = format!("{}/oauth/v2/token", self.accounts_url);
        let response = self
            .client
            .post(&url)
            .form(&[
                ("refresh_token", self.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| ZohoCampaignsError::OAuthTokenError(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown".to_string());
            warn!(status = %status, body = %body, "Failed to obtain Zoho OAuth token.");
            return Err(ZohoCampaignsError::OAuthTokenError(format!(
                "HTTP {status}: {body}"
            )));
        }

        let token_response: OAuthTokenResponse = response
            .json()
            .await
            .map_err(|e| ZohoCampaignsError::OAuthTokenError(e.to_string()))?;

        let expires_at =
            OffsetDateTime::now_utc() + time::Duration::seconds(token_response.expires_in - 60);

        let access_token = token_response.access_token.clone();

        let mut cached = self.cached_token.write().await;
        *cached = Some(CachedToken {
            access_token: token_response.access_token,
            expires_at,
        });

        debug!("Obtained new Zoho OAuth access token.");
        Ok(access_token)
    }

    fn build_contact_info(subscription: &UpsertNewsletterSubscription) -> String {
        let mut contact = serde_json::Map::new();

        contact.insert(
            "Contact Email".to_string(),
            serde_json::Value::String(subscription.email.to_string()),
        );

        if let Some(ref first_name) = subscription.first_name {
            contact.insert(
                "First Name".to_string(),
                serde_json::Value::String(first_name.to_string()),
            );
        }

        if let Some(ref last_name) = subscription.last_name {
            contact.insert(
                "Last Name".to_string(),
                serde_json::Value::String(last_name.to_string()),
            );
        }

        if let Some(ref language) = subscription.language {
            contact.insert(
                "language".to_string(),
                serde_json::Value::String(format!("{language:?}")),
            );
        }

        if let Some(ref currency) = subscription.currency {
            contact.insert(
                "currency".to_string(),
                serde_json::Value::String(format!("{currency:?}")),
            );
        }

        if let Some(ref user_id) = subscription.user_id {
            contact.insert(
                "user_id".to_string(),
                serde_json::Value::String(user_id.to_string()),
            );
        }

        serde_json::Value::Object(contact).to_string()
    }
}

#[async_trait::async_trait]
impl ZohoCampaignsService for ZohoCampaignsServiceImpl {
    async fn subscribe(
        &self,
        subscription: &UpsertNewsletterSubscription,
    ) -> Result<(), ZohoCampaignsError> {
        let access_token = self.get_access_token().await?;
        let contact_info = Self::build_contact_info(subscription);

        let url = format!("{}/api/v1.1/json/listsubscribe", self.campaigns_url);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
            .form(&[
                ("resfmt", "JSON"),
                ("listkey", &self.list_key),
                ("contactinfo", &contact_info),
            ])
            .send()
            .await
            .map_err(|e| ZohoCampaignsError::ApiRequestError(e.to_string()))?;

        let api_response: ZohoApiResponse = response
            .json()
            .await
            .map_err(|e| ZohoCampaignsError::ApiRequestError(e.to_string()))?;

        if api_response.status != "success" {
            return Err(ZohoCampaignsError::ApiResponseError {
                status: api_response.status,
                message: api_response.message,
                code: api_response.code,
            });
        }

        info!(email = %subscription.email, zohoMessage = ?api_response.message, "Subscribed contact to Zoho Campaigns.");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::UpsertNewsletterSubscription;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn mk_subscription() -> UpsertNewsletterSubscription {
        UpsertNewsletterSubscription {
            email: "test.user@example.com".try_into().unwrap(),
            first_name: Some("John".into()),
            last_name: Some("Doe".into()),
            language: Some(common::language::domain::Language::En),
            currency: Some(common::currency::domain::Currency::Eur),
            user_id: Some(common::user_id::UserId::new()),
        }
    }

    fn mk_service(mock_server_url: &str, list_key: &str) -> ZohoCampaignsServiceImpl {
        ZohoCampaignsServiceImpl::new(
            list_key.to_string(),
            reqwest::Client::new(),
            "test-client-id".to_string(),
            "test-client-secret".to_string(),
            "test-refresh-token".to_string(),
            mock_server_url.to_string(),
            mock_server_url.to_string(),
        )
    }

    fn mock_oauth_success() -> Mock {
        Mock::given(method("POST"))
            .and(path("/oauth/v2/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("client_id=test-client-id"))
            .and(body_string_contains("client_secret=test-client-secret"))
            .and(body_string_contains("refresh_token=test-refresh-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "mock-access-token",
                "expires_in": 3600
            })))
    }

    fn mock_subscribe_success(list_key: &str) -> Mock {
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .and(header("Authorization", "Zoho-oauthtoken mock-access-token"))
            .and(body_string_contains(format!("listkey={list_key}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "message": "Contact added successfully."
            })))
    }

    #[tokio::test]
    async fn should_subscribe_contact_when_api_returns_success() {
        let mock_server = MockServer::start().await;
        let list_key = "test-list-key";
        mock_oauth_success().mount(&mock_server).await;
        mock_subscribe_success(list_key).mount(&mock_server).await;

        let service = mk_service(&mock_server.uri(), list_key);
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_return_oauth_token_error_when_token_endpoint_fails() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/v2/token"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": "invalid_client"
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "test-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ZohoCampaignsError::OAuthTokenError(_))
        ));
    }

    #[tokio::test]
    async fn should_return_api_response_error_when_subscribe_fails() {
        let mock_server = MockServer::start().await;
        mock_oauth_success().mount(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "error",
                "message": "Invalid list key.",
                "code": 2501
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "bad-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ZohoCampaignsError::ApiResponseError {
                code: Some(2501),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn should_return_api_response_error_with_none_code_when_code_absent() {
        let mock_server = MockServer::start().await;
        mock_oauth_success().mount(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "error",
                "message": "Unknown error."
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "bad-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ZohoCampaignsError::ApiResponseError { code: None, .. })
        ));
    }

    #[tokio::test]
    async fn should_parse_string_error_code_when_zoho_returns_code_as_string() {
        let mock_server = MockServer::start().await;
        mock_oauth_success().mount(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "error",
                "message": "Invalid contact email address.",
                "code": "2004"
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "test-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ZohoCampaignsError::ApiResponseError {
                code: Some(2004),
                ..
            })
        ));
    }

    #[tokio::test]
    async fn should_parse_none_code_when_zoho_returns_non_numeric_string_code() {
        let mock_server = MockServer::start().await;
        mock_oauth_success().mount(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "error",
                "message": "Unknown error.",
                "code": "ERROR"
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "test-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ZohoCampaignsError::ApiResponseError { code: None, .. })
        ));
    }

    #[tokio::test]
    async fn should_succeed_when_zoho_returns_success_without_message_field() {
        let mock_server = MockServer::start().await;
        mock_oauth_success().mount(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success"
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "test-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_cache_oauth_token_across_calls() {
        let mock_server = MockServer::start().await;
        mock_oauth_success().expect(1).mount(&mock_server).await;
        mock_subscribe_success("test-list-key")
            .expect(2)
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "test-list-key");
        let subscription = mk_subscription();

        let result1 = service.subscribe(&subscription).await;
        let result2 = service.subscribe(&subscription).await;

        assert!(result1.is_ok());
        assert!(result2.is_ok());
    }

    #[tokio::test]
    async fn should_include_contact_email_in_subscribe_request() {
        let mock_server = MockServer::start().await;
        let email_encoded = "test.user%40example.com";

        mock_oauth_success().mount(&mock_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .and(body_string_contains(email_encoded))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success",
                "message": "Contact added."
            })))
            .mount(&mock_server)
            .await;

        let service = mk_service(&mock_server.uri(), "test-list-key");
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn should_return_api_request_error_when_network_fails() {
        let service = ZohoCampaignsServiceImpl::new(
            "test-list-key".to_string(),
            reqwest::Client::new(),
            "test-client-id".to_string(),
            "test-client-secret".to_string(),
            "test-refresh-token".to_string(),
            "http://localhost:1".to_string(),
            "http://localhost:1".to_string(),
        );
        let subscription = mk_subscription();

        let result = service.subscribe(&subscription).await;

        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ZohoCampaignsError::OAuthTokenError(_))
        ));
    }

    #[test]
    fn should_build_contact_info_with_all_fields() {
        let subscription = mk_subscription();
        let contact_info = ZohoCampaignsServiceImpl::build_contact_info(&subscription);

        let parsed: serde_json::Value = serde_json::from_str(&contact_info).unwrap();
        assert!(parsed.get("Contact Email").is_some());
        assert!(parsed.get("First Name").is_some());
        assert!(parsed.get("Last Name").is_some());
        assert!(parsed.get("language").is_some());
        assert!(parsed.get("currency").is_some());
        assert!(parsed.get("user_id").is_some());
    }

    #[test]
    fn should_build_contact_info_with_only_email_when_optional_fields_are_none() {
        let subscription = UpsertNewsletterSubscription {
            email: "minimal@example.com".try_into().unwrap(),
            first_name: None,
            last_name: None,
            language: None,
            currency: None,
            user_id: None,
        };

        let contact_info = ZohoCampaignsServiceImpl::build_contact_info(&subscription);
        let parsed: serde_json::Value = serde_json::from_str(&contact_info).unwrap();

        assert_eq!(
            parsed.get("Contact Email").unwrap().as_str().unwrap(),
            "minimal@example.com"
        );
        assert!(parsed.get("First Name").is_none());
        assert!(parsed.get("Last Name").is_none());
        assert!(parsed.get("language").is_none());
        assert!(parsed.get("currency").is_none());
        assert!(parsed.get("user_id").is_none());
    }
}
