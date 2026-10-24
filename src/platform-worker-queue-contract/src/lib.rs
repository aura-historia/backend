//! Shared SQS destination identity and read-only source/DLQ contract.
use aura_historia_jobs::{WorkerQueueType, WorkerScope};
use aws_sdk_sqs::types::QueueAttributeName;
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use url::Url;

pub const AWS_REGION_ENV: &str = "AWS_REGION";
pub const SQS_ENDPOINT_ENV: &str = "AWS_ENDPOINT_URL_SQS";

/// One trusted source/DLQ pair per deployed scope. Construction performs no AWS mutations.
#[derive(Clone, Debug)]
pub struct SqsQueueConfig {
    scope: WorkerScope,
    queue_url: Url,
    region: String,
    stage: String,
    endpoint: Option<Url>,
    account: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QueueError {
    #[error("missing worker queue configuration: {0}")]
    MissingConfig(&'static str),
    #[error("invalid worker queue configuration: {0}")]
    InvalidConfig(&'static str),
    #[error("worker queue attribute contract mismatch: {0}")]
    Attribute(&'static str),
    #[error("SQS operation unavailable")]
    Unavailable,
    #[error("SQS operation timed out")]
    Timeout,
    #[error("invalid SQS response metadata")]
    InvalidResponse,
    #[error("worker SQS message exceeds encoded size limit")]
    MessageTooLarge,
    #[error("prepared worker job does not match the configured queue scope or type")]
    PreparedJob,
}

/// The router has one configured source queue per deployed scope; consent is optional until C06.
#[derive(Clone, Debug)]
pub struct CdcRouterQueueConfig {
    queues: Vec<SqsQueueConfig>,
}

impl CdcRouterQueueConfig {
    pub fn from_env() -> Result<Self, QueueError> {
        Self::from_getter(|name| std::env::var(name).ok())
    }

    pub fn from_getter<F>(mut get: F) -> Result<Self, QueueError>
    where
        F: FnMut(&'static str) -> Option<String>,
    {
        let region = required_config(&mut get, AWS_REGION_ENV)?;
        let stage = required_config(&mut get, "STAGE")?;
        // Do not let the AWS global endpoint override bypass the explicit local boundary.
        if get("AWS_ENDPOINT_URL").is_some() {
            return Err(QueueError::InvalidConfig(
                "AWS_ENDPOINT_URL (use AWS_ENDPOINT_URL_SQS)",
            ));
        }
        let endpoint = get(SQS_ENDPOINT_ENV)
            .map(|value| {
                value
                    .parse()
                    .map_err(|_| QueueError::InvalidConfig(SQS_ENDPOINT_ENV))
            })
            .transpose()?;
        let mut queues = Vec::with_capacity(WorkerScope::ALL.len());
        for scope in WorkerScope::ALL {
            let env = scope.router_queue_url_env();
            let configured_url = match get(env) {
                Some(value) if !value.is_empty() => value,
                Some(_) => return Err(QueueError::MissingConfig(env)),
                None if scope == WorkerScope::MarketingConsentSync => {
                    // C05 defines the target contract; C06 supplies and activates this destination.
                    continue;
                }
                None => return Err(QueueError::MissingConfig(env)),
            };
            let queue_url = configured_url
                .parse()
                .map_err(|_| QueueError::InvalidConfig(env))?;
            queues.push(SqsQueueConfig::new(
                scope,
                queue_url,
                region.clone(),
                stage.clone(),
                endpoint.clone(),
            )?);
        }
        Ok(Self { queues })
    }

    pub fn region(&self) -> &str {
        // Ten existing destinations are required, so the configured queue list is never empty.
        self.queues[0].region()
    }

    pub fn into_queues(self) -> Vec<SqsQueueConfig> {
        self.queues
    }
}
impl SqsQueueConfig {
    pub fn new(
        scope: WorkerScope,
        queue_url: Url,
        region: String,
        stage: String,
        local_endpoint: Option<Url>,
    ) -> Result<Self, QueueError> {
        let queue_suffix = if scope.queue_type() == WorkerQueueType::Fifo {
            ".fifo"
        } else {
            ""
        };
        if !valid_label(&stage)
            || format!("aura-worker-{}-dlq-{stage}{queue_suffix}", scope.as_str()).len() > 80
        {
            return Err(QueueError::InvalidConfig("STAGE"));
        }
        if !valid_label(&region) {
            return Err(QueueError::InvalidConfig(AWS_REGION_ENV));
        }
        let local = matches!(stage.as_str(), "ephemeral" | "local" | "test");
        if local_endpoint.is_some() && !local {
            return Err(QueueError::InvalidConfig(SQS_ENDPOINT_ENV));
        }
        clean_url(&queue_url)?;
        let segments: Vec<_> = queue_url.path().split('/').collect();
        let source_name = format!("aura-worker-{}-{stage}{queue_suffix}", scope.as_str());
        if segments.len() != 3
            || segments[2] != source_name
            || segments[1].len() != 12
            || !segments[1].bytes().all(|b| b.is_ascii_digit())
        {
            return Err(QueueError::InvalidConfig("queue URL"));
        }
        let account = segments[1].to_owned();
        let aws_host = format!("sqs.{region}.amazonaws.com");
        if let Some(endpoint) = &local_endpoint {
            clean_url(endpoint)?;
            if endpoint.path() != "/"
                || endpoint.origin() != queue_url.origin()
                || !matches!(endpoint.scheme(), "http" | "https")
            {
                return Err(QueueError::InvalidConfig(SQS_ENDPOINT_ENV));
            }
        } else if queue_url.scheme() != "https"
            || queue_url.host_str() != Some(aws_host.as_str())
            || queue_url.port().is_some()
        {
            return Err(QueueError::InvalidConfig("queue URL"));
        }
        Ok(Self {
            scope,
            queue_url,
            region,
            stage,
            endpoint: local_endpoint,
            account,
        })
    }

    pub const fn scope(&self) -> WorkerScope {
        self.scope
    }
    pub const fn queue_type(&self) -> WorkerQueueType {
        self.scope.queue_type()
    }
    pub fn queue_url(&self) -> &Url {
        &self.queue_url
    }
    pub fn region(&self) -> &str {
        &self.region
    }
    pub fn stage(&self) -> &str {
        &self.stage
    }
    pub fn visibility_timeout(&self) -> Duration {
        visibility(self.scope)
    }

    pub fn arn(&self, dlq: bool) -> String {
        format!(
            "arn:aws:sqs:{}:{}:{}",
            self.region,
            self.account,
            self.name(dlq)
        )
    }
    fn name(&self, dlq: bool) -> String {
        let fifo_suffix = if self.queue_type() == WorkerQueueType::Fifo {
            ".fifo"
        } else {
            ""
        };
        format!(
            "aura-worker-{}{}-{}{fifo_suffix}",
            self.scope.as_str(),
            if dlq { "-dlq" } else { "" },
            self.stage
        )
    }
    pub fn dlq_url(&self) -> Url {
        let mut url = self.queue_url.clone();
        url.set_path(&format!("/{}/{}", self.account, self.name(true)));
        url
    }
    pub fn endpoint_url(&self) -> String {
        self.endpoint
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("https://sqs.{}.amazonaws.com", self.region))
    }
}

/// Source-queue visibility deployed by `infra/src/worker-queue-config.ts`.
pub fn visibility(scope: WorkerScope) -> Duration {
    Duration::from_secs(match scope {
        WorkerScope::ProductListingOpenSearch
        | WorkerScope::SearchFilterMatchNotification
        | WorkerScope::WatchlistNotification
        | WorkerScope::GeoResolution
        | WorkerScope::ListingGeography
        | WorkerScope::ListingShipping
        | WorkerScope::ProductListingTranslation => 300,
        WorkerScope::MarketingConsentSync | WorkerScope::UserIdentityDeletion => 330,
        WorkerScope::ProductListingContentAssessment
        | WorkerScope::ProductListingRawNormalization => 270,
        WorkerScope::ProductListingEmbedding => 360,
        WorkerScope::NotificationDelivery => 330,
    })
}

fn required_config<F>(get: &mut F, name: &'static str) -> Result<String, QueueError>
where
    F: FnMut(&'static str) -> Option<String>,
{
    get(name)
        .filter(|value| !value.is_empty())
        .ok_or(QueueError::MissingConfig(name))
}

fn valid_label(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.ends_with('-')
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn clean_url(url: &Url) -> Result<(), QueueError> {
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(QueueError::InvalidConfig("queue URL"));
    }
    Ok(())
}

pub type Attributes = HashMap<QueueAttributeName, String>;
fn attr(attributes: &Attributes, name: QueueAttributeName) -> Option<&str> {
    attributes.get(&name).map(String::as_str)
}
fn require(
    attributes: &Attributes,
    name: QueueAttributeName,
    expected: &str,
    field: &'static str,
) -> Result<(), QueueError> {
    if attr(attributes, name) != Some(expected) {
        return Err(QueueError::Attribute(field));
    }
    Ok(())
}

pub fn validate_attributes(
    config: &SqsQueueConfig,
    attributes: &Attributes,
    dlq: bool,
) -> Result<(), QueueError> {
    use QueueAttributeName as A;
    let arn = config.arn(dlq);
    require(attributes, A::QueueArn, &arn, "QueueArn")?;
    // AWS may omit FifoQueue on Standard queues; FIFO must be explicit on both queues.
    match config.queue_type() {
        WorkerQueueType::Standard => {
            if !matches!(attr(attributes, A::FifoQueue), None | Some("false")) {
                return Err(QueueError::Attribute("FifoQueue"));
            }
        }
        WorkerQueueType::Fifo => {
            require(attributes, A::FifoQueue, "true", "FifoQueue")?;
            require(
                attributes,
                A::ContentBasedDeduplication,
                "false",
                "ContentBasedDeduplication",
            )?;
        }
    }
    require(
        attributes,
        A::MessageRetentionPeriod,
        if dlq { "1209600" } else { "604800" },
        "MessageRetentionPeriod",
    )?;
    if attr(attributes, A::SqsManagedSseEnabled) != Some("true")
        && !attr(attributes, A::KmsMasterKeyId).is_some_and(|key| !key.is_empty())
    {
        return Err(QueueError::Attribute("encryption"));
    }
    let policy: Value =
        serde_json::from_str(attr(attributes, A::Policy).ok_or(QueueError::Attribute("Policy"))?)
            .map_err(|_| QueueError::Attribute("Policy"))?;
    if !tls_denied(&policy, &arn) {
        return Err(QueueError::Attribute("TLS policy"));
    }
    if !private_policy(&policy) {
        return Err(QueueError::Attribute("public access policy"));
    }
    let redrive_allow: Value = serde_json::from_str(
        attr(attributes, A::RedriveAllowPolicy)
            .ok_or(QueueError::Attribute("RedriveAllowPolicy"))?,
    )
    .map_err(|_| QueueError::Attribute("RedriveAllowPolicy"))?;
    let expected_allow = if dlq {
        serde_json::json!({"redrivePermission": "byQueue", "sourceQueueArns": [config.arn(false)]})
    } else {
        serde_json::json!({"redrivePermission": "denyAll"})
    };
    if redrive_allow != expected_allow {
        return Err(QueueError::Attribute("RedriveAllowPolicy"));
    }
    if dlq && attr(attributes, A::RedrivePolicy).is_some() {
        return Err(QueueError::Attribute("DLQ RedrivePolicy"));
    }
    if !dlq {
        validate_source_attributes(config, attributes)?;
    }
    Ok(())
}

fn validate_source_attributes(
    config: &SqsQueueConfig,
    attributes: &Attributes,
) -> Result<(), QueueError> {
    use QueueAttributeName as A;
    require(
        attributes,
        A::VisibilityTimeout,
        &config.visibility_timeout().as_secs().to_string(),
        "VisibilityTimeout",
    )?;
    require(
        attributes,
        A::ReceiveMessageWaitTimeSeconds,
        "20",
        "ReceiveMessageWaitTimeSeconds",
    )?;
    let redrive: Value = serde_json::from_str(
        attr(attributes, A::RedrivePolicy).ok_or(QueueError::Attribute("RedrivePolicy"))?,
    )
    .map_err(|_| QueueError::Attribute("RedrivePolicy"))?;
    if redrive["deadLetterTargetArn"] != config.arn(true)
        || !(redrive["maxReceiveCount"] == 5 || redrive["maxReceiveCount"] == "5")
    {
        return Err(QueueError::Attribute("RedrivePolicy"));
    }
    Ok(())
}

fn contains(value: &Value, expected: &str) -> bool {
    value.as_str() == Some(expected)
        || value
            .as_array()
            .is_some_and(|values| values.iter().any(|v| v.as_str() == Some(expected)))
}
fn statements(policy: &Value) -> &[Value] {
    match &policy["Statement"] {
        Value::Array(statements) => statements.as_slice(),
        Value::Object(_) => std::slice::from_ref(&policy["Statement"]),
        _ => &[],
    }
}

fn private_policy(policy: &Value) -> bool {
    statements(policy)
        .iter()
        .all(|statement| match statement["Effect"].as_str() {
            Some("Deny") => true,
            Some("Allow") => {
                // Fail closed, even for conditional wildcard grants. IAM identity policies grant worker access.
                statement.get("NotPrincipal").is_none()
                    && statement["Principal"]
                        .as_object()
                        .is_some_and(|principals| {
                            !principals.is_empty() && principals.values().all(explicit_principal)
                        })
            }
            _ => false,
        })
}

fn explicit_principal(value: &Value) -> bool {
    match value {
        Value::String(principal) => !principal.is_empty() && !principal.contains(['*', '?']),
        Value::Array(principals) => {
            !principals.is_empty()
                && principals
                    .iter()
                    .all(|principal| principal.is_string() && explicit_principal(principal))
        }
        _ => false,
    }
}

fn tls_denied(policy: &Value, arn: &str) -> bool {
    statements(policy).iter().any(|s| {
        s["Effect"] == "Deny"
            && (s["Principal"] == "*" || s["Principal"]["AWS"] == "*")
            && (contains(&s["Action"], "sqs:*") || contains(&s["Action"], "*"))
            && (contains(&s["Resource"], arn) || contains(&s["Resource"], "*"))
            && s.get("NotAction").is_none()
            && s.get("NotResource").is_none()
            && s.get("NotPrincipal").is_none()
            && s["Condition"].as_object().is_some_and(|c| c.len() == 1)
            && s["Condition"]["Bool"]
                .as_object()
                .is_some_and(|c| c.len() == 1)
            && (s["Condition"]["Bool"]["aws:SecureTransport"] == false
                || s["Condition"]["Bool"]["aws:SecureTransport"] == "false")
    })
}

#[cfg(test)]
mod tests {
    use super::{
        Attributes, CdcRouterQueueConfig, QueueError, SqsQueueConfig, private_policy, tls_denied,
        validate_attributes,
    };
    use aura_historia_jobs::{WorkerQueueType, WorkerScope};
    use aws_sdk_sqs::types::QueueAttributeName as A;
    use serde_json::json;
    use std::collections::HashMap;

    #[test]
    fn deployed_names_and_visibility_stay_stable() {
        let expected = [
            ("search-filter-match-notification", 300),
            ("watchlist-notification", 300),
            ("product-content-assessment", 270),
            ("product-translation", 300),
            ("product-embedding", 360),
            ("product-listing-opensearch", 300),
            ("product-listing-normalization", 270),
            ("notification-delivery", 330),
            ("marketing-consent-sync", 330),
            ("geo-resolution", 300),
            ("listing-geography", 300),
            ("listing-shipping", 300),
            ("user-identity-deletion", 330),
        ];
        assert_eq!(13, expected.len());
        assert_eq!(WorkerScope::ALL.len(), expected.len());
        for (scope, (name, seconds)) in WorkerScope::ALL.into_iter().zip(expected) {
            assert_eq!(scope.as_str(), name);
            let suffix = if scope.queue_type() == WorkerQueueType::Fifo {
                ".fifo"
            } else {
                ""
            };
            let url = format!(
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{name}-prod{suffix}"
            );
            let config = SqsQueueConfig::new(
                scope,
                url.parse().unwrap(),
                "eu-central-1".into(),
                "prod".into(),
                None,
            )
            .unwrap();
            assert_eq!(config.queue_url().as_str(), url);
            assert_eq!(config.visibility_timeout().as_secs(), seconds);
            assert_eq!(
                config.dlq_url().path(),
                format!("/123456789012/aura-worker-{name}-dlq-prod{suffix}")
            );
        }
    }

    #[test]
    fn removed_worker_queue_names_are_not_valid_destinations() {
        for removed in ["search-filter-projection", "search-filter-percolator"] {
            for scope in WorkerScope::ALL {
                assert_eq!(
                Some(QueueError::InvalidConfig("queue URL")),
                SqsQueueConfig::new(
                    scope,
                    format!("https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{removed}-prod")
                        .parse()
                        .unwrap(),
                    "eu-central-1".into(),
                    "prod".into(),
                    None,
                )
                .err(),
            );
            }
        }
    }

    #[test]
    fn fifo_destination_identity_fails_closed_on_stage_region_account_or_type_mismatch() {
        let scope = WorkerScope::MarketingConsentSync;
        for (url, region, stage) in [
            (
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-marketing-consent-sync-dev.fifo",
                "eu-central-1",
                "prod",
            ),
            (
                "https://sqs.us-east-1.amazonaws.com/123456789012/aura-worker-marketing-consent-sync-prod.fifo",
                "eu-central-1",
                "prod",
            ),
            (
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-marketing-consent-sync-prod",
                "eu-central-1",
                "prod",
            ),
            (
                "https://sqs.eu-central-1.amazonaws.com/not-an-account/aura-worker-marketing-consent-sync-prod.fifo",
                "eu-central-1",
                "prod",
            ),
        ] {
            assert!(
                SqsQueueConfig::new(
                    scope,
                    url.parse().unwrap(),
                    region.into(),
                    stage.into(),
                    None
                )
                .is_err()
            );
        }

        let config = SqsQueueConfig::new(
        scope,
        "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-marketing-consent-sync-prod.fifo"
            .parse()
            .unwrap(),
        "eu-central-1".into(),
        "prod".into(),
        None,
    )
    .unwrap();
        let mut source = attributes(&config, false);
        source.insert(
        A::RedrivePolicy,
        json!({
            "deadLetterTargetArn": "arn:aws:sqs:eu-central-1:999999999999:aura-worker-marketing-consent-sync-dlq-prod.fifo",
            "maxReceiveCount": 5
        })
        .to_string(),
    );
        assert!(validate_attributes(&config, &source, false).is_err());
    }

    #[test]
    fn router_requires_all_scoped_urls() {
        let mut values = HashMap::from([
            ("AWS_REGION", "eu-central-1".to_owned()),
            ("STAGE", "prod".to_owned()),
        ]);
        for scope in WorkerScope::ALL {
            let suffix = if scope.queue_type() == WorkerQueueType::Fifo {
                ".fifo"
            } else {
                ""
            };
            values.insert(
            scope.router_queue_url_env(),
            format!(
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-prod{suffix}",
                scope.as_str(),
            ),
        );
        }
        let queues = CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned())
            .unwrap()
            .into_queues();
        assert_eq!(queues.len(), WorkerScope::ALL.len());
        assert_eq!(
            queues.iter().map(SqsQueueConfig::scope).collect::<Vec<_>>(),
            WorkerScope::ALL
        );
        for scope in WorkerScope::ALL
            .into_iter()
            .filter(|scope| *scope != WorkerScope::MarketingConsentSync)
        {
            let name = scope.router_queue_url_env();
            let value = values.remove(name).unwrap();
            assert_eq!(
                CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned()).err(),
                Some(QueueError::MissingConfig(name))
            );
            values.insert(name, value);
        }
    }

    #[test]
    fn marketing_consent_router_url_is_optional_before_c06_activation() {
        let mut values = HashMap::from([
            ("AWS_REGION", "eu-central-1".to_owned()),
            ("STAGE", "prod".to_owned()),
        ]);
        for scope in WorkerScope::ALL
            .into_iter()
            .filter(|scope| *scope != WorkerScope::MarketingConsentSync)
        {
            values.insert(
                scope.router_queue_url_env(),
                format!(
                    "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-prod",
                    scope.as_str()
                ),
            );
        }
        let queues = CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned())
            .unwrap()
            .into_queues();
        assert_eq!(WorkerScope::ALL.len() - 1, queues.len());
    }

    fn attributes(config: &SqsQueueConfig, dlq: bool) -> Attributes {
        let mut attrs = HashMap::from([
            (A::QueueArn, config.arn(dlq)),
            (A::SqsManagedSseEnabled, "true".into()),
            (
                A::MessageRetentionPeriod,
                if dlq { "1209600" } else { "604800" }.into(),
            ),
            (
                A::VisibilityTimeout,
                config.visibility_timeout().as_secs().to_string(),
            ),
            (A::ReceiveMessageWaitTimeSeconds, "20".into()),
            (
                A::Policy,
                json!({"Statement": [{"Effect":"Deny", "Principal":"*", "Action":"sqs:*",
            "Resource":config.arn(dlq), "Condition":{"Bool":{"aws:SecureTransport":"false"}}}]})
                .to_string(),
            ),
            (
                A::RedriveAllowPolicy,
                if dlq {
                    json!({"redrivePermission":"byQueue", "sourceQueueArns":[config.arn(false)]})
                } else {
                    json!({"redrivePermission":"denyAll"})
                }
                .to_string(),
            ),
        ]);
        if !dlq {
            attrs.insert(
                A::RedrivePolicy,
                json!({"deadLetterTargetArn":config.arn(true), "maxReceiveCount":5}).to_string(),
            );
        }
        if config.queue_type() == WorkerQueueType::Fifo {
            attrs.insert(A::FifoQueue, "true".into());
            attrs.insert(A::ContentBasedDeduplication, "false".into());
        }
        attrs
    }

    #[test]
    fn source_and_dlq_checks_fail_closed() {
        for scope in WorkerScope::ALL {
            let config = SqsQueueConfig::new(
                scope,
                format!(
                    "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-prod{}",
                    scope.as_str(),
                    if scope.queue_type() == WorkerQueueType::Fifo {
                        ".fifo"
                    } else {
                        ""
                    }
                )
                .parse()
                .unwrap(),
                "eu-central-1".into(),
                "prod".into(),
                None,
            )
            .unwrap();
            for dlq in [false, true] {
                let good = attributes(&config, dlq);
                assert_eq!(validate_attributes(&config, &good, dlq), Ok(()));
                for key in [
                    A::QueueArn,
                    A::MessageRetentionPeriod,
                    A::Policy,
                    A::RedriveAllowPolicy,
                ] {
                    let mut broken = good.clone();
                    broken.remove(&key);
                    assert!(validate_attributes(&config, &broken, dlq).is_err());
                }
                let wrong_fifo = if config.queue_type() == WorkerQueueType::Fifo {
                    (A::FifoQueue, "false")
                } else {
                    (A::FifoQueue, "true")
                };
                for (key, value) in [
                    wrong_fifo,
                    (A::SqsManagedSseEnabled, "false"),
                    (A::Policy, "{}"),
                    (A::RedriveAllowPolicy, "{}"),
                ] {
                    let mut broken = good.clone();
                    broken.insert(key, value.into());
                    assert!(validate_attributes(&config, &broken, dlq).is_err());
                }
                if config.queue_type() == WorkerQueueType::Fifo {
                    let mut broken = good.clone();
                    broken.insert(A::ContentBasedDeduplication, "true".into());
                    assert!(validate_attributes(&config, &broken, dlq).is_err());
                    let mut broken = good.clone();
                    broken.remove(&A::ContentBasedDeduplication);
                    assert!(validate_attributes(&config, &broken, dlq).is_err());
                }
                let mut broken = good;
                if dlq {
                    broken.insert(A::RedrivePolicy, "{}".into());
                } else {
                    broken.insert(
                        A::RedrivePolicy,
                        json!({"deadLetterTargetArn":config.arn(true), "maxReceiveCount":6})
                            .to_string(),
                    );
                }
                assert!(validate_attributes(&config, &broken, dlq).is_err());
            }
        }
    }

    #[test]
    fn denies_must_cover_all_insecure_calls() {
        let arn = "arn:aws:sqs:eu-central-1:123456789012:queue";
        let statement = json!({"Effect":"Deny", "Principal":"*", "Action":"sqs:*", "Resource":arn,
        "Condition":{"Bool":{"aws:SecureTransport":"false"}}});
        assert!(tls_denied(&json!({"Statement":statement}), arn));
        for (field, value) in [
            ("Effect", json!("Allow")),
            ("Principal", json!({"AWS":"restricted"})),
            ("Action", json!("sqs:SendMessage")),
            ("Resource", json!("other")),
            ("Condition", json!({"Bool":{"aws:SecureTransport":"true"}})),
            (
                "Condition",
                json!({"Bool":{"aws:SecureTransport":"false"}, "StringEquals":{"aws:SourceArn":"other"}}),
            ),
        ] {
            let mut bad = statement.clone();
            bad[field] = value;
            assert!(!tls_denied(&json!({"Statement":[bad]}), arn));
        }
    }

    #[test]
    fn only_explicit_principals_are_private() {
        let allow = json!({"Effect":"Allow", "Principal":{"AWS":["arn:aws:iam::123456789012:role/worker"]},
        "Action":"sqs:SendMessage", "Resource":"*"});
        assert!(private_policy(&json!({"Statement":allow})));
        let mut broken = allow;
        broken["NotPrincipal"] = json!({"AWS":"arn:aws:iam::123456789012:role/other"});
        assert!(!private_policy(&json!({"Statement":broken})));
    }

    #[test]
    fn global_endpoint_override_is_not_accepted() {
        let config = CdcRouterQueueConfig::from_getter(|name| match name {
            "AWS_REGION" => Some("eu-central-1".into()),
            "STAGE" => Some("prod".into()),
            "AWS_ENDPOINT_URL" => Some("https://other.example".into()),
            _ => None,
        });
        assert_eq!(
            config.err(),
            Some(QueueError::InvalidConfig(
                "AWS_ENDPOINT_URL (use AWS_ENDPOINT_URL_SQS)"
            ))
        );
    }
}
