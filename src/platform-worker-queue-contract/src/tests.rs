use super::{
    Attributes, CdcRouterQueueConfig, QueueError, SqsQueueConfig, private_policy, tls_denied,
    validate_attributes,
};
use aura_historia_jobs::WorkerScope;
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
    ];
    assert_eq!(WorkerScope::ALL.len(), expected.len());
    for (scope, (name, seconds)) in WorkerScope::ALL.into_iter().zip(expected) {
        assert_eq!(scope.as_str(), name);
        let url =
            format!("https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{name}-prod");
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
            format!("/123456789012/aura-worker-{name}-dlq-prod")
        );
    }
}

#[test]
fn router_requires_all_ten_scoped_urls() {
    let mut values = HashMap::from([
        ("AWS_REGION", "eu-central-1".to_owned()),
        ("STAGE", "prod".to_owned()),
    ]);
    for scope in WorkerScope::ALL {
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
    assert_eq!(queues.len(), 10);
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
    attrs
}

#[test]
fn source_and_dlq_checks_fail_closed() {
    for scope in WorkerScope::ALL {
        let config = SqsQueueConfig::new(
            scope,
            format!(
                "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-{}-prod",
                scope.as_str()
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
            for (key, value) in [
                (A::FifoQueue, "true"),
                (A::SqsManagedSseEnabled, "false"),
                (A::Policy, "{}"),
                (A::RedriveAllowPolicy, "{}"),
            ] {
                let mut broken = good.clone();
                broken.insert(key, value.into());
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
