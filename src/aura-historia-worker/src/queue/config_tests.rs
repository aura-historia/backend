use super::{
    AWS_REGION_ENV, Attributes, CdcRouterQueueConfig, QueueError, SQS_ENDPOINT_ENV, SqsQueueConfig,
    WORKER_QUEUE_URL_ENV, validate_attributes,
};
use crate::WorkerScope;
use aws_sdk_sqs::types::QueueAttributeName;
use serde_json::{Value, json};
use std::collections::HashMap;
use strum::IntoEnumIterator;
use url::Url;

fn config(scope: WorkerScope) -> SqsQueueConfig {
    SqsQueueConfig::new(
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
    .unwrap()
}
fn attributes(config: &SqsQueueConfig, dlq: bool) -> Attributes {
    use QueueAttributeName as A;
    let mut attributes = HashMap::from([
        (A::QueueArn, config.arn(dlq)),
        (A::FifoQueue, "false".into()),
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
            json!({"Statement": [{"Effect": "Deny", "Principal": "*", "Action": "sqs:*",
            "Resource": config.arn(dlq), "Condition": {"Bool": {"aws:SecureTransport": "false"}}}]})
            .to_string(),
        ),
        (
            A::RedrivePolicy,
            json!({"deadLetterTargetArn": config.arn(true), "maxReceiveCount": 5}).to_string(),
        ),
    ]);
    attributes.insert(
        A::RedriveAllowPolicy,
        if dlq {
            json!({"redrivePermission":"byQueue", "sourceQueueArns":[config.arn(false)]})
        } else {
            json!({"redrivePermission":"denyAll"})
        }
        .to_string(),
    );
    if dlq {
        attributes.remove(&A::RedrivePolicy);
    }
    attributes
}

#[test]
fn should_require_one_distinct_router_queue_url_for_every_production_scope() {
    let mut values = HashMap::from([
        (AWS_REGION_ENV, "eu-central-1".to_owned()),
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

    let config = CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned())
        .expect("all router queues configured");
    let queues = config.into_queues();
    assert_eq!(10, queues.len());
    assert_eq!(
        WorkerScope::ALL.to_vec(),
        queues.iter().map(SqsQueueConfig::scope).collect::<Vec<_>>()
    );

    values.remove("STAGE");
    assert_eq!(
        Err(QueueError::MissingConfig("STAGE")),
        CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned()).map(|_| ())
    );
    values.insert("STAGE", "prod".to_owned());
    values.remove(WorkerScope::NotificationDelivery.router_queue_url_env());
    assert_eq!(
        Err(QueueError::MissingConfig(
            "AURA_HISTORIA_ROUTER_QUEUE_URL_NOTIFICATION_DELIVERY"
        )),
        CdcRouterQueueConfig::from_getter(|name| values.get(name).cloned()).map(|_| ())
    );
}

