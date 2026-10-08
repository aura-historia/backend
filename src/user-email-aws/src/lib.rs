//! SES v2 adapter for transient newsletter confirmation proofs. Only the service port
//! crosses the boundary; token-bearing messages are neither cached nor logged.

use aws_sdk_s3::Client as S3Client;
use aws_sdk_sesv2::{
    Client as SesClient,
    config::retry::RetryConfig,
    error::SdkError,
    operation::send_email::{SendEmailError, SendEmailOutput},
    types::{Body, Content, Destination, EmailContent, Message},
};
use handlebars::Handlebars;
use localization::Language;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use url::{Host, Url};
use user_core::newsletter_confirmation::RawNewsletterConfirmationToken;
use user_service::ports::{
    NewsletterConfirmationEmail, NewsletterConfirmationEmailRetryability,
    NewsletterConfirmationEmailSendOutcome, NewsletterConfirmationEmailSender,
};

// Includes the SDK call and response body. C11 must leave room for service persistence
// and HTTP response after these sequential deadlines (2s + 3s).
const S3_DEADLINE: Duration = Duration::from_secs(2);
const SES_DEADLINE: Duration = Duration::from_secs(3);
const MAX_TEMPLATE_BYTES: usize = 128 * 1024;
const MAX_RENDERED_BYTES: usize = 256 * 1024;

/// Safe to display: no configuration value or proof is retained in errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NewsletterConfirmationEmailConfigError {
    InvalidConfiguration,
    InvalidFrontendOrigin,
}

impl std::fmt::Display for NewsletterConfirmationEmailConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidConfiguration => "invalid newsletter confirmation email configuration",
            Self::InvalidFrontendOrigin => "invalid newsletter confirmation frontend origin",
        })
    }
}

impl std::error::Error for NewsletterConfirmationEmailConfigError {}

#[derive(Clone, PartialEq, Eq)]
pub struct NewsletterConfirmationEmailConfig {
    bucket: String,
    from: String,
    reply_to: String,
    stage: String,
    commit: String,
    frontend_origin: Url,
}

impl NewsletterConfirmationEmailConfig {
    /// `frontend_origin` is a trusted deployment setting, never a request redirect.
    /// Real stages have pinned HTTPS origins; local/ephemeral require HTTP loopback.
    pub fn new(
        bucket: impl Into<String>,
        from: impl Into<String>,
        reply_to: impl Into<String>,
        stage: impl Into<String>,
        commit: impl Into<String>,
        frontend_origin: &str,
    ) -> Result<Self, NewsletterConfirmationEmailConfigError> {
        let (bucket, from, reply_to, stage, commit) = (
            bucket.into(),
            from.into(),
            reply_to.into(),
            stage.into(),
            commit.into(),
        );
        if bucket.trim().is_empty()
            || from.trim().is_empty()
            || reply_to.trim().is_empty()
            || !safe_key_component(&stage)
            || !safe_key_component(&commit)
        {
            return Err(NewsletterConfirmationEmailConfigError::InvalidConfiguration);
        }
        let origin = Url::parse(frontend_origin)
            .map_err(|_| NewsletterConfirmationEmailConfigError::InvalidFrontendOrigin)?;
        let loopback = match origin.host() {
            Some(Host::Domain("localhost")) => true,
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        };
        let trusted_origin = match stage.as_str() {
            "prod" => {
                origin.scheme() == "https"
                    && origin.host_str() == Some("aura-historia.com")
                    && origin.port().is_none()
            }
            "dev" => {
                origin.scheme() == "https"
                    && origin.host_str() == Some("stage.aura-historia.com")
                    && origin.port().is_none()
            }
            "local" | "ephemeral" => {
                origin.scheme() == "http" && loopback && origin.port().is_some()
            }
            _ => return Err(NewsletterConfirmationEmailConfigError::InvalidConfiguration),
        };
        if !trusted_origin
            || origin.cannot_be_a_base()
            || origin.username() != ""
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err(NewsletterConfirmationEmailConfigError::InvalidFrontendOrigin);
        }
        Ok(Self {
            bucket,
            from,
            reply_to,
            stage,
            commit,
            frontend_origin: origin,
        })
    }
}

fn safe_key_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EmailLanguage {
    De,
    En,
    Es,
    Fr,
    It,
}

