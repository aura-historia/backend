//! Exercises the real S3 and SES SDKs with only their HTTP connectors replaced; no AWS calls.
use aws_sdk_s3::config::{Credentials as S3Credentials, Region as S3Region};
use aws_sdk_sesv2::config::{
    Credentials as SesCredentials, Region as SesRegion, retry::RetryConfig, timeout::TimeoutConfig,
};
use aws_smithy_runtime_api::{
    client::{
        http::{
            HttpClient, HttpConnector, HttpConnectorFuture, HttpConnectorSettings,
            SharedHttpConnector,
        },
        orchestrator::{HttpRequest, HttpResponse},
        result::ConnectorError,
        runtime_components::RuntimeComponents,
    },
    shared::IntoShared,
};
use aws_smithy_types::body::SdkBody;
use localization::Language;
use serde_email::Email;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use user_core::{
    first_name::FirstName, newsletter_confirmation::RawNewsletterConfirmationToken,
    newsletter_confirmation_id::NewsletterConfirmationId,
};
use user_email_aws::{NewsletterConfirmationEmailConfig, SesNewsletterConfirmationEmailSender};
use user_service::ports::{
    NewsletterConfirmationEmail, NewsletterConfirmationEmailRetryability,
    NewsletterConfirmationEmailSendOutcome, NewsletterConfirmationEmailSender, NewsletterProfile,
};

const TEMPLATE_KEY: &str = "ephemeral/test-commit/mjml/newsletter/confirmation/en.html";
const TEMPLATE: &str = "<html><body>{{#if first_name}}Hello {{first_name}}{{else}}Hello{{/if}}<a href=\"{{confirmation_url}}\">Confirm subscription</a></body></html>";

#[derive(Clone, Copy, Debug)]
enum Service {
    S3,
    Ses,
}

#[derive(Debug)]
enum Reply {
    Response(u16, &'static str),
    OversizedTemplate,
    IoFailure,
    Hang,
}

#[derive(Debug)]
struct State {
    replies: VecDeque<Reply>,
    requests: Vec<Value>,
    paths: Vec<String>,
}

#[derive(Clone, Debug)]
struct ReplayHttp {
    service: Service,
    state: Arc<Mutex<State>>,
}

impl ReplayHttp {
    fn new(service: Service, replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            service,
            state: Arc::new(Mutex::new(State {
                replies: replies.into_iter().collect(),
                requests: Vec::new(),
                paths: Vec::new(),
            })),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }

    fn paths(&self) -> Vec<String> {
        self.state.lock().unwrap().paths.clone()
    }
}

impl HttpConnector for ReplayHttp {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        assert!(request.headers().get("authorization").is_some());
        let body = match self.service {
            Service::S3 => {
                assert_eq!(request.method(), "GET");
                assert!(request.uri().to_string().contains(TEMPLATE_KEY));
                Value::Null
            }
            Service::Ses => {
                assert_eq!(request.method(), "POST");
                assert!(
                    request
                        .uri()
                        .to_string()
                        .contains("/v2/email/outbound-emails")
                );
                serde_json::from_slice(request.body().bytes().expect("SES JSON request body"))
                    .expect("valid SES JSON request")
            }
        };
        let mut state = self.state.lock().unwrap();
        state.paths.push(request.uri().to_string());
        state.requests.push(body);
        let reply = state.replies.pop_front().expect("unexpected SDK retry");
        drop(state);
        match reply {
            Reply::Response(status, body) => {
                let response: HttpResponse = http::Response::builder()
                    .status(status)
                    .header(
                        "content-type",
                        match self.service {
                            Service::S3 => "text/html",
                            Service::Ses => "application/json",
                        },
                    )
                    .body(SdkBody::from(body.to_owned()))
                    .unwrap()
                    .try_into()
                    .unwrap();
                HttpConnectorFuture::ready(Ok(response))
            }
            Reply::OversizedTemplate => {
                let response: HttpResponse = http::Response::builder()
                    .status(200)
                    .header("content-type", "text/html")
                    .body(SdkBody::from("x".repeat(128 * 1024 + 1)))
                    .unwrap()
                    .try_into()
                    .unwrap();
                HttpConnectorFuture::ready(Ok(response))
            }
            Reply::IoFailure => HttpConnectorFuture::ready(Err(ConnectorError::io(
                std::io::Error::other("lost SES response").into(),
            ))),
            Reply::Hang => HttpConnectorFuture::new(async {
                std::future::pending::<Result<HttpResponse, ConnectorError>>().await
            }),
        }
    }
}

impl HttpClient for ReplayHttp {
    fn http_connector(
        &self,
        _: &HttpConnectorSettings,
        _: &RuntimeComponents,
    ) -> SharedHttpConnector {
        self.clone().into_shared()
    }
}

fn sender(s3_http: ReplayHttp, ses_http: ReplayHttp) -> SesNewsletterConfirmationEmailSender {
    sender_with_ses_timeout(s3_http, ses_http, Some(Duration::from_millis(100)))
}

