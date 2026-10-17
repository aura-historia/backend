use super::AuthError;
use aws_sdk_cognitoidentityprovider::Client as CognitoClient;
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_USER_INFO_BYTES: usize = 65_536;

#[async_trait::async_trait]
pub trait CognitoTokenStatus: Send + Sync {
    /// Called only after local JWT validation. Successful checks must not be cached.
    async fn verify(&self, token: &str, subject: &str, scopes: &str) -> Result<(), AuthError>;
}

pub struct LiveCognitoTokenStatus {
    client: Client,
    user_info_url: Url,
    cognito: CognitoClient,
}

impl LiveCognitoTokenStatus {
    pub fn new(user_info_url: &str, cognito: CognitoClient) -> Result<Self, AuthError> {
        let url = Url::parse(user_info_url)
            .map_err(|_| AuthError::Internal("invalid Cognito userInfo URL".to_owned()))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(AuthError::Internal("invalid Cognito userInfo URL".to_owned()));
        }
        let client = Client::builder()
            .redirect(Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|_| {
                AuthError::Internal("Cognito HTTP client initialization failed".to_owned())
            })?;
        Ok(Self {
            client,
            user_info_url: url,
            cognito,
        })
    }

    async fn user_info_subject(&self, token: &str) -> Result<String, AuthError> {
        #[derive(Deserialize)]
        struct UserInfo {
            sub: String,
        }
        let mut response = self
            .client
            .get(self.user_info_url.clone())
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| AuthError::TemporarilyUnavailable)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Err(AuthError::InvalidCredentials);
        }
        if response.status() != StatusCode::OK {
            return Err(AuthError::TemporarilyUnavailable);
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| AuthError::TemporarilyUnavailable)?
        {
            if body.len() + chunk.len() > MAX_USER_INFO_BYTES {
                return Err(AuthError::TemporarilyUnavailable);
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice::<UserInfo>(&body)
            .map(|user| user.sub)
            .map_err(|_| AuthError::TemporarilyUnavailable)
    }

    async fn native_subject(&self, token: &str) -> Result<String, AuthError> {
        let user = self
            .cognito
            .get_user()
            .access_token(token)
            .send()
            .await
            .map_err(|error| {
                if error.as_service_error().is_some_and(|error| {
                    error.is_not_authorized_exception()
                        || error.is_user_not_found_exception()
                        || error.is_password_reset_required_exception()
                        || error.is_user_not_confirmed_exception()
                }) {
                    AuthError::InvalidCredentials
                } else {
                    AuthError::TemporarilyUnavailable
                }
            })?;
        user.user_attributes()
            .iter()
            .find(|attribute| attribute.name() == "sub")
            .and_then(|attribute| attribute.value())
            .map(ToOwned::to_owned)
            .ok_or(AuthError::TemporarilyUnavailable)
    }
}