impl EmailLanguage {
    fn resolve(language: Option<Language>) -> Self {
        match language {
            Some(Language::De) => Self::De,
            Some(Language::Es) => Self::Es,
            Some(Language::Fr) => Self::Fr,
            Some(Language::It) => Self::It,
            _ => Self::En,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::De => "de",
            Self::En => "en",
            Self::Es => "es",
            Self::Fr => "fr",
            Self::It => "it",
        }
    }

    fn subject(self) -> &'static str {
        match self {
            Self::De => "Aura Historia: Newsletter-Anmeldung bestätigen",
            Self::En => "Aura Historia: Confirm your newsletter subscription",
            Self::Es => "Aura Historia: Confirma tu suscripción al boletín",
            Self::Fr => "Aura Historia : Confirmez votre inscription à la newsletter",
            Self::It => "Aura Historia: Conferma l'iscrizione alla newsletter",
        }
    }
}

/// Internal failure categories intentionally carry no provider error, recipient, token or URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreparationFailure {
    TemplateMissing,
    TemplateUnavailable,
    TemplateInvalid,
    InvalidLink,
}

pub struct SesNewsletterConfirmationEmailSender {
    s3: S3Client,
    ses: SesClient,
    config: NewsletterConfirmationEmailConfig,
}

impl SesNewsletterConfirmationEmailSender {
    pub fn new(s3: S3Client, ses: SesClient, config: NewsletterConfirmationEmailConfig) -> Self {
        // SendEmail has no idempotency key. A retry after response loss can send twice.
        let ses = SesClient::from_conf(
            ses.config()
                .to_builder()
                .retry_config(RetryConfig::disabled())
                .build(),
        );
        Self { s3, ses, config }
    }

    fn template_key(&self, language: EmailLanguage) -> String {
        format!(
            "{}/{}/mjml/newsletter/confirmation/{}.html",
            self.config.stage,
            self.config.commit,
            language.code()
        )
    }

    fn confirmation_url(
        &self,
        language: EmailLanguage,
        token: &RawNewsletterConfirmationToken,
    ) -> Result<Url, PreparationFailure> {
        let mut url = self.config.frontend_origin.clone();
        url.set_path(&format!("/{}/newsletter/confirm", language.code()));
        url.set_fragment(Some(&format!("token={}", token.as_str())));
        if url.query().is_some() {
            return Err(PreparationFailure::InvalidLink);
        }
        Ok(url)
    }