fn sender_with_ses_timeout(
    s3_http: ReplayHttp,
    ses_http: ReplayHttp,
    sdk_timeout: Option<Duration>,
) -> SesNewsletterConfirmationEmailSender {
    let s3 = aws_sdk_s3::Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version_latest()
            .region(S3Region::new("eu-central-1"))
            .credentials_provider(S3Credentials::new("test", "test", None, None, "sdk-test"))
            .force_path_style(true)
            .http_client(s3_http)
            .build(),
    );
    // The injected client deliberately permits retries. SendEmail has no idempotency key;
    // the sender must override this to one attempt before making a network call.
    let mut ses_config = aws_sdk_sesv2::Config::builder()
        .behavior_version_latest()
        .region(SesRegion::new("eu-central-1"))
        .credentials_provider(SesCredentials::new("test", "test", None, None, "sdk-test"))
        .http_client(ses_http)
        .retry_config(RetryConfig::standard().with_max_attempts(3));
    if let Some(timeout) = sdk_timeout {
        ses_config =
            ses_config.timeout_config(TimeoutConfig::builder().operation_timeout(timeout).build());
    }
    let ses = aws_sdk_sesv2::Client::from_conf(ses_config.build());
    let config = NewsletterConfirmationEmailConfig::new(
        "test-templates",
        "sender@example.test",
        "reply@example.test",
        "ephemeral",
        "test-commit",
        "http://127.0.0.1:3000",
    )
    .unwrap_or_else(|_| panic!("valid newsletter email config"));
    SesNewsletterConfirmationEmailSender::new(s3, ses, config)
}

fn email(first_name: Option<&str>) -> NewsletterConfirmationEmail {
    NewsletterConfirmationEmail {
        confirmation_id: NewsletterConfirmationId::new(),
        recipient: Email::try_from("reader@example.test").unwrap(),
        token: RawNewsletterConfirmationToken::from_entropy([0x5a; 32]),
        profile: NewsletterProfile {
            first_name: first_name.map(FirstName::from),
            language: Some(Language::En),
            ..NewsletterProfile::default()
        },
    }
}

fn template_http() -> ReplayHttp {
    ReplayHttp::new(Service::S3, [Reply::Response(200, TEMPLATE)])
}

#[tokio::test]
async fn sends_signed_ses_payload_with_confirmation_link_and_accepts_receipt() {
    let s3_http = template_http();
    let ses_http = ReplayHttp::new(
        Service::Ses,
        [Reply::Response(200, r#"{"MessageId":"ses-receipt-123"}"#)],
    );
    let email = email(Some("Ada & Co"));
    let token = email.token.as_str().to_owned();
    let confirmation_id = email.confirmation_id.to_string();
    let outcome = sender(s3_http.clone(), ses_http.clone()).send(email).await;
    assert_eq!(outcome, NewsletterConfirmationEmailSendOutcome::Accepted);
    assert_eq!(s3_http.paths().len(), 1);
    assert_eq!(ses_http.requests().len(), 1);
    let requests = ses_http.requests();
    let request = &requests[0];
    assert_eq!(
        request["Destination"]["ToAddresses"],
        json!(["reader@example.test"])
    );
    assert_eq!(request["FromEmailAddress"], "sender@example.test");
    assert_eq!(request["ReplyToAddresses"], json!(["reply@example.test"]));
    assert!(
        request["Content"]["Simple"]["Subject"]["Data"]
            .as_str()
            .is_some_and(|subject| subject.contains("Aura Historia"))
    );
    let html = request["Content"]["Simple"]["Body"]["Html"]["Data"]
        .as_str()
        .expect("HTML body");
    assert!(
        html.contains("Ada &amp; Co"),
        "first name must be HTML-escaped"
    );
    assert!(html.contains("http://127.0.0.1:3000"));
    assert!(
        !html.contains(&confirmation_id),
        "the link must not expose the challenge ID"
    );
    assert!(html.contains(&token));
    assert!(html.contains("Confirm subscription"));
    assert!(request.get("EmailTags").is_none());
    let url = html
        .split("href=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    let decoded = html_escape::decode_html_entities(url);
    let link = url::Url::parse(&decoded).expect("valid decoded confirmation URL");
    assert_eq!(link.path(), "/en/newsletter/confirm");
    assert_eq!(link.fragment(), Some(format!("token={token}").as_str()));
    assert_eq!(link.query(), None);
    assert!(
        !html.contains("{{"),
        "all template expressions must be rendered"
    );
}

#[tokio::test]
async fn missing_oversized_and_invalid_templates_are_rejected_before_ses() {
    for reply in [
        Reply::Response(404, "<Error><Code>NoSuchKey</Code></Error>"),
        Reply::OversizedTemplate,
        Reply::Response(200, "{{#if invalid"),
        Reply::Response(200, "<html>{{{first_name}}}{{confirmation_url}}</html>"),
    ] {
        let ses_http = ReplayHttp::new(Service::Ses, []);
        let s3_http = ReplayHttp::new(Service::S3, [reply]);
        assert_eq!(
            sender(s3_http.clone(), ses_http.clone())
                .send(email(None))
                .await,
            NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
                retryability: NewsletterConfirmationEmailRetryability::NotRetryable,
            }
        );
        assert_eq!(s3_http.paths().len(), 1);
        assert!(ses_http.requests().is_empty());
    }
}

#[tokio::test]
async fn unavailable_templates_are_retryable_without_sending_email() {
    let s3_http = ReplayHttp::new(
        Service::S3,
        (0..3).map(|_| Reply::Response(503, "<Error><Code>ServiceUnavailable</Code></Error>")),
    );
    let ses_http = ReplayHttp::new(Service::Ses, []);
    assert_eq!(
        sender(s3_http.clone(), ses_http.clone())
            .send(email(None))
            .await,
        NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
            retryability: NewsletterConfirmationEmailRetryability::Retryable,
        }
    );
    assert!(!s3_http.paths().is_empty());
    assert!(ses_http.requests().is_empty());
}

#[tokio::test]
async fn success_without_a_nonempty_ses_receipt_has_unknown_acceptance() {
    for response in [r#"{}"#, r#"{"MessageId":""}"#] {
        let ses_http = ReplayHttp::new(Service::Ses, [Reply::Response(200, response)]);
        let outcome = sender(template_http(), ses_http.clone())
            .send(email(None))
            .await;
        assert_eq!(
            outcome,
            NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
        );
        assert_eq!(ses_http.requests().len(), 1);
    }
}

#[tokio::test]
async fn modeled_message_rejection_is_definite() {
    let ses_http = ReplayHttp::new(
        Service::Ses,
        [Reply::Response(
            400,
            r#"{"__type":"MessageRejected","message":"rejected"}"#,
        )],
    );
    let outcome = sender(template_http(), ses_http.clone())
        .send(email(None))
        .await;
    assert_eq!(
        outcome,
        NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
            retryability: NewsletterConfirmationEmailRetryability::NotRetryable,
        }
    );
    assert_eq!(ses_http.requests().len(), 1);
}

