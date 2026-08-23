use application::error::{BoxError, box_error, static_error};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use user_core::newsletter_subscription::NewsletterSubscription;
use user_service::ports::{NewsletterSubscriptionWriteError, NewsletterSubscriptionWriter};

const TOKEN_EXPIRY_SKEW: Duration = Duration::from_secs(60);
const INVALID_EMAIL_CODES: [i64; 3] = [2004, 2005, 2007];

struct CachedAccessToken {
    value: String,
    expires_at: Instant,
}

#[derive(Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    expires_in: u64,
}

#[derive(Deserialize)]
struct ZohoResponse {
    status: String,
    #[serde(default, deserialize_with = "deserialize_zoho_code")]
    code: Option<i64>,
}

fn deserialize_zoho_code<'de, D>(deserializer: D) -> Result<Option<i64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<Value>::deserialize(deserializer)?;
    Ok(match value {
        Some(Value::Number(number)) => number.as_i64(),
        Some(Value::String(code)) => code.parse().ok(),
        _ => None,
    })
}

#[derive(Debug, thiserror::Error)]
#[error("Zoho {operation} returned HTTP {status}")]
struct ZohoHttpStatusError {
    operation: &'static str,
    status: StatusCode,
}

pub struct ZohoNewsletterSubscriptionWriter {
    list_key: String,
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    refresh_token: String,
    accounts_url: String,
    campaigns_url: String,
    cached_access_token: Mutex<Option<CachedAccessToken>>,
}

impl ZohoNewsletterSubscriptionWriter {
    #[allow(clippy::too_many_arguments)]
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
            cached_access_token: Mutex::new(None),
        }
    }

    async fn access_token(&self) -> Result<String, NewsletterSubscriptionWriteError> {
        let mut cached_access_token = self.cached_access_token.lock().await;
        if let Some(cached) = cached_access_token.as_ref()
            && cached.expires_at > Instant::now()
        {
            return Ok(cached.value.clone());
        }

        let response = self
            .client
            .post(format!(
                "{}/oauth/v2/token",
                trim_trailing_slash(&self.accounts_url)
            ))
            .form(&[
                ("refresh_token", self.refresh_token.as_str()),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|error| temporarily_unavailable(box_error(error)))?;

        if !response.status().is_success() {
            return Err(temporarily_unavailable(box_error(ZohoHttpStatusError {
                operation: "OAuth token endpoint",
                status: response.status(),
            })));
        }

        let token = response
            .json::<OAuthTokenResponse>()
            .await
            .map_err(|error| internal(box_error(error)))?;
        let cache_duration =
            Duration::from_secs(token.expires_in).saturating_sub(TOKEN_EXPIRY_SKEW);
        if cache_duration.is_zero() {
            return Err(internal(static_error(
                "Zoho OAuth token expiry is too short",
            )));
        }

        let access_token = token.access_token.clone();
        *cached_access_token = Some(CachedAccessToken {
            value: token.access_token,
            expires_at: Instant::now() + cache_duration,
        });
        Ok(access_token)
    }

    fn contact_info(
        subscription: &NewsletterSubscription,
    ) -> Result<String, NewsletterSubscriptionWriteError> {
        let mut contact = Map::new();
        contact.insert(
            "Contact Email".into(),
            Value::String(subscription.email().to_string()),
        );
        if let Some(first_name) = subscription.first_name() {
            contact.insert("First Name".into(), Value::String(first_name.to_string()));
        }
        if let Some(last_name) = subscription.last_name() {
            contact.insert("Last Name".into(), Value::String(last_name.to_string()));
        }
        if let Some(language) = subscription.language() {
            contact.insert("language".into(), Value::String(language.as_str().into()));
        }
        if let Some(currency) = subscription.currency() {
            contact.insert("currency".into(), Value::String(currency.as_str().into()));
        }
        if let Some(user_id) = subscription.user_id() {
            contact.insert("user_id".into(), Value::String(user_id.to_string()));
        }
        serde_json::to_string(&contact).map_err(|error| internal(box_error(error)))
    }

    async fn subscribe_to_list(
        &self,
        subscription: &NewsletterSubscription,
    ) -> Result<(), NewsletterSubscriptionWriteError> {
        let access_token = self.access_token().await?;
        let contact_info = Self::contact_info(subscription)?;
        let response = self
            .client
            .post(format!(
                "{}/api/v1.1/json/listsubscribe",
                trim_trailing_slash(&self.campaigns_url)
            ))
            .header("Authorization", format!("Zoho-oauthtoken {access_token}"))
            .form(&[
                ("resfmt", "JSON"),
                ("listkey", self.list_key.as_str()),
                ("contactinfo", contact_info.as_str()),
            ])
            .send()
            .await
            .map_err(|error| temporarily_unavailable(box_error(error)))?;

        if !response.status().is_success() {
            return Err(temporarily_unavailable(box_error(ZohoHttpStatusError {
                operation: "Campaigns subscription endpoint",
                status: response.status(),
            })));
        }

        let body = response
            .json::<ZohoResponse>()
            .await
            .map_err(|error| internal(box_error(error)))?;
        if body.status.eq_ignore_ascii_case("success") {
            return Ok(());
        }
        if body
            .code
            .is_some_and(|code| INVALID_EMAIL_CODES.contains(&code))
        {
            return Err(NewsletterSubscriptionWriteError::InvalidEmail);
        }
        Err(internal(static_error(
            "Zoho Campaigns rejected subscription request",
        )))
    }
}

