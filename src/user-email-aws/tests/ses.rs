//! Exercises the real SES SDK with only its HTTP connector replaced; no AWS calls.
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
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tracing::instrument::WithSubscriber;
use user_core::{
    first_name::FirstName, newsletter_confirmation::RawNewsletterConfirmationToken,
    newsletter_confirmation_id::NewsletterConfirmationId,
};
use user_email_aws::{NewsletterConfirmationEmailConfig, SesNewsletterConfirmationEmailSender};
use user_service::ports::{
    NewsletterConfirmationEmail, NewsletterConfirmationEmailRetryability,
    NewsletterConfirmationEmailSendOutcome, NewsletterConfirmationEmailSender, NewsletterProfile,
};

const CONFIGURATION_SET: &str = "aura-historia-ephemeral-email";
const TEMPLATE: &str = "<html><body>{{#if first_name}}Hello {{first_name}}{{else}}Hello{{/if}}<a href=\"{{confirmation_url}}\">Confirm subscription</a></body></html>";

#[derive(Debug)]
enum Reply {
    Response(u16, &'static str),
    IoFailure,
    Hang,
}

#[derive(Debug)]
struct State {
    replies: VecDeque<Reply>,
    requests: Vec<Value>,
}

#[derive(Clone, Debug)]
struct ReplayHttp {
    state: Arc<Mutex<State>>,
}

impl ReplayHttp {
    fn new(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                replies: replies.into_iter().collect(),
                requests: Vec::new(),
            })),
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }
}