    async fn rendered_body(
        &self,
        language: EmailLanguage,
        email: &NewsletterConfirmationEmail,
    ) -> Result<String, PreparationFailure> {
        // One deadline covers both the S3 request and streaming body consumption.
        let bytes = tokio::time::timeout(S3_DEADLINE, async {
            let response = self
                .s3
                .get_object()
                .bucket(&self.config.bucket)
                .key(self.template_key(language))
                .send()
                .await
                .map_err(|error| {
                    if error
                        .as_service_error()
                        .is_some_and(|service| service.is_no_such_key())
                        || error
                            .raw_response()
                            .is_some_and(|response| response.status().as_u16() == 404)
                    {
                        PreparationFailure::TemplateMissing
                    } else {
                        PreparationFailure::TemplateUnavailable
                    }
                })?;
            if response
                .content_length()
                .is_some_and(|len| len > MAX_TEMPLATE_BYTES as i64)
            {
                return Err(PreparationFailure::TemplateInvalid);
            }
            let mut bytes = Vec::new();
            response
                .body
                .into_async_read()
                .take((MAX_TEMPLATE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| PreparationFailure::TemplateUnavailable)?;
            Ok::<_, PreparationFailure>(bytes)
        })
        .await
        .map_err(|_| PreparationFailure::TemplateUnavailable)??;
        if bytes.len() > MAX_TEMPLATE_BYTES {
            return Err(PreparationFailure::TemplateInvalid);
        }
        let template =
            std::str::from_utf8(&bytes).map_err(|_| PreparationFailure::TemplateInvalid)?;
        if !template.contains("{{confirmation_url}}") || template.contains("{{{") {
            return Err(PreparationFailure::TemplateInvalid);
        }
        let url = self.confirmation_url(language, &email.token)?;
        let mut handlebars = Handlebars::new();
        handlebars.set_strict_mode(true);
        let body = handlebars
            .render_template(
                template,
                &serde_json::json!({
                    "confirmation_url": url.as_str(),
                    "first_name": email.profile.first_name.as_ref().map(AsRef::as_ref),
                }),
            )
            .map_err(|_| PreparationFailure::TemplateInvalid)?;
        if body.len() > MAX_RENDERED_BYTES {
            return Err(PreparationFailure::TemplateInvalid);
        }
        Ok(body)
    }

    async fn send_once(
        &self,
        email: NewsletterConfirmationEmail,
    ) -> NewsletterConfirmationEmailSendOutcome {
        let language = EmailLanguage::resolve(email.profile.language);
        let body = match self.rendered_body(language, &email).await {
            Ok(body) => body,
            Err(_) => return rejected(NewsletterConfirmationEmailRetryability::NotRetryable),
        };
        let subject = match Content::builder().data(language.subject()).build() {
            Ok(content) => content,
            Err(_) => return rejected(NewsletterConfirmationEmailRetryability::NotRetryable),
        };
        let html = match Content::builder().data(body).build() {
            Ok(content) => content,
            Err(_) => return rejected(NewsletterConfirmationEmailRetryability::NotRetryable),
        };
        let message = Message::builder()
            .subject(subject)
            .body(Body::builder().html(html).build())
            .build();
        let response = tokio::time::timeout(
            SES_DEADLINE,
            self.ses
                .send_email()
                .from_email_address(&self.config.from)
                .reply_to_addresses(&self.config.reply_to)
                .destination(
                    Destination::builder()
                        .to_addresses(email.recipient.to_string())
                        .build(),
                )
                .content(EmailContent::builder().simple(message).build())
                .send(),
        )
        .await;
        match response {
            Ok(Ok(receipt)) => accepted_receipt(&receipt),
            Ok(Err(error)) => classify_ses_error(&error),
            Err(_) => NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown,
        }
    }
}

#[async_trait::async_trait]
impl NewsletterConfirmationEmailSender for SesNewsletterConfirmationEmailSender {
    async fn send(
        &self,
        email: NewsletterConfirmationEmail,
    ) -> NewsletterConfirmationEmailSendOutcome {
        self.send_once(email).await
    }
}

fn accepted_receipt(response: &SendEmailOutput) -> NewsletterConfirmationEmailSendOutcome {
    if response
        .message_id()
        .is_some_and(|id| !id.trim().is_empty())
    {
        NewsletterConfirmationEmailSendOutcome::Accepted
    } else {
        NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
    }
}

fn classify_ses_error(error: &SdkError<SendEmailError>) -> NewsletterConfirmationEmailSendOutcome {
    match error {
        SdkError::ConstructionFailure(_) => {
            rejected(NewsletterConfirmationEmailRetryability::NotRetryable)
        }
        SdkError::ServiceError(response) => {
            let service = response.err();
            if service.is_too_many_requests_exception()
                || service.is_limit_exceeded_exception()
                || response.raw().status().as_u16() == 429
            {
                rejected(NewsletterConfirmationEmailRetryability::Retryable)
            } else if service.is_bad_request_exception()
                || service.is_message_rejected()
                || service.is_account_suspended_exception()
                || service.is_mail_from_domain_not_verified_exception()
                || service.is_not_found_exception()
                || service.is_sending_paused_exception()
                || matches!(response.raw().status().as_u16(), 400..=407 | 409..=428 | 430..=499)
            {
                rejected(NewsletterConfirmationEmailRetryability::NotRetryable)
            } else {
                // Timeout and 5xx may follow acceptance; do not retry.
                NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
            }
        }
        _ => NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown,
    }
}

fn rejected(
    retryability: NewsletterConfirmationEmailRetryability,
) -> NewsletterConfirmationEmailSendOutcome {
    NewsletterConfirmationEmailSendOutcome::DefinitelyRejected { retryability }
}

#[cfg(test)]
mod tests {
    use super::*;
    use user_core::newsletter_confirmation::RawNewsletterConfirmationToken;
    use user_core::newsletter_confirmation_id::NewsletterConfirmationId;
    use user_service::ports::NewsletterProfile;

    fn config(
        stage: &str,
        origin: &str,
    ) -> Result<NewsletterConfirmationEmailConfig, NewsletterConfirmationEmailConfigError> {
        NewsletterConfirmationEmailConfig::new(
            "bucket",
            "from@example.test",
            "reply@example.test",
            stage,
            "abc123",
            origin,
        )
    }

