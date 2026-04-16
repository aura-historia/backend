use crate::core::user::User;

#[derive(thiserror::Error, Debug)]
pub enum ZohoCampaignsError {
    #[error("Failed to obtain Zoho OAuth access token: {0}")]
    OAuthTokenError(String),

    #[error("Zoho Campaigns API request failed: {0}")]
    ApiRequestError(String),

    #[error("Zoho Campaigns API returned error status '{status}': {message}")]
    ApiResponseError { status: String, message: String },
}

#[async_trait::async_trait]
#[mockall::automock]
pub trait ZohoCampaignsService {
    async fn subscribe(&self, user: &User) -> Result<(), ZohoCampaignsError>;
    async fn unsubscribe(&self, user: &User) -> Result<(), ZohoCampaignsError>;
}

#[cfg(feature = "zoho")]
pub mod zoho_impl {
    use super::{ZohoCampaignsError, ZohoCampaignsService};
    use crate::core::user::User;
    use serde::Deserialize;
    use std::sync::Arc;
    use time::OffsetDateTime;
    use tokio::sync::RwLock;
    use tracing::{debug, error};

    #[derive(Debug, Deserialize)]
    struct OAuthTokenResponse {
        access_token: String,
        expires_in: i64,
    }

    #[derive(Debug, Deserialize)]
    struct ZohoApiResponse {
        status: String,
        message: String,
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
                error!(status = %status, body = %body, "Failed to obtain Zoho OAuth token.");
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

        fn build_contact_info(user: &User) -> String {
            let mut contact = serde_json::Map::new();

            contact.insert(
                "Contact Email".to_string(),
                serde_json::Value::String(user.email.to_string()),
            );

            if let Some(ref first_name) = user.first_name {
                contact.insert(
                    "First Name".to_string(),
                    serde_json::Value::String(first_name.to_string()),
                );
            }

            if let Some(ref last_name) = user.last_name {
                contact.insert(
                    "Last Name".to_string(),
                    serde_json::Value::String(last_name.to_string()),
                );
            }

            contact.insert(
                "Marketing Consent".to_string(),
                serde_json::Value::String(user.marketing_consent.to_string()),
            );

            if let Some(ref language) = user.language {
                contact.insert(
                    "Language".to_string(),
                    serde_json::Value::String(format!("{language:?}")),
                );
            }

            if let Some(ref currency) = user.currency {
                contact.insert(
                    "Currency".to_string(),
                    serde_json::Value::String(format!("{currency:?}")),
                );
            }

            contact.insert(
                "Tier".to_string(),
                serde_json::Value::String(format!("{:?}", user.tier)),
            );

            serde_json::Value::Object(contact).to_string()
        }
    }