impl HttpConnector for ReplayHttp {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        assert!(request.headers().get("authorization").is_some());
        assert_eq!(request.method(), "POST");
        assert!(
            request
                .uri()
                .to_string()
                .contains("/v2/email/outbound-emails")
        );
        let body: Value =
            serde_json::from_slice(request.body().bytes().expect("SES JSON request body"))
                .expect("valid SES JSON request");
        assert_eq!(body["ConfigurationSetName"], CONFIGURATION_SET);
        let mut state = self.state.lock().unwrap();
        state.requests.push(body);
        let reply = state.replies.pop_front().expect("unexpected SDK retry");
        drop(state);
        match reply {
            Reply::Response(status, body) => {
                let response: HttpResponse = http::Response::builder()
                    .status(status)
                    .header("content-type", "application/json")
                    .body(SdkBody::from(body.to_owned()))
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

fn sender(ses_http: ReplayHttp) -> SesNewsletterConfirmationEmailSender {
    sender_with_ses_timeout(ses_http, Some(Duration::from_millis(100)))
}

static NEXT_TEMPLATE_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

/// A fresh template directory holding `template` for every email language.
fn template_directory(template: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "newsletter-confirmation-ses-{}-{}",
        std::process::id(),
        NEXT_TEMPLATE_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    for language in ["de", "en", "es", "fr", "it"] {
        let path = directory.join(format!("newsletter/confirmation/{language}.html"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, template).unwrap();
    }
    directory
}

fn config(template_directory: PathBuf) -> NewsletterConfirmationEmailConfig {
    NewsletterConfirmationEmailConfig::new(
        template_directory,
        "sender@example.test",
        "reply@example.test",
        CONFIGURATION_SET,
        "ephemeral",
        "http://127.0.0.1:3000",
    )
    .unwrap_or_else(|_| panic!("valid newsletter email config"))
}

fn ses_client(ses_http: ReplayHttp, sdk_timeout: Option<Duration>) -> aws_sdk_sesv2::Client {
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
    aws_sdk_sesv2::Client::from_conf(ses_config.build())
}

fn sender_with_template(
    ses_http: ReplayHttp,
    template: &str,
) -> SesNewsletterConfirmationEmailSender {
    SesNewsletterConfirmationEmailSender::new(
        ses_client(ses_http, Some(Duration::from_millis(100))),
        config(template_directory(template)),
    )
    .unwrap_or_else(|error| panic!("{error}"))
}

fn sender_with_ses_timeout(
    ses_http: ReplayHttp,
    sdk_timeout: Option<Duration>,
) -> SesNewsletterConfirmationEmailSender {
    SesNewsletterConfirmationEmailSender::new(
        ses_client(ses_http, sdk_timeout),
        config(template_directory(TEMPLATE)),
    )
    .unwrap_or_else(|error| panic!("{error}"))
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

#[tokio::test]
async fn sends_signed_ses_payload_with_confirmation_link_and_accepts_receipt() {
    let ses_http = ReplayHttp::new([Reply::Response(200, r#"{"MessageId":"ses-receipt-123"}"#)]);
    let email = email(Some("Ada & Co"));
    let token = email.token.as_str().to_owned();
    let confirmation_id = email.confirmation_id.to_string();
    let outcome = sender(ses_http.clone()).send(email).await;
    assert_eq!(outcome, NewsletterConfirmationEmailSendOutcome::Accepted);
    assert_eq!(ses_http.requests().len(), 1);
    let requests = ses_http.requests();
    let request = &requests[0];
    assert_eq!(
        request["Destination"]["ToAddresses"],
        json!(["reader@example.test"])
    );
    assert_eq!(request["ConfigurationSetName"], CONFIGURATION_SET);
    assert_eq!(
        request["FromEmailAddress"],
        "\"Newsletter | Aura Historia\" <sender@example.test>"
    );
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

#[test]
fn missing_oversized_and_unsafe_templates_stop_the_sender_from_starting() {
    let missing = template_directory(TEMPLATE);
    std::fs::remove_file(missing.join("newsletter/confirmation/it.html")).unwrap();
    let invalid_utf8 = template_directory(TEMPLATE);
    std::fs::write(invalid_utf8.join("newsletter/confirmation/de.html"), [0xff]).unwrap();
    for directory in [
        missing,
        invalid_utf8,
        template_directory(&format!("{TEMPLATE}{}", "x".repeat(128 * 1024))),
        template_directory("{{#if invalid"),
        template_directory("<html>{{{first_name}}}{{confirmation_url}}</html>"),
        template_directory("<html>no confirmation link</html>"),
    ] {
        let error = SesNewsletterConfirmationEmailSender::new(
            ses_client(ReplayHttp::new([]), None),
            config(directory),
        )
        .err()
        .expect("unusable template");
        assert!(
            error
                .to_string()
                .starts_with("newsletter confirmation template newsletter/confirmation/"),
            "{error}"
        );
    }
}

#[tokio::test]
async fn success_without_a_nonempty_ses_receipt_has_unknown_acceptance() {
    for response in [r#"{}"#, r#"{"MessageId":""}"#] {
        let ses_http = ReplayHttp::new([Reply::Response(200, response)]);
        let outcome = sender(ses_http.clone()).send(email(None)).await;
        assert_eq!(
            outcome,
            NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
        );
        assert_eq!(ses_http.requests().len(), 1);
    }
}

#[tokio::test]
async fn modeled_message_rejection_is_definite() {
    let ses_http = ReplayHttp::new([Reply::Response(
        400,
        r#"{"__type":"MessageRejected","message":"rejected"}"#,
    )]);
    let outcome = sender(ses_http.clone()).send(email(None)).await;
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
        let ses_http = ReplayHttp::new([reply]);
        assert_eq!(
            sender(ses_http.clone()).send(email(None)).await,
            NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
                retryability: NewsletterConfirmationEmailRetryability::Retryable,
            }
        );
        assert_eq!(ses_http.requests().len(), 1);
    }
}

#[tokio::test]
async fn ambiguous_transport_failure_does_not_trigger_hidden_ses_sdk_retry() {
    let ses_http = ReplayHttp::new([
        Reply::IoFailure,
        Reply::Response(200, r#"{"MessageId":"second-send"}"#),
    ]);
    let outcome = sender(ses_http.clone()).send(email(None)).await;
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
    let ses_http = ReplayHttp::new([Reply::Hang]);
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        sender(ses_http.clone()).send(email(None)),
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
    let ses_http = ReplayHttp::new([Reply::Hang]);
    let start = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        sender_with_ses_timeout(ses_http.clone(), None).send(email(None)),
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

#[derive(Clone, Default)]
struct CapturedLogs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for CapturedLogs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn failure_logs_identify_the_phase_without_exposing_mail_or_provider_payloads() {
    // Keep two dispatches alive while capturing: tracing-core's single-dispatcher
    // fast path can cache `never` when another test first registers a shared
    // callsite without a thread-local subscriber. This inert dispatch forces
    // interest calculation across dispatches without installing a global default.
    let _interest_cache_guard =
        tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
    for (template, ses_reply, phase, category) in [
        (
            // Strict rendering fails on a variable the confirmation data does not carry.
            "{{unknown}}{{confirmation_url}}",
            None,
            "preparation",
            "TemplateInvalid",
        ),
        (
            TEMPLATE,
            Some(Reply::Response(
                403,
                r#"{"__type":"AccessDeniedException","message":"private-provider-detail reader@example.test"}"#,
            )),
            "ses",
            "AccessDenied",
        ),
        (
            TEMPLATE,
            Some(Reply::Response(
                400,
                r#"{"__type":"MessageRejected","message":"private-provider-detail reader@example.test"}"#,
            )),
            "ses",
            "MessageRejected",
        ),
        (
            TEMPLATE,
            Some(Reply::Response(
                400,
                r#"{"__type":"private-provider-detail","message":"reader@example.test"}"#,
            )),
            "ses",
            "OtherServiceError",
        ),
        (TEMPLATE, Some(Reply::IoFailure), "ses", "TransportFailure"),
        (
            TEMPLATE,
            Some(Reply::Response(200, r#"{}"#)),
            "ses",
            "MissingReceipt",
        ),
    ] {
        let logs = CapturedLogs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        let message = email(Some("private-recipient-name"));
        let token = message.token.as_str().to_owned();
        let confirmation_id = message.confirmation_id.to_string();
        let ses_http = ReplayHttp::new(ses_reply);
        let outcome = sender_with_template(ses_http.clone(), template)
            .send(message)
            .with_subscriber(subscriber)
            .await;
        let expected = if matches!(category, "TransportFailure" | "MissingReceipt") {
            NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
        } else {
            NewsletterConfirmationEmailSendOutcome::DefinitelyRejected {
                retryability: NewsletterConfirmationEmailRetryability::NotRetryable,
            }
        };
        assert_eq!(outcome, expected);
        assert_eq!(ses_http.requests().len(), usize::from(phase == "ses"));
        let output = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let events: Vec<Value> = output
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|event| event["fields"]["event"] == "newsletter_confirmation_email.failed")
            .collect();
        assert_eq!(
            events.len(),
            1,
            "expected a categorized failure for {category}"
        );
        assert_eq!(events[0]["fields"]["phase"], phase);
        assert_eq!(events[0]["fields"]["failure_category"], category);
        for private in [
            "reader@example.test",
            "private-recipient-name",
            "private-provider-detail",
            "http://127.0.0.1:3000",
            "Confirm subscription",
            token.as_str(),
            confirmation_id.as_str(),
        ] {
            assert!(
                !output.contains(private),
                "failure log exposed private data"
            );
        }
    }
}