#[tokio::test]
async fn modeled_and_unmodeled_throttling_are_retryable_definite_rejections() {
    for reply in [
        Reply::Response(
            500,
            r#"{"__type":"TooManyRequestsException","message":"throttled"}"#,
        ),
        Reply::Response(
            500,
            r#"{"__type":"LimitExceededException","message":"limited"}"#,
        ),
        Reply::Response(429, r#"{"message":"rate limited"}"#),
    ] {
        let ses_http = ReplayHttp::new(Service::Ses, [reply]);
        assert_eq!(
            sender(template_http(), ses_http.clone())
                .send(email(None))
                .await,
            NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
                retryability: NewsletterConfirmationEmailRetryability::Retryable,
            }
        );
        assert_eq!(ses_http.requests().len(), 1);
    }
}

#[tokio::test]
async fn ambiguous_transport_failure_does_not_trigger_hidden_ses_sdk_retry() {
    let ses_http = ReplayHttp::new(
        Service::Ses,
        [
            Reply::IoFailure,
            Reply::Response(200, r#"{"MessageId":"second-send"}"#),
        ],
    );
    let outcome = sender(template_http(), ses_http.clone())
        .send(email(None))
        .await;
    assert_eq!(
        outcome,
        NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
    );
    assert_eq!(
        ses_http.requests().len(),
        1,
        "SES must never resend invisibly"
    );
}

#[tokio::test]
async fn stalled_ses_response_times_out_with_unknown_acceptance_and_no_resend() {
    let ses_http = ReplayHttp::new(Service::Ses, [Reply::Hang]);
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        sender(template_http(), ses_http.clone()).send(email(None)),
    )
    .await
    .expect("SDK operation timeout must bound the stalled request");
    assert_eq!(
        outcome,
        NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
    );
    assert_eq!(ses_http.requests().len(), 1);
}

#[tokio::test]
async fn stalled_ses_request_is_bounded_by_adapter_deadline() {
    let ses_http = ReplayHttp::new(Service::Ses, [Reply::Hang]);
    let start = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        sender_with_ses_timeout(template_http(), ses_http.clone(), None).send(email(None)),
    )
    .await
    .expect("adapter SES deadline must bound the stalled request");
    assert_eq!(
        outcome,
        NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
    );
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert_eq!(ses_http.requests().len(), 1);
}

#[tokio::test]
async fn stalled_s3_request_is_bounded_by_adapter_deadline_without_ses_send() {
    let s3_http = ReplayHttp::new(Service::S3, [Reply::Hang]);
    let ses_http = ReplayHttp::new(Service::Ses, []);
    let start = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(4),
        sender_with_ses_timeout(s3_http.clone(), ses_http.clone(), None).send(email(None)),
    )
    .await
    .expect("adapter S3 deadline must bound the stalled request");
    assert_eq!(
        outcome,
        NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
            retryability: NewsletterConfirmationEmailRetryability::Retryable,
        }
    );
    assert!(start.elapsed() >= Duration::from_secs(2));
    assert_eq!(s3_http.paths().len(), 1);
    assert!(ses_http.requests().is_empty());
}
