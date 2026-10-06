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
        ("search-filter-projection", 300),
        ("search-filter-percolator", 300),
        ("search-filter-match-notification", 300),
        ("watchlist-notification", 300),
        ("product-content-assessment", 270),
        ("product-translation", 300),
        ("product-embedding", 360),
        ("product-listing-opensearch", 300),
        ("product-listing-normalization", 270),
        ("notification-delivery", 330),
        ("marketing-consent-sync", 300),
    ];
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
fn router_requires_all_eleven_scoped_urls() {
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
    assert_eq!(queues.len(), 11);
    assert_eq!(
        queues.iter().map(SqsQueueConfig::scope).collect::<Vec<_>>(),
        WorkerScope::ALL
    );
    values.remove(WorkerScope::NotificationDelivery.router_queue_url_env());
    assert_eq!(
        CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned()).err(),
        Some(QueueError::MissingConfig(
            "AURA_HISTORIA_ROUTER_QUEUE_URL_NOTIFICATION_DELIVERY"
        ))
    );
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
    assert_eq!(10, queues.len());
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