#[test]
fn should_validate_exact_ten_scope_queue_and_dlq_contracts() {
    let scopes: Vec<_> = WorkerScope::iter().collect();
    assert_eq!(10, scopes.len());
    let names: std::collections::HashSet<_> = scopes.iter().map(|scope| scope.as_str()).collect();
    assert_eq!(10, names.len());
    for scope in scopes {
        let config = config(scope);
        for dlq in [false, true] {
            assert_eq!(
                Ok(()),
                validate_attributes(&config, &attributes(&config, dlq), dlq)
            );
        }
        assert_eq!(
            match scope {
                WorkerScope::NotificationDelivery
                | WorkerScope::SearchFilterPercolator
                | WorkerScope::ProductListingTranslation
                | WorkerScope::ProductListingEmbedding
                | WorkerScope::ProductListingOpenSearch
                | WorkerScope::ProductListingRawNormalization => 240,
                WorkerScope::SearchFilterProjection
                | WorkerScope::SearchFilterMatchNotification
                | WorkerScope::WatchlistNotification
                | WorkerScope::ProductListingContentAssessment => 45,
            },
            config.execution_budget().as_secs()
        );
        assert_eq!(
            match scope {
                WorkerScope::ProductListingOpenSearch
                | WorkerScope::SearchFilterProjection
                | WorkerScope::SearchFilterPercolator
                | WorkerScope::SearchFilterMatchNotification
                | WorkerScope::WatchlistNotification
                | WorkerScope::ProductListingTranslation => 300,
                WorkerScope::ProductListingContentAssessment
                | WorkerScope::ProductListingRawNormalization => 270,
                WorkerScope::ProductListingEmbedding => 360,
                WorkerScope::NotificationDelivery => 330,
            },
            config.visibility_timeout().as_secs()
        );
        assert!(
            config
                .dlq_url()
                .path()
                .ends_with(&format!("aura-worker-{}-dlq-prod", scope.as_str()))
        );
    }
}
#[test]
fn should_fail_closed_for_wrong_identity_attributes_encryption_or_tls() {
    use QueueAttributeName as A;
    for scope in WorkerScope::iter() {
        let config = config(scope);
        for dlq in [false, true] {
            let good = attributes(&config, dlq);
            for (key, bad) in [
                (A::QueueArn, "arn:aws:sqs:other:123456789012:wrong"),
                (A::FifoQueue, "true"),
                (A::MessageRetentionPeriod, "1"),
                (A::SqsManagedSseEnabled, "false"),
                (A::Policy, "{}"),
                (A::Policy, "not-json"),
            ] {
                let mut broken = good.clone();
                broken.insert(key, bad.into());
                assert!(validate_attributes(&config, &broken, dlq).is_err());
            }
            for key in [
                A::QueueArn,
                A::MessageRetentionPeriod,
                A::SqsManagedSseEnabled,
                A::Policy,
            ] {
                let mut broken = good.clone();
                broken.remove(&key);
                assert!(validate_attributes(&config, &broken, dlq).is_err());
            }
        }
        for (key, bad) in [
            (A::VisibilityTimeout, "59"),
            (A::ReceiveMessageWaitTimeSeconds, "0"),
            (A::RedrivePolicy, "{}"),
            (
                A::RedrivePolicy,
                "{\"deadLetterTargetArn\":\"wrong\",\"maxReceiveCount\":5}",
            ),
        ] {
            let mut broken = attributes(&config, false);
            broken.insert(key, bad.into());
            assert!(validate_attributes(&config, &broken, false).is_err());
        }
        let mut wrong_count = attributes(&config, false);
        wrong_count.insert(
            A::RedrivePolicy,
            json!({"deadLetterTargetArn": config.arn(true), "maxReceiveCount": 6}).to_string(),
        );
        assert!(validate_attributes(&config, &wrong_count, false).is_err());
    }
}
#[test]
fn should_accept_aws_standard_absent_fifo_and_kms_encryption() {
    let config = config(WorkerScope::NotificationDelivery);
    let mut attrs = attributes(&config, false);
    attrs.remove(&QueueAttributeName::FifoQueue);
    attrs.remove(&QueueAttributeName::SqsManagedSseEnabled);
    attrs.insert(QueueAttributeName::KmsMasterKeyId, "alias/aws/sqs".into());
    assert_eq!(Ok(()), validate_attributes(&config, &attrs, false));
}

