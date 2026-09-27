use crate::config::LoopsNewsletterConfig;
use application::error::box_error;
use async_trait::async_trait;
use reqwest::{StatusCode, header::AUTHORIZATION};
use serde::Deserialize;
use serde_json::{Map, Value};
use user_core::newsletter_subscription::NewsletterSubscription;
use user_service::ports::{NewsletterSubscriptionWriteError, NewsletterSubscriptionWriter};

const MAX_RESPONSE_BYTES: usize = 16 * 1024;
const SOURCE: &str = "aura-historia-newsletter-api";

#[derive(Debug, thiserror::Error)]
#[error("Loops newsletter {kind}; HTTP status {status:?}")]
struct LoopsFailure {
    kind: &'static str,
    status: Option<u16>,
}

fn internal(kind: &'static str, status: Option<u16>) -> NewsletterSubscriptionWriteError {
    NewsletterSubscriptionWriteError::Internal {
        source: box_error(LoopsFailure { kind, status }),
    }
}

fn unavailable(kind: &'static str, status: Option<u16>) -> NewsletterSubscriptionWriteError {
    NewsletterSubscriptionWriteError::TemporarilyUnavailable {
        source: box_error(LoopsFailure { kind, status }),
    }
}

fn transport_error(error: reqwest::Error) -> NewsletterSubscriptionWriteError {
    if error.is_builder() {
        internal("request construction failed", None)
    } else if error.is_timeout() {
        unavailable("request timed out", None)
    } else {
        unavailable("transport failed", None)
    }
}

#[derive(Deserialize)]
struct SuccessResponse {
    success: bool,
    id: String,
}

#[derive(Deserialize)]
struct RejectionResponse {
    success: bool,
    message: String,
}

pub struct LoopsNewsletterSubscriptionWriter {
    config: LoopsNewsletterConfig,
    client: reqwest::Client,
}

impl LoopsNewsletterSubscriptionWriter {
    pub fn new(config: LoopsNewsletterConfig, client: reqwest::Client) -> Self {
        Self { config, client }
    }

    fn payload(&self, subscription: &NewsletterSubscription) -> Value {
        let mut fields = Map::new();
        fields.insert(
            "email".into(),
            Value::String(subscription.email().to_string()),
        );
        fields.insert("source".into(), Value::String(SOURCE.into()));
        if let Some(value) = subscription.first_name() {
            fields.insert("firstName".into(), Value::String(value.to_string()));
        }
        if let Some(value) = subscription.last_name() {
            fields.insert("lastName".into(), Value::String(value.to_string()));
        }
        if let Some(value) = subscription.language() {
            fields.insert("language".into(), Value::String(value.as_str().into()));
        }
        if let Some(value) = subscription.currency() {
            fields.insert("currency".into(), Value::String(value.as_str().into()));
        }
        if let Some(value) = subscription.user_id() {
            fields.insert("auraUserId".into(), Value::String(value.to_string()));
        }
        let mut lists = Map::new();
        lists.insert(self.config.newsletter_list_id.clone(), Value::Bool(true));
        fields.insert("mailingLists".into(), Value::Object(lists));

        Value::Object(fields)
    }
}