    #[test]
    fn validates_only_trusted_origins_and_key_components() {
        for origin in [
            "https://attacker.test",
            "https://attacker.test/path",
            "https://stage.aura-historia.com",
            "https://aura-historia.com:8443",
            "https://attacker.test/?next=evil",
            "https://attacker.test/#token=evil",
            "https://user:pass@attacker.test",
            "http://aura-historia.com",
            "http://127.0.0.2",
        ] {
            assert!(config("prod", origin).is_err(), "{origin}");
        }
        assert!(config("prod", "https://aura-historia.com").is_ok());
        assert!(config("dev", "https://stage.aura-historia.com").is_ok());
        assert!(config("dev", "https://aura-historia.com").is_err());
        assert!(config("dev", "https://stage.aura-historia.com:8443").is_err());
        assert!(config("ephemeral", "https://attacker.test").is_err());
        assert!(config("stage", "https://stage.aura-historia.com").is_err());
        assert!(config("ephemeral", "http://127.0.0.1:3000").is_ok());
        assert!(config("local", "http://localhost:3000").is_ok());
        assert!(config("prod", "http://localhost:3000").is_err());
        assert!(
            NewsletterConfirmationEmailConfig::new(
                "bucket",
                "from",
                "reply",
                "../prod",
                "abc123",
                "https://aura-historia.com"
            )
            .is_err()
        );
    }

    #[test]
    fn languages_keys_and_localized_fragment_only_links() {
        for (input, code) in [
            (Some(Language::De), "de"),
            (Some(Language::En), "en"),
            (Some(Language::Es), "es"),
            (Some(Language::Fr), "fr"),
            (Some(Language::It), "it"),
            (Some(Language::Zh), "en"),
            (None, "en"),
        ] {
            let language = EmailLanguage::resolve(input);
            let token = RawNewsletterConfirmationToken::from_entropy([90; 32]);
            let sender = SesNewsletterConfirmationEmailSender {
                s3: S3Client::from_conf(
                    aws_sdk_s3::Config::builder()
                        .behavior_version_latest()
                        .region(aws_sdk_s3::config::Region::new("eu-central-1"))
                        .build(),
                ),
                ses: SesClient::from_conf(
                    aws_sdk_sesv2::Config::builder()
                        .behavior_version_latest()
                        .region(aws_sdk_sesv2::config::Region::new("eu-central-1"))
                        .build(),
                ),
                config: config("dev", "https://stage.aura-historia.com").unwrap(),
            };
            let email = NewsletterConfirmationEmail {
                confirmation_id: NewsletterConfirmationId::new(),
                recipient: "reader@example.test".try_into().unwrap(),
                token: token.clone(),
                profile: NewsletterProfile {
                    language: input,
                    ..Default::default()
                },
            };
            assert_eq!(
                sender.template_key(language),
                format!("dev/abc123/mjml/newsletter/confirmation/{code}.html")
            );
            let link = sender.confirmation_url(language, &email.token).unwrap();
            assert_eq!(link.path(), format!("/{code}/newsletter/confirm"));
            assert_eq!(
                link.fragment(),
                Some(format!("token={}", token.as_str()).as_str())
            );
            assert_eq!(link.query(), None);
            assert!(!link.path().contains(token.as_str()));
        }
    }

    #[test]
    fn sdk_retries_are_disabled_even_when_caller_provides_retries() {
        let ses = SesClient::from_conf(
            aws_sdk_sesv2::Config::builder()
                .behavior_version_latest()
                .region(aws_sdk_sesv2::config::Region::new("eu-central-1"))
                .retry_config(RetryConfig::standard().with_max_attempts(3))
                .build(),
        );
        let s3 = S3Client::from_conf(
            aws_sdk_s3::Config::builder()
                .behavior_version_latest()
                .region(aws_sdk_s3::config::Region::new("eu-central-1"))
                .build(),
        );
        let sender = SesNewsletterConfirmationEmailSender::new(
            s3,
            ses,
            config("prod", "https://aura-historia.com").unwrap(),
        );
        assert_eq!(
            sender
                .ses
                .config()
                .retry_config()
                .map(RetryConfig::max_attempts),
            Some(1)
        );
    }

    #[test]
    fn provider_errors_cannot_expose_provider_payloads() {
        let secret = "token-and-recipient@example.test";
        let failure = SdkError::timeout_error(std::io::Error::other(secret));
        assert_eq!(
            classify_ses_error(&failure),
            NewsletterConfirmationEmailSendOutcome::AcceptanceUnknown
        );
        assert!(!format!("{:?}", classify_ses_error(&failure)).contains(secret));
        assert!(!format!("{:?}", PreparationFailure::TemplateInvalid).contains(secret));
    }
}