fn trim_trailing_slash(url: &str) -> &str {
    url.trim_end_matches('/')
}

fn temporarily_unavailable(source: BoxError) -> NewsletterSubscriptionWriteError {
    NewsletterSubscriptionWriteError::TemporarilyUnavailable { source }
}

fn internal(source: BoxError) -> NewsletterSubscriptionWriteError {
    NewsletterSubscriptionWriteError::Internal { source }
}

#[async_trait]
impl NewsletterSubscriptionWriter for ZohoNewsletterSubscriptionWriter {
    async fn upsert(
        &self,
        subscription: &NewsletterSubscription,
    ) -> Result<(), NewsletterSubscriptionWriteError> {
        self.subscribe_to_list(subscription).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use localization::Language;
    use money::Currency;
    use url::form_urlencoded;
    use user_core::user_id::UserId;
    use user_core::{first_name::FirstName, last_name::LastName};
    use wiremock::{
        Match, Mock, MockServer, Request, ResponseTemplate,
        matchers::{header, method, path},
    };

    fn subscription() -> NewsletterSubscription {
        NewsletterSubscription::new(
            "ada@example.com"
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            Some(FirstName::from("Ada")),
            Some(LastName::from("Lovelace")),
            Some(Language::En),
            Some(Currency::Eur),
            Some(UserId::new()),
        )
    }

    fn writer(server: &MockServer) -> ZohoNewsletterSubscriptionWriter {
        ZohoNewsletterSubscriptionWriter::new(
            "newsletter-list".into(),
            reqwest::Client::new(),
            "client-id".into(),
            "client-secret".into(),
            "refresh-token".into(),
            server.uri(),
            server.uri(),
        )
    }

    fn oauth_success() -> Mock {
        Mock::given(method("POST"))
            .and(path("/oauth/v2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access-token",
                "expires_in": 3600
            })))
    }

    #[derive(Debug)]
    struct ContactInfoMatcher {
        expected_user_id: String,
    }

    impl Match for ContactInfoMatcher {
        fn matches(&self, request: &Request) -> bool {
            let fields: std::collections::HashMap<_, _> =
                form_urlencoded::parse(request.body.as_slice())
                    .into_owned()
                    .collect();
            let Some(contact_info) = fields.get("contactinfo") else {
                return false;
            };
            let Ok(contact) = serde_json::from_str::<Value>(contact_info) else {
                return false;
            };
            fields.get("resfmt") == Some(&"JSON".to_owned())
                && fields.get("listkey") == Some(&"newsletter-list".to_owned())
                && contact.get("Contact Email") == Some(&Value::String("ada@example.com".into()))
                && contact.get("First Name") == Some(&Value::String("Ada".into()))
                && contact.get("Last Name") == Some(&Value::String("Lovelace".into()))
                && contact.get("language") == Some(&Value::String("en".into()))
                && contact.get("currency") == Some(&Value::String("EUR".into()))
                && contact.get("user_id") == Some(&Value::String(self.expected_user_id.clone()))
        }
    }

    #[tokio::test]
    async fn should_upsert_expected_contact_profile_and_user_fields() {
        let server = MockServer::start().await;
        let subscription = subscription();
        let expected_user_id = subscription
            .user_id()
            .map(|user_id| user_id.to_string())
            .unwrap_or_else(|| panic!("test subscription must have a user ID"));
        oauth_success().mount(&server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .and(header("Authorization", "Zoho-oauthtoken access-token"))
            .and(ContactInfoMatcher { expected_user_id })
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success"
            })))
            .mount(&server)
            .await;

        assert!(writer(&server).upsert(&subscription).await.is_ok());
    }

    #[tokio::test]
    async fn should_return_invalid_email_for_known_numeric_or_string_zoho_codes() {
        for code in [serde_json::json!(2004), serde_json::json!("2007")] {
            let server = MockServer::start().await;
            oauth_success().mount(&server).await;
            Mock::given(method("POST"))
                .and(path("/api/v1.1/json/listsubscribe"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "status": "error",
                    "code": code
                })))
                .mount(&server)
                .await;

            assert!(matches!(
                writer(&server).upsert(&subscription()).await,
                Err(NewsletterSubscriptionWriteError::InvalidEmail)
            ));
        }
    }

    #[tokio::test]
    async fn should_map_token_and_api_failures_to_temporarily_unavailable() {
        let token_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/v2/token"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&token_server)
            .await;
        let token_result = writer(&token_server).upsert(&subscription()).await;

        let api_server = MockServer::start().await;
        oauth_success().mount(&api_server).await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(502))
            .mount(&api_server)
            .await;
        let api_result = writer(&api_server).upsert(&subscription()).await;

        assert!(matches!(
            token_result,
            Err(NewsletterSubscriptionWriteError::TemporarilyUnavailable { .. })
        ));
        assert!(matches!(
            api_result,
            Err(NewsletterSubscriptionWriteError::TemporarilyUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn should_reuse_cached_oauth_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/v2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access-token",
                "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1.1/json/listsubscribe"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "success"
            })))
            .expect(2)
            .mount(&server)
            .await;
        let writer = writer(&server);

        assert!(writer.upsert(&subscription()).await.is_ok());
        assert!(writer.upsert(&subscription()).await.is_ok());
    }
}