#[test]
fn should_reject_wrong_url_scope_stage_region_account_and_fifo() {
    let scope = WorkerScope::NotificationDelivery;
    for url in [
        "http://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-prod",
        "https://sqs.eu-west-1.amazonaws.com/123456789012/aura-worker-notification-delivery-prod",
        "https://evil.example/123456789012/aura-worker-notification-delivery-prod",
        "https://sqs.eu-central-1.amazonaws.com/123/aura-worker-notification-delivery-prod",
        "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-test",
        "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-product-translation-prod",
        "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-prod.fifo",
        "https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-prod?x=1",
        "https://user:secret@sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-prod",
    ] {
        assert!(
            SqsQueueConfig::new(
                scope,
                url.parse().unwrap(),
                "eu-central-1".into(),
                "prod".into(),
                None
            )
            .is_err()
        );
    }
}
#[test]
fn should_allow_localstack_only_with_explicit_development_endpoint() {
    for stage in ["prod", "develop", "ephemeral", "local", "test"] {
        let url =
            format!("http://localhost:4566/000000000000/aura-worker-notification-delivery-{stage}")
                .parse()
                .unwrap();
        let result = SqsQueueConfig::new(
            WorkerScope::NotificationDelivery,
            url,
            "eu-central-1".into(),
            stage.into(),
            Some("http://localhost:4566".parse().unwrap()),
        );
        assert_eq!(
            matches!(stage, "ephemeral" | "local" | "test"),
            result.is_ok()
        );
    }
    let url = "http://localhost:4566/000000000000/aura-worker-notification-delivery-test"
        .parse()
        .unwrap();
    assert!(
        SqsQueueConfig::new(
            WorkerScope::NotificationDelivery,
            url,
            "eu-central-1".into(),
            "test".into(),
            None
        )
        .is_err()
    );
}
#[test]
fn should_require_private_policies_and_exclusive_dlq_redrive() {
    use QueueAttributeName as A;
    let config = config(WorkerScope::NotificationDelivery);
    for dlq in [false, true] {
        let good = attributes(&config, dlq);
        for principal in [
            json!("*"),
            json!({"AWS":"*"}),
            json!({"AWS":["123456789012", "*"]}),
            json!({"Service":"*"}),
            json!({"AWS":"arn:aws:iam::*:root"}),
            json!({}),
            Value::Null,
        ] {
            let mut broken = good.clone();
            let mut policy: Value = serde_json::from_str(&broken[&A::Policy]).unwrap();
            policy["Statement"].as_array_mut().unwrap().push(json!({"Effect":"Allow", "Principal":principal,
                "Action":"sqs:SendMessage", "Resource":config.arn(dlq), "Condition":{"Bool":{"aws:SecureTransport":"true"}}}));
            broken.insert(A::Policy, policy.to_string());
            assert_eq!(
                Err(QueueError::Attribute("public access policy")),
                validate_attributes(&config, &broken, dlq)
            );
        }
        let mut broken = good.clone();
        broken.remove(&A::RedriveAllowPolicy);
        assert_eq!(
            Err(QueueError::Attribute("RedriveAllowPolicy")),
            validate_attributes(&config, &broken, dlq)
        );
        for allow in [
            json!({}),
            json!({"redrivePermission":"allowAll"}),
            json!({"redrivePermission":"byQueue", "sourceQueueArns":[config.arn(true)]}),
            json!({"redrivePermission":"byQueue", "sourceQueueArns":[config.arn(false), "other"]}),
            json!({"redrivePermission":"byQueue", "sourceQueueArns":[]}),
        ] {
            let mut broken = good.clone();
            broken.insert(A::RedriveAllowPolicy, allow.to_string());
            assert_eq!(
                Err(QueueError::Attribute("RedriveAllowPolicy")),
                validate_attributes(&config, &broken, dlq)
            );
        }
    }
    let mut broken = attributes(&config, true);
    broken.insert(
        A::RedrivePolicy,
        json!({"deadLetterTargetArn":config.arn(false), "maxReceiveCount":5}).to_string(),
    );
    assert_eq!(
        Err(QueueError::Attribute("DLQ RedrivePolicy")),
        validate_attributes(&config, &broken, true)
    );
}

#[test]
fn should_reject_noncanonical_stages_and_endpoint_origin_bypasses() {
    let scope = WorkerScope::NotificationDelivery;
    for stage in [
        "",
        "TEST",
        "test ",
        "-test",
        "test-",
        "test/name",
        "test_name",
        &"a".repeat(81),
    ] {
        let url = format!("https://sqs.eu-central-1.amazonaws.com/123456789012/aura-worker-notification-delivery-{stage}").parse().unwrap();
        assert!(
            SqsQueueConfig::new(scope, url, "eu-central-1".into(), stage.into(), None).is_err()
        );
    }
    let url: Url = "http://localhost:4566/000000000000/aura-worker-notification-delivery-ephemeral"
        .parse()
        .unwrap();
    for endpoint in [
        "http://127.0.0.1:4566",
        "http://localhost:4567",
        "https://localhost:4566",
        "http://localhost:4566/path",
        "http://user:secret@localhost:4566",
        "http://localhost:4566?x=1",
    ] {
        assert!(
            SqsQueueConfig::new(
                scope,
                url.clone(),
                "eu-central-1".into(),
                "ephemeral".into(),
                Some(endpoint.parse().unwrap())
            )
            .is_err()
        );
    }
    for endpoint_env in ["AWS_ENDPOINT_URL", SQS_ENDPOINT_ENV] {
        let result = SqsQueueConfig::from_getter(scope, |name| match name {
            WORKER_QUEUE_URL_ENV => Some(config(scope).queue_url().to_string()),
            AWS_REGION_ENV => Some("eu-central-1".into()),
            "STAGE" => Some("prod".into()),
            name if name == endpoint_env => Some("http://localhost:4566".into()),
            _ => None,
        });
        assert!(result.is_err());
    }
}

#[test]
fn should_require_queue_url_region_and_stage() {
    let config = config(WorkerScope::NotificationDelivery);
    for absent in [WORKER_QUEUE_URL_ENV, AWS_REGION_ENV, "STAGE"] {
        let result = SqsQueueConfig::from_getter(config.scope(), |name| {
            if name == absent {
                return None;
            }
            match name {
                WORKER_QUEUE_URL_ENV => Some(config.queue_url().to_string()),
                AWS_REGION_ENV => Some("eu-central-1".into()),
                "STAGE" => Some("prod".into()),
                _ => None,
            }
        });
        assert_eq!(Some(QueueError::MissingConfig(absent)), result.err());
    }
}
