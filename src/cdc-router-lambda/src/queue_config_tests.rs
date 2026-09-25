use super::{Attributes, CdcRouterQueueConfig, QueueError, SqsQueueConfig, validate_attributes};
use crate::WorkerScope;
use aws_sdk_sqs::types::QueueAttributeName as A;
use serde_json::json;
use std::collections::HashMap;

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
fn exact_ten_scope_configuration_keeps_router_environment_identity() {
    let mut values = HashMap::from([
        ("AWS_REGION", "eu-central-1".to_owned()),
        ("STAGE", "prod".to_owned()),
    ]);
    for scope in WorkerScope::ALL {
        values.insert(
            scope.router_queue_url_env(),
            config(scope).queue_url().to_string(),
        );
    }
    let queues = CdcRouterQueueConfig::from_getter(|key| values.get(key).cloned())
        .unwrap()
        .into_queues();
    assert_eq!(10, queues.len());
    assert_eq!(
        WorkerScope::ALL.to_vec(),
        queues.iter().map(SqsQueueConfig::scope).collect::<Vec<_>>()
    );
    values.remove(WorkerScope::NotificationDelivery.router_queue_url_env());
    assert_eq!(
        Err(QueueError::MissingConfig(
            "AURA_HISTORIA_ROUTER_QUEUE_URL_NOTIFICATION_DELIVERY"
        )),
        CdcRouterQueueConfig::from_getter(|key| values.get(key).cloned()).map(|_| ())
    );
}

#[test]
fn validates_all_source_and_dlq_custody_attributes() {
    for scope in WorkerScope::ALL {
        let queue = config(scope);
        for dlq in [false, true] {
            let good = attributes(&queue, dlq);
            assert_eq!(Ok(()), validate_attributes(&queue, &good, dlq));
            for (name, bad) in [
                (A::QueueArn, "wrong"),
                (A::MessageRetentionPeriod, "1"),
                (A::SqsManagedSseEnabled, "false"),
                (A::Policy, "{}"),
                (A::FifoQueue, "true"),
                (A::RedriveAllowPolicy, "{}"),
            ] {
                let mut broken = good.clone();
                broken.insert(name, bad.into());
                assert!(validate_attributes(&queue, &broken, dlq).is_err());
            }
            if !dlq {
                let mut broken = good.clone();
                broken.insert(
                    A::RedrivePolicy,
                    json!({"deadLetterTargetArn":queue.arn(true), "maxReceiveCount":6}).to_string(),
                );
                assert!(validate_attributes(&queue, &broken, false).is_err());
            }
        }
    }
}
