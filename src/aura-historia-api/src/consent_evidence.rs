//! Where the API's marketing-consent evidence goes. On the host, both evidence paths run in this
//! process: the REST API's and Cognito post-confirmation's (`/internal/cognito/post-confirmation`).
//! Their records keep landing in the two retained CloudWatch log groups that the retired Lambda
//! functions created ([marketing consent](../../../docs/marketing-consent.md#evidence-and-privacy)),
//! through a dedicated target that neither the log level nor the container log shipping affects.
use platform_cloudwatch_logs::{
    CloudWatchLogsSink, RecordReceiver, Shipper, ShipperConfig, TargetRecords, channel,
};
use platform_observability::{LoggingConfig, init_with_dedicated_target, init_with_directives};
use std::time::Duration;
use tracing_subscriber::Layer;
use user_service::use_cases::{
    COGNITO_SIGNUP_CONSENT_SOURCE, CONSENT_EVIDENCE_LOG_DIRECTIVE, CONSENT_EVIDENCE_LOG_TARGET,
};

pub const CONSENT_EVIDENCE_API_LOG_GROUP_ENV: &str = "CONSENT_EVIDENCE_API_LOG_GROUP";
pub const CONSENT_EVIDENCE_POST_CONFIRMATION_LOG_GROUP_ENV: &str =
    "CONSENT_EVIDENCE_POST_CONFIRMATION_LOG_GROUP";
const STAGE_ENV: &str = "STAGE";
/// Records logged while CloudWatch is slow wait here; beyond it they are dropped and counted.
const QUEUE_CAPACITY: usize = 4_096;
const FLUSH_INTERVAL: Duration = Duration::from_secs(1);

/// Safe to display: names the settings, never their values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ConsentEvidenceConfigError {
    #[error(
        "{CONSENT_EVIDENCE_API_LOG_GROUP_ENV} and {CONSENT_EVIDENCE_POST_CONFIRMATION_LOG_GROUP_ENV} must both be set or both be unset"
    )]
    Incomplete,
    #[error("production requires the consent-evidence log groups")]
    RequiredInProduction,
}

/// The two retained evidence groups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsentEvidenceLogGroups {
    api: String,
    post_confirmation: String,
}