#[async_trait::async_trait]
impl CognitoTokenStatus for LiveCognitoTokenStatus {
    async fn verify(&self, token: &str, subject: &str, scopes: &str) -> Result<(), AuthError> {
        let verification = async {
            let actual_subject = if scopes.split_whitespace().any(|scope| scope == "openid") {
                self.user_info_subject(token).await?
            } else if scopes
                .split_whitespace()
                .any(|scope| scope == "aws.cognito.signin.user.admin")
            {
                self.native_subject(token).await?
            } else {
                return Err(AuthError::InvalidCredentials);
            };
            if actual_subject == subject {
                Ok(())
            } else {
                Err(AuthError::InvalidCredentials)
            }
        };
        tokio::time::timeout(REQUEST_TIMEOUT, verification)
            .await
            .map_err(|_| AuthError::TemporarilyUnavailable)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_sdk_cognitoidentityprovider::config::{Credentials, Region, retry::RetryConfig};
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path},
    };

    fn verifier(server: &MockServer) -> LiveCognitoTokenStatus {
        let cognito = CognitoClient::from_conf(
            aws_sdk_cognitoidentityprovider::config::Builder::new()
                .behavior_version_latest()
                .credentials_provider(Credentials::new("test", "test", None, None, "offline-fixture"))
                .region(Region::new("eu-central-1"))
                .endpoint_url(server.uri())
                .retry_config(RetryConfig::disabled())
                .build(),
        );
        let mut verifier =
            LiveCognitoTokenStatus::new("https://auth.test/oauth2/userInfo", cognito).unwrap();
        // Only tests replace trusted HTTPS egress with an isolated loopback server.
        verifier.user_info_url = Url::parse(&format!("{}/oauth2/userInfo", server.uri())).unwrap();
        verifier
    }

    #[tokio::test]
    async fn should_validate_hosted_tokens_and_check_every_request() {
        let server = MockServer::start().await;
        let verifier = verifier(&server);
        Mock::given(method("GET"))
            .and(path("/oauth2/userInfo"))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"sub": "subject"})))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            verifier
                .verify("test-token", "subject", "openid email")
                .await
                .is_ok()
        );
        server.verify().await;
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            verifier
                .verify("test-token", "subject", "openid email")
                .await,
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn should_validate_native_tokens_and_reject_revocation() {
        let server = MockServer::start().await;
        let verifier = verifier(&server);
        Mock::given(method("POST"))
            .and(header("x-amz-target", "AWSCognitoIdentityProviderService.GetUser"))
            .and(body_json(json!({"AccessToken": "test-token"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "Username": "username",
                "UserAttributes": [{"Name": "sub", "Value": "subject"}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert!(
            verifier
                .verify("test-token", "subject", "aws.cognito.signin.user.admin")
                .await
                .is_ok()
        );
        server.verify().await;
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "__type": "NotAuthorizedException", "message": "Access Token has been revoked"
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert!(matches!(
            verifier
                .verify("test-token", "subject", "aws.cognito.signin.user.admin")
                .await,
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn should_fail_closed_for_mismatched_subjects_and_unavailable_provider() {
        for (status, body, invalid) in [
            (200, json!({"sub": "other-subject"}).to_string(), true),
            (200, "not-json".to_owned(), false),
            (200, json!({"email": "unused@test"}).to_string(), false),
            (200, "x".repeat(MAX_USER_INFO_BYTES + 1), false),
            (429, "".to_owned(), false),
            (500, "".to_owned(), false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(status).set_body_string(body))
                .expect(1)
                .mount(&server)
            .await;
            let result = verifier(&server)
                .verify("test-token", "subject", "openid")
                .await;
            if invalid {
                assert!(matches!(result, Err(AuthError::InvalidCredentials)));
            } else {
                assert!(matches!(result, Err(AuthError::TemporarilyUnavailable)));
            }
        }
    }

    #[tokio::test]
    async fn should_fail_closed_for_native_subject_mismatch_and_provider_failures() {
        for (status, body, invalid) in [
            (200, json!({"Username": "user", "UserAttributes": [{"Name": "sub", "Value": "other"}]}), true),
            (200, json!({"Username": "user", "UserAttributes": []}), false),
            (400, json!({"__type": "TooManyRequestsException"}), false),
            (500, json!({"__type": "InternalErrorException"}), false),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(ResponseTemplate::new(status).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let result = verifier(&server)
                .verify("test-token", "subject", "aws.cognito.signin.user.admin")
                .await;
            if invalid {
                assert!(matches!(result, Err(AuthError::InvalidCredentials)));
            } else {
                assert!(matches!(result, Err(AuthError::TemporarilyUnavailable)));
            }
        }
    }

    #[tokio::test]
    async fn should_not_follow_redirects_or_accept_unsupported_scopes() {
        let server = MockServer::start().await;
        let destination = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", destination.uri()))
            .expect(1)
            .mount(&server)
            .await;
        let verifier = verifier(&server);
        assert!(matches!(
            verifier.verify("test-token", "subject", "openid").await,
            Err(AuthError::TemporarilyUnavailable)
        ));
        assert!(destination.received_requests().await.unwrap().is_empty());
        assert!(matches!(
            verifier
                .verify("test-token", "subject", "email profile")
                .await,
            Err(AuthError::InvalidCredentials)
        ));
    }

    #[tokio::test]
    async fn should_reject_unsafe_user_info_urls() {
        let server = MockServer::start().await;
        let cognito = verifier(&server).cognito;
        for url in [
            "http://auth.test",
            "https://user:password@auth.test",
            "https://auth.test?token=x",
        ] {
            assert!(LiveCognitoTokenStatus::new(url, cognito.clone()).is_err());
        }
    }
}