async fn read_bounded_body(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, NewsletterSubscriptionWriteError> {
    let status = Some(response.status().as_u16());
    if response
        .content_length()
        .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
    {
        return Err(internal("response body too large", status));
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
            return Err(internal("response body too large", status));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[async_trait]
impl NewsletterSubscriptionWriter for LoopsNewsletterSubscriptionWriter {
    async fn upsert(
        &self,
        subscription: &NewsletterSubscription,
    ) -> Result<(), NewsletterSubscriptionWriteError> {
        let response = self
            .client
            .put(self.config.update_url.clone())
            .header(AUTHORIZATION, self.config.authorization.clone())
            .json(&self.payload(subscription))
            .send()
            .await
            .map_err(transport_error)?;
        let status = response.status();
        let code = Some(status.as_u16());

        if status == StatusCode::REQUEST_TIMEOUT
            || status == StatusCode::TOO_MANY_REQUESTS
            || status.is_server_error()
        {
            return Err(unavailable("provider unavailable", code));
        }
        if status != StatusCode::OK && status != StatusCode::BAD_REQUEST {
            return Err(internal("unexpected provider status", code));
        }

        let body = read_bounded_body(response).await?;
        if status == StatusCode::BAD_REQUEST {
            let rejected: RejectionResponse = serde_json::from_slice(&body)
                .map_err(|_| internal("invalid rejection response", code))?;
            if !rejected.success && rejected.message == "Invalid email address." {
                return Err(NewsletterSubscriptionWriteError::InvalidEmail);
            }
            return Err(internal("provider rejected request", code));
        }

        let accepted: SuccessResponse = serde_json::from_slice(&body)
            .map_err(|_| internal("invalid success response", code))?;
        if !accepted.success || accepted.id.trim().is_empty() {
            return Err(internal("provider did not confirm success", code));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LoopsNewsletterConfig;
    use localization::Language;
    use money::Currency;
    use serde_json::json;
    use std::{error::Error, time::Duration};
    use user_core::{first_name::FirstName, last_name::LastName, user_id::UserId};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{body_json, header, method, path},
    };

    fn minimal_subscription() -> NewsletterSubscription {
        NewsletterSubscription::new(
            "collector@example.com"
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            None,
            None,
            None,
            None,
            None,
        )
    }

    fn full_subscription(email: &str, user_id: UserId) -> NewsletterSubscription {
        NewsletterSubscription::new(
            email
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            Some(FirstName::from("Ada")),
            Some(LastName::from("Lovelace")),
            Some(Language::En),
            Some(Currency::Eur),
            Some(user_id),
        )
    }

    fn writer_with_client(
        base_url: &str,
        client: reqwest::Client,
    ) -> LoopsNewsletterSubscriptionWriter {
        let config = LoopsNewsletterConfig::new(
            "test-api-key".into(),
            "test-newsletter-list".into(),
            base_url.into(),
        )
        .unwrap_or_else(|error| panic!("invalid test config: {error}"));
        LoopsNewsletterSubscriptionWriter::new(config, client)
    }

    fn writer(server: &MockServer) -> LoopsNewsletterSubscriptionWriter {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|error| panic!("test client failed: {error}"));
        writer_with_client(&format!("{}/api", server.uri()), client)
    }

    fn successful_response() -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "success": true,
            "id": "test-contact-id"
        }))
    }

    #[tokio::test]
    async fn should_send_minimal_email_keyed_upsert_without_resetting_opt_out() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(header("Authorization", "Bearer test-api-key"))
            .and(header("Content-Type", "application/json"))
            .and(body_json(json!({
                "email": "collector@example.com",
                "source": SOURCE,
                "mailingLists": {"test-newsletter-list": true}
            })))
            .respond_with(successful_response())
            .expect(1)
            .mount(&server)
            .await;

        assert!(
            writer(&server)
                .upsert(&minimal_subscription())
                .await
                .is_ok()
        );
        let requests = server
            .received_requests()
            .await
            .unwrap_or_else(|| panic!("request recording unavailable"));
        assert_eq!(1, requests.len());
        let payload: Value = serde_json::from_slice(&requests[0].body)
            .unwrap_or_else(|error| panic!("invalid captured JSON: {error}"));
        for absent in [
            "subscribed",
            "userId",
            "auraUserId",
            "firstName",
            "lastName",
            "language",
            "currency",
            "doubleOptIn",
            "optInStatus",
            "formId",
            "eventName",
        ] {
            assert!(payload.get(absent).is_none(), "unexpected field: {absent}");
        }
        assert!(
            payload
                .as_object()
                .is_some_and(|fields| { fields.keys().all(|key| !key.starts_with("__")) })
        );
    }

    #[tokio::test]
    async fn should_map_full_contact_metadata_without_using_loops_user_id() {
        let server = MockServer::start().await;
        let user_id = UserId::new();
        let subscription = full_subscription("collector@example.com", user_id);
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(body_json(json!({
                "email": "collector@example.com",
                "firstName": "Ada",
                "lastName": "Lovelace",
                "language": "en",
                "currency": "EUR",
                "auraUserId": user_id.to_string(),
                "source": SOURCE,
                "mailingLists": {"test-newsletter-list": true}
            })))
            .respond_with(successful_response())
            .expect(1)
            .mount(&server)
            .await;

        writer(&server)
            .upsert(&subscription)
            .await
            .unwrap_or_else(|error| panic!("Loops upsert failed: {error}"));
        let requests = server.received_requests().await.expect("request recording");
        let payload: Value = serde_json::from_slice(&requests[0].body).expect("captured JSON");
        assert!(payload.get("userId").is_none());
        assert_eq!(
            json!({"test-newsletter-list": true}),
            payload["mailingLists"]
        );
    }

    #[tokio::test]
    async fn should_keep_distinct_emails_for_one_aura_user() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(successful_response())
            .expect(2)
            .mount(&server)
            .await;
        let writer = writer(&server);
        let user_id = UserId::new();

        writer
            .upsert(&full_subscription("first@example.com", user_id))
            .await
            .expect("first write");
        writer
            .upsert(&full_subscription("second@example.com", user_id))
            .await
            .expect("second write");

        let requests = server.received_requests().await.expect("request recording");
        assert_eq!(2, requests.len());
        for (request, email) in requests
            .iter()
            .zip(["first@example.com", "second@example.com"])
        {
            let payload: Value = serde_json::from_slice(&request.body).expect("captured JSON");
            assert_eq!(email, payload["email"]);
            assert_eq!(user_id.to_string(), payload["auraUserId"]);
            assert!(payload.get("userId").is_none());
        }
    }

    #[tokio::test]
    async fn should_not_clear_profile_values_or_restore_subscription_on_anonymous_repeat() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .respond_with(successful_response())
            .expect(2)
            .mount(&server)
            .await;
        let writer = writer(&server);
        writer
            .upsert(&full_subscription("collector@example.com", UserId::new()))
            .await
            .expect("enriched write");
        writer
            .upsert(&minimal_subscription())
            .await
            .expect("anonymous write");

        let requests = server.received_requests().await.expect("request recording");
        let enriched: Value = serde_json::from_slice(&requests[0].body).expect("enriched JSON");
        let anonymous: Value = serde_json::from_slice(&requests[1].body).expect("minimal JSON");
        for field in [
            "firstName",
            "lastName",
            "language",
            "currency",
            "auraUserId",
        ] {
            assert!(enriched.get(field).is_some());
            assert!(anonymous.get(field).is_none());
        }
        for payload in [enriched, anonymous] {
            assert!(payload.get("subscribed").is_none());
        }
    }

    #[tokio::test]
    async fn should_use_only_one_put_to_the_configured_update_endpoint_per_call() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/api/v1/contacts/update"))
            .and(header("Authorization", "Bearer test-api-key"))
            .respond_with(successful_response())
            .expect(2)
            .mount(&server)
            .await;
        let writer = writer(&server);

        writer
            .upsert(&minimal_subscription())
            .await
            .expect("first write");
        writer
            .upsert(&minimal_subscription())
            .await
            .expect("repeated write");

        let requests = server.received_requests().await.expect("request recording");
        assert_eq!(2, requests.len());
        assert!(requests.iter().all(|request| request.method == "PUT"));
        assert!(requests.iter().all(|request| {
            serde_json::from_slice::<Value>(&request.body)
                .is_ok_and(|payload| payload.get("events").is_none())
        }));
    }

    #[tokio::test]
    async fn should_append_endpoint_once_for_base_urls_with_or_without_trailing_slash() {
        for base_suffix in ["/api", "/api/"] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .and(path("/api/v1/contacts/update"))
                .respond_with(successful_response())
                .expect(1)
                .mount(&server)
                .await;
            let writer = writer_with_client(
                &format!("{}{base_suffix}", server.uri()),
                reqwest::Client::new(),
            );
            writer
                .upsert(&minimal_subscription())
                .await
                .unwrap_or_else(|error| panic!("Loops request failed: {error}"));
        }
    }

    #[tokio::test]
    async fn should_accept_only_a_confirmed_nonempty_success_response() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(successful_response())
            .mount(&server)
            .await;

        assert!(
            writer(&server)
                .upsert(&minimal_subscription())
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn should_reject_malformed_or_unconfirmed_success_responses() {
        for body in [
            "{}",
            r#"{"success":true}"#,
            r#"{"success":true,"id":null}"#,
            r#"{"success":true,"id":42}"#,
            r#"{"success":true,"id":""}"#,
            r#"{"success":true,"id":"   "}"#,
            r#"{"success":false,"id":"contact-id"}"#,
            "not-json",
            "",
        ] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .expect(1)
                .mount(&server)
                .await;
            let result = writer(&server).upsert(&minimal_subscription()).await;
            assert!(
                matches!(
                    result,
                    Err(NewsletterSubscriptionWriteError::Internal { .. })
                ),
                "unexpected result for body {body:?}"
            );
        }
    }

    #[tokio::test]
    async fn should_classify_only_the_specific_invalid_email_response_as_invalid_email() {
        for (body, is_invalid_email) in [
            (
                json!({"success": false, "message": "Invalid email address."}),
                true,
            ),
            (
                json!({"success": false, "message": "Unknown contact property emailPreference"}),
                false,
            ),
            (
                json!({"success": false, "message": "Invalid mailing list"}),
                false,
            ),
            (
                json!({"success": true, "message": "Invalid email address."}),
                false,
            ),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .respond_with(ResponseTemplate::new(400).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
            let result = writer(&server).upsert(&minimal_subscription()).await;
            if is_invalid_email {
                assert!(matches!(
                    result,
                    Err(NewsletterSubscriptionWriteError::InvalidEmail)
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(NewsletterSubscriptionWriteError::Internal { .. })
                ));
            }
        }
    }

    #[tokio::test]
    async fn should_map_provider_auth_and_configuration_failures_to_internal() {
        for status in [401, 403] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(&server)
                .await;
            assert!(matches!(
                writer(&server).upsert(&minimal_subscription()).await,
                Err(NewsletterSubscriptionWriteError::Internal { .. })
            ));
        }

        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "success": false,
                "message": "Unknown contact property language"
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            writer(&server).upsert(&minimal_subscription()).await,
            Err(NewsletterSubscriptionWriteError::Internal { .. })
        ));
    }

    #[tokio::test]
    async fn should_return_transient_http_statuses_as_temporarily_unavailable() {
        for status in [408, 429, 500, 502, 503, 504] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(&server)
                .await;

            assert!(
                matches!(
                    writer(&server).upsert(&minimal_subscription()).await,
                    Err(NewsletterSubscriptionWriteError::TemporarilyUnavailable { .. })
                ),
                "HTTP {status}"
            );
        }
    }

    #[tokio::test]
    async fn should_reject_unexpected_success_and_redirect_statuses() {
        for status in [201, 204, 302] {
            let server = MockServer::start().await;
            Mock::given(method("PUT"))
                .respond_with(ResponseTemplate::new(status))
                .expect(1)
                .mount(&server)
                .await;

            assert!(
                matches!(
                    writer(&server).upsert(&minimal_subscription()).await,
                    Err(NewsletterSubscriptionWriteError::Internal { .. })
                ),
                "HTTP {status}"
            );
        }
    }

    #[tokio::test]
    async fn should_not_follow_redirects_with_authorization_or_contact_data() {
        let target = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(successful_response())
            .expect(0)
            .mount(&target)
            .await;
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", format!("{}/steal", target.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;

        assert!(matches!(
            writer(&server).upsert(&minimal_subscription()).await,
            Err(NewsletterSubscriptionWriteError::Internal { .. })
        ));
        let requests = target
            .received_requests()
            .await
            .expect("target request recording");
        assert!(requests.is_empty());
    }

    #[tokio::test]
    async fn should_map_request_timeouts_to_temporarily_unavailable() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(150))
                    .set_body_json(json!({"success": true, "id": "contact-id"})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(25))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("test client");

        assert!(matches!(
            writer_with_client(&format!("{}/api", server.uri()), client)
                .upsert(&minimal_subscription())
                .await,
            Err(NewsletterSubscriptionWriteError::TemporarilyUnavailable { .. })
        ));
    }

    #[tokio::test]
    async fn should_reject_advertised_oversized_response_bodies() {
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(
                ResponseTemplate::new(200).set_body_bytes(vec![b'x'; MAX_RESPONSE_BYTES + 1]),
            )
            .expect(1)
            .mount(&server)
            .await;

        assert!(matches!(
            writer(&server).upsert(&minimal_subscription()).await,
            Err(NewsletterSubscriptionWriteError::Internal { .. })
        ));
    }

    #[tokio::test]
    async fn should_reject_streamed_oversized_response_without_content_length() {
        use tokio::{io::AsyncReadExt, io::AsyncWriteExt, net::TcpListener};

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let address = listener.local_addr().expect("test listener address");
        let response_body = vec![b'x'; MAX_RESPONSE_BYTES + 1];
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await
                .expect("write response headers");
            for chunk in response_body.chunks(8 * 1024) {
                let header = format!("{:X}\r\n", chunk.len());
                stream
                    .write_all(header.as_bytes())
                    .await
                    .expect("write chunk size");
                stream.write_all(chunk).await.expect("write chunk body");
                stream
                    .write_all(b"\r\n")
                    .await
                    .expect("write chunk terminator");
            }
            stream
                .write_all(b"0\r\n\r\n")
                .await
                .expect("finish response");
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("test client");

        assert!(matches!(
            writer_with_client(&format!("http://{address}/api"), client)
                .upsert(&minimal_subscription())
                .await,
            Err(NewsletterSubscriptionWriteError::Internal { .. })
        ));
    }

    #[tokio::test]
    async fn constructors_make_no_http_requests() {
        let server = MockServer::start().await;
        let config = LoopsNewsletterConfig::new(
            "test-api-key".into(),
            "test-newsletter-list".into(),
            format!("{}/api", server.uri()),
        )
        .expect("valid config");
        let writer = LoopsNewsletterSubscriptionWriter::new(config, reqwest::Client::new());

        assert!(
            server
                .received_requests()
                .await
                .expect("request recording")
                .is_empty()
        );
        drop(writer);
    }

    #[tokio::test]
    async fn safe_error_chain_does_not_reveal_credentials_contacts_or_provider_bodies() {
        let api_key = "credential-sentinel";
        let email_sentinel = "private-contact-sentinel@example.com";
        let body_sentinel = "provider-body-sentinel";
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .respond_with(ResponseTemplate::new(500).set_body_string(body_sentinel))
            .expect(1)
            .mount(&server)
            .await;
        let config = LoopsNewsletterConfig::new(
            api_key.into(),
            "test-newsletter-list".into(),
            format!("{}/api", server.uri()),
        )
        .expect("valid config");
        let writer = LoopsNewsletterSubscriptionWriter::new(config, reqwest::Client::new());
        let subscription = NewsletterSubscription::new(
            email_sentinel
                .try_into()
                .unwrap_or_else(|error| panic!("invalid test email: {error}")),
            None,
            None,
            None,
            None,
            None,
        );
        let error = writer
            .upsert(&subscription)
            .await
            .expect_err("provider failure expected");
        let mut rendered = format!("{error}\n{error:?}");
        let mut source = Error::source(&error);
        while let Some(error) = source {
            rendered.push_str(&error.to_string());
            rendered.push_str(&format!("{error:?}"));
            source = error.source();
        }

        for sentinel in [api_key, email_sentinel, body_sentinel] {
            assert!(!rendered.contains(sentinel), "leaked {sentinel}");
        }
    }
}