impl ConsentEvidenceLogGroups {
    /// Both groups, or `None` when neither is set, which keeps evidence in the process log for
    /// local runs. Production (`STAGE=prod`) requires both.
    pub fn from_getter(
        mut get: impl FnMut(&'static str) -> Option<String>,
    ) -> Result<Option<Self>, ConsentEvidenceConfigError> {
        let mut setting = |name| get(name).filter(|value| !value.trim().is_empty());
        let groups = match (
            setting(CONSENT_EVIDENCE_API_LOG_GROUP_ENV),
            setting(CONSENT_EVIDENCE_POST_CONFIRMATION_LOG_GROUP_ENV),
        ) {
            (Some(api), Some(post_confirmation)) => Some(Self {
                api,
                post_confirmation,
            }),
            (None, None) => None,
            _ => return Err(ConsentEvidenceConfigError::Incomplete),
        };
        if groups.is_none() && setting(STAGE_ENV).as_deref() == Some("prod") {
            return Err(ConsentEvidenceConfigError::RequiredInProduction);
        }
        Ok(groups)
    }
}

/// Cognito post-confirmation records Cognito sign-up evidence and nothing else, and nothing else
/// records it; it goes to the post-confirmation group (index 1), everything else to the API's.
pub fn route(line: &serde_json::Value) -> usize {
    usize::from(line["fields"]["consent_source"] == COGNITO_SIGNUP_CONSENT_SOURCE)
}

/// Evidence records waiting for [`ConsentEvidenceLog::start`].
pub struct ConsentEvidenceLog {
    groups: ConsentEvidenceLogGroups,
    receiver: RecordReceiver,
}

/// Initializes the process log. With evidence groups, evidence goes only to them; without, it
/// stays in the process log at INFO whatever the level.
pub fn init_logging(
    logging: LoggingConfig,
    groups: Option<ConsentEvidenceLogGroups>,
) -> Option<ConsentEvidenceLog> {
    let Some(groups) = groups else {
        init_with_directives(logging, &[CONSENT_EVIDENCE_LOG_DIRECTIVE]);
        return None;
    };
    let (records, receiver): (TargetRecords, RecordReceiver) =
        channel(CONSENT_EVIDENCE_LOG_TARGET, QUEUE_CAPACITY);
    init_with_dedicated_target(
        logging,
        &[],
        CONSENT_EVIDENCE_LOG_TARGET,
        records.layer().boxed(),
    );
    Some(ConsentEvidenceLog { groups, receiver })
}

impl ConsentEvidenceLog {
    /// Starts delivering to CloudWatch with the host's AWS credentials, in a stream of its own.
    pub fn start(self, client: aws_sdk_cloudwatchlogs::Client) -> Shipper {
        Shipper::spawn(
            CloudWatchLogsSink::new(client),
            self.receiver,
            ShipperConfig {
                log_groups: vec![self.groups.api, self.groups.post_confirmation],
                stream: stream_name(),
                route,
                flush_interval: FLUSH_INTERVAL,
            },
        )
    }
}

/// `host-api/<date>/<uuid>`: one stream per process, so replicas never share one.
fn stream_name() -> String {
    let today = time::OffsetDateTime::now_utc().date();
    format!("host-api/{today}/{}", uuid::Uuid::new_v4().simple())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn groups(
        values: &[(&'static str, &str)],
    ) -> Result<Option<ConsentEvidenceLogGroups>, ConsentEvidenceConfigError> {
        let values = values
            .iter()
            .map(|(name, value)| (*name, (*value).to_owned()))
            .collect::<HashMap<_, _>>();
        ConsentEvidenceLogGroups::from_getter(|name| values.get(name).cloned())
    }

    #[test]
    fn should_take_both_groups_or_neither_and_require_them_in_production() {
        assert_eq!(
            Ok(Some(ConsentEvidenceLogGroups {
                api: "/aws/lambda/aura-historia-api-prod".to_owned(),
                post_confirmation: "/aws/lambda/cognito-post-confirmation-prod".to_owned(),
            })),
            groups(&[
                (STAGE_ENV, "prod"),
                (
                    CONSENT_EVIDENCE_API_LOG_GROUP_ENV,
                    "/aws/lambda/aura-historia-api-prod"
                ),
                (
                    CONSENT_EVIDENCE_POST_CONFIRMATION_LOG_GROUP_ENV,
                    "/aws/lambda/cognito-post-confirmation-prod"
                ),
            ])
        );
        assert_eq!(Ok(None), groups(&[(STAGE_ENV, "local")]));
        assert_eq!(
            Err(ConsentEvidenceConfigError::RequiredInProduction),
            groups(&[(STAGE_ENV, "prod")])
        );
        assert_eq!(
            Err(ConsentEvidenceConfigError::Incomplete),
            groups(&[(CONSENT_EVIDENCE_API_LOG_GROUP_ENV, "/api")])
        );
        assert_eq!(
            Err(ConsentEvidenceConfigError::Incomplete),
            groups(&[
                (CONSENT_EVIDENCE_API_LOG_GROUP_ENV, " "),
                (
                    CONSENT_EVIDENCE_POST_CONFIRMATION_LOG_GROUP_ENV,
                    "/post-confirmation"
                ),
            ])
        );
    }

    #[test]
    fn should_route_cognito_signup_evidence_to_the_post_confirmation_group() {
        let line = |source: &str| serde_json::json!({ "fields": { "consent_source": source } });

        assert_eq!(1, route(&line("COGNITO_SIGNUP")));
        for source in [
            "AURA_DOUBLE_OPT_IN",
            "USER_WITHDRAWAL",
            "LOOPS_USER_PREFERENCE",
        ] {
            assert_eq!(0, route(&line(source)));
        }
        assert_eq!(0, route(&serde_json::json!({})));
    }

    #[test]
    fn should_name_a_fresh_stream_per_process() {
        let (first, second) = (stream_name(), stream_name());
        assert!(first.starts_with("host-api/"));
        assert!(!first.contains(':') && !first.contains('*'));
        assert_ne!(first, second);
    }
}