    #[async_trait::async_trait]
    impl ZohoCampaignsService for ZohoCampaignsServiceImpl {
        async fn subscribe(&self, user: &User) -> Result<(), ZohoCampaignsError> {
            let access_token = self.get_access_token().await?;
            let contact_info = Self::build_contact_info(user);

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
                });
            }

            debug!(email = %user.email, "Subscribed user to Zoho Campaigns.");
            Ok(())
        }

        async fn unsubscribe(&self, user: &User) -> Result<(), ZohoCampaignsError> {
            let access_token = self.get_access_token().await?;

            let url = format!("{}/api/v1.1/json/listunsubscribe", self.campaigns_url);
            let response = self
                .client
                .post(&url)
                .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
                .form(&[
                    ("resfmt", "JSON"),
                    ("listkey", &self.list_key),
                    ("contactinfo", user.email.as_ref()),
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
                });
            }

            debug!(email = %user.email, "Unsubscribed user from Zoho Campaigns.");
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::core::user::User;
        use fake::{Fake, Faker};
        use wiremock::matchers::{body_string_contains, header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn mk_user() -> User {
            Faker.fake()
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

        fn mock_unsubscribe_success(list_key: &str) -> Mock {
            Mock::given(method("POST"))
                .and(path("/api/v1.1/json/listunsubscribe"))
                .and(header("Authorization", "Zoho-oauthtoken mock-access-token"))
                .and(body_string_contains(format!("listkey={list_key}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "status": "success",
                    "message": "Contact unsubscribed successfully."
                })))
        }

        #[tokio::test]
        async fn should_subscribe_user_when_api_returns_success() {
            let mock_server = MockServer::start().await;
            let list_key = "test-list-key";
            mock_oauth_success().mount(&mock_server).await;
            mock_subscribe_success(list_key).mount(&mock_server).await;

            let service = mk_service(&mock_server.uri(), list_key);
            let user = mk_user();

            let result = service.subscribe(&user).await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_unsubscribe_user_when_api_returns_success() {
            let mock_server = MockServer::start().await;
            let list_key = "test-list-key";
            mock_oauth_success().mount(&mock_server).await;
            mock_unsubscribe_success(list_key).mount(&mock_server).await;

            let service = mk_service(&mock_server.uri(), list_key);
            let user = mk_user();

            let result = service.unsubscribe(&user).await;

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
            let user = mk_user();

            let result = service.subscribe(&user).await;

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
                    "message": "Invalid list key."
                })))
                .mount(&mock_server)
                .await;

            let service = mk_service(&mock_server.uri(), "bad-list-key");
            let user = mk_user();

            let result = service.subscribe(&user).await;

            assert!(result.is_err());
            assert!(matches!(
                result,
                Err(ZohoCampaignsError::ApiResponseError { .. })
            ));
        }

        #[tokio::test]
        async fn should_return_api_response_error_when_unsubscribe_fails() {
            let mock_server = MockServer::start().await;
            mock_oauth_success().mount(&mock_server).await;
            Mock::given(method("POST"))
                .and(path("/api/v1.1/json/listunsubscribe"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "status": "error",
                    "message": "Contact not found."
                })))
                .mount(&mock_server)
                .await;

            let service = mk_service(&mock_server.uri(), "test-list-key");
            let user = mk_user();

            let result = service.unsubscribe(&user).await;

            assert!(result.is_err());
            assert!(matches!(
                result,
                Err(ZohoCampaignsError::ApiResponseError { .. })
            ));
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
            let user = mk_user();

            let result1 = service.subscribe(&user).await;
            let result2 = service.subscribe(&user).await;

            assert!(result1.is_ok());
            assert!(result2.is_ok());
        }

        #[tokio::test]
        async fn should_include_contact_email_in_subscribe_request() {
            let mock_server = MockServer::start().await;
            let mut user = mk_user();
            user.email = "test.user@example.com".try_into().unwrap();
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

            let result = service.subscribe(&user).await;

            assert!(result.is_ok());
        }

        #[tokio::test]
        async fn should_include_contact_email_in_unsubscribe_request() {
            let mock_server = MockServer::start().await;
            let user = mk_user();
            let email_encoded = user.email.to_string().replace('@', "%40");

            mock_oauth_success().mount(&mock_server).await;
            Mock::given(method("POST"))
                .and(path("/api/v1.1/json/listunsubscribe"))
                .and(body_string_contains(&email_encoded))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "status": "success",
                    "message": "Contact unsubscribed."
                })))
                .mount(&mock_server)
                .await;

            let service = mk_service(&mock_server.uri(), "test-list-key");

            let result = service.unsubscribe(&user).await;

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
            let user = mk_user();

            let result = service.subscribe(&user).await;

            assert!(result.is_err());
            assert!(matches!(
                result,
                Err(ZohoCampaignsError::OAuthTokenError(_))
            ));
        }

        #[test]
        fn should_build_contact_info_with_all_fields_for_full_user() {
            let user = mk_user();
            let contact_info = ZohoCampaignsServiceImpl::build_contact_info(&user);

            let parsed: serde_json::Value = serde_json::from_str(&contact_info).unwrap();
            assert!(parsed.get("Contact Email").is_some());
            assert!(parsed.get("Marketing Consent").is_some());
            assert!(parsed.get("Tier").is_some());
        }

        #[test]
        fn should_build_contact_info_with_minimal_fields_for_user_without_optional_fields() {
            use crate::core::{role::UserRole, tier::UserTier};
            use common::user_id::UserId;
            use time::OffsetDateTime;

            let user = User {
                user_id: UserId::new(),
                email: "test@example.com".try_into().unwrap(),
                first_name: None,
                last_name: None,
                language: None,
                currency: None,
                prohibited_content_consent: false,
                marketing_consent: false,
                tier: UserTier::Free,
                role: UserRole::User,
                created: OffsetDateTime::now_utc(),
                updated: OffsetDateTime::now_utc(),
            };

            let contact_info = ZohoCampaignsServiceImpl::build_contact_info(&user);
            let parsed: serde_json::Value = serde_json::from_str(&contact_info).unwrap();

            assert_eq!(
                parsed.get("Contact Email").unwrap().as_str().unwrap(),
                "test@example.com"
            );
            assert_eq!(
                parsed.get("Marketing Consent").unwrap().as_str().unwrap(),
                "false"
            );
            assert!(parsed.get("First Name").is_none());
            assert!(parsed.get("Last Name").is_none());
            assert!(parsed.get("Language").is_none());
            assert!(parsed.get("Currency").is_none());
        }
    }
}
