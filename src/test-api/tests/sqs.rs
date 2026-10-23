use aws_sdk_sqs::types::QueueAttributeName as A;
use std::{collections::HashSet, sync::LazyLock};
use test_api::{
    IntegrationTestService, Sqs, SqsQueuePair, WorkerSqs, aura_integration_test, aws,
    get_sqs_client,
};

// This case exercises the legacy purge contract under the macro's real exclusive gate.
static UNIQUE_A: LazyLock<SqsQueuePair> = LazyLock::new(|| SqsQueuePair::unique("scoped-cleanup"));
static UNIQUE_B: LazyLock<SqsQueuePair> = LazyLock::new(|| SqsQueuePair::unique("scoped-cleanup"));

const SQS: Sqs = Sqs { name: "test-sqs" };

#[aura_integration_test(services = [SQS], isolated = true)]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_run_without_errors() {
    assert!(SQS.queue_url().starts_with(aws::get_endpoint_url()));
}

#[aura_integration_test(services = [SQS, &*UNIQUE_A, &*UNIQUE_B])]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_purge_only_its_source_and_dlq_including_invisible_messages() {
    let client = get_sqs_client().await;
    assert_ne!(UNIQUE_A.name, UNIQUE_B.name);
    assert!(UNIQUE_A.name.contains(&std::process::id().to_string()));
    for url in [
        UNIQUE_A.queue_url(),
        UNIQUE_A.dead_letter_queue_url(),
        UNIQUE_B.queue_url(),
    ] {
        client
            .send_message()
            .queue_url(&url)
            .message_body("scoped fixture message")
            .send()
            .await
            .unwrap();
        let received = client
            .receive_message()
            .queue_url(&url)
            .visibility_timeout(60)
            .wait_time_seconds(0)
            .send()
            .await
            .unwrap();
        assert_eq!(1, received.messages().len());
    }
    UNIQUE_A.tear_down().await;
    for url in [UNIQUE_A.queue_url(), UNIQUE_A.dead_letter_queue_url()] {
        let result = client
            .get_queue_attributes()
            .queue_url(url)
            .attribute_names(A::All)
            .send()
            .await
            .unwrap();
        let attrs = result.attributes().unwrap();
        assert_eq!("0", attrs[&A::ApproximateNumberOfMessages]);
        assert_eq!("0", attrs[&A::ApproximateNumberOfMessagesNotVisible]);
    }
    let other = client
        .get_queue_attributes()
        .queue_url(UNIQUE_B.queue_url())
        .attribute_names(A::ApproximateNumberOfMessagesNotVisible)
        .send()
        .await
        .unwrap();
    assert_eq!(
        "1",
        other.attributes().unwrap()[&A::ApproximateNumberOfMessagesNotVisible]
    );
}

#[aura_integration_test(services = [SQS], isolated = true)]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_expose_production_queue_metadata_for_every_worker_visibility_class() {
    let context = test_api::TestContext::current().unwrap();
    let namespace = format!("a{}-{}-", context.run_id(), &context.test_id()[..16]);
    let mut names = HashSet::new();
    let expected = [
        ("product-listing-opensearch", 300),
        ("search-filter-match-notification", 300),
        ("watchlist-notification", 300),
        ("product-content-assessment", 270),
        ("product-embedding", 360),
        ("product-translation", 300),
        ("product-listing-normalization", 270),
        ("notification-delivery", 330),
        ("marketing-consent-sync", 330),
        ("geo-resolution", 300),
        ("listing-geography", 300),
    ];
    assert_eq!(11, expected.len());
    for (scope, visibility) in expected {
        let fifo = scope == "marketing-consent-sync";
        let queues = WorkerSqs::new(scope, visibility);
        queues.set_up().await;
        let source = get_sqs_client()
            .await
            .get_queue_attributes()
            .queue_url(queues.queue_url())
            .attribute_names(A::All)
            .send()
            .await
            .unwrap();
        let dlq = get_sqs_client()
            .await
            .get_queue_attributes()
            .queue_url(queues.dead_letter_queue_url())
            .attribute_names(A::All)
            .send()
            .await
            .unwrap();
        let source = source.attributes().unwrap();
        let dlq = dlq.attributes().unwrap();
        let source_name = source[&A::QueueArn].rsplit(':').next().unwrap();
        let dlq_name = dlq[&A::QueueArn].rsplit(':').next().unwrap();
        for name in [source_name, dlq_name] {
            assert!(name.starts_with(&namespace));
            assert_eq!(fifo, name.ends_with(".fifo"));
            assert!(
                names.insert(name.to_owned()),
                "worker queues must be distinct"
            );
        }
        assert_eq!(
            format!("{}/000000000000/{source_name}", aws::get_endpoint_url()),
            queues.queue_url()
        );
        assert_eq!("604800", source[&A::MessageRetentionPeriod]);
        assert_eq!("1209600", dlq[&A::MessageRetentionPeriod]);
        assert_eq!(visibility.to_string(), source[&A::VisibilityTimeout]);
        assert_eq!("20", source[&A::ReceiveMessageWaitTimeSeconds]);
        for attrs in [source, dlq] {
            if fifo {
                assert_eq!("true", attrs[&A::FifoQueue]);
                assert_eq!("false", attrs[&A::ContentBasedDeduplication]);
            } else {
                assert_ne!(Some("true"), attrs.get(&A::FifoQueue).map(String::as_str));
            }
            assert_eq!("true", attrs[&A::SqsManagedSseEnabled]);
            let policy: serde_json::Value = serde_json::from_str(&attrs[&A::Policy]).unwrap();
            assert_eq!(
                serde_json::json!({"Version":"2012-10-17", "Statement":[{
                    "Sid":"DenyInsecureTransport", "Effect":"Deny", "Principal":"*", "Action":"sqs:*",
                    "Resource":attrs[&A::QueueArn], "Condition":{"Bool":{"aws:SecureTransport":"false"}},
                }]}),
                policy
            );
        }
        let source_allow: serde_json::Value =
            serde_json::from_str(&source[&A::RedriveAllowPolicy]).unwrap();
        assert_eq!(
            serde_json::json!({"redrivePermission": "denyAll"}),
            source_allow
        );
        let redrive: serde_json::Value = serde_json::from_str(&source[&A::RedrivePolicy]).unwrap();
        assert_eq!(
            serde_json::json!({"deadLetterTargetArn":dlq[&A::QueueArn],"maxReceiveCount":5}),
            redrive
        );
        let allow: serde_json::Value = serde_json::from_str(&dlq[&A::RedriveAllowPolicy]).unwrap();
        assert_eq!(
            serde_json::json!({"redrivePermission":"byQueue","sourceQueueArns":[source[&A::QueueArn]]}),
            allow
        );
        queues.tear_down().await;
    }
}

#[tokio::test]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_isolate_worker_queue_from_previous_tests_cancelled_long_poll() {
    aws::get_aws_emulator(&["sqs"], &[]).await;
    let previous =
        test_api::TestContext::acquire("cancelled worker poll", test_api::Isolation::Isolated)
            .await
            .unwrap();
    previous
        .context()
        .scope(async {
            SQS.set_up().await;
            let queues = WorkerSqs::new("product-content-assessment", 60);
            queues.set_up().await;
            let queue_url = queues.queue_url();
            let receive = test_api::spawn_owned(async move {
                get_sqs_client()
                    .await
                    .receive_message()
                    .queue_url(queue_url)
                    .wait_time_seconds(20)
                    .send()
                    .await
            });
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            assert!(
                !receive.is_finished(),
                "previous test has a pending long poll"
            );
            receive.abort();
            assert!(receive.await.unwrap_err().is_cancelled());
            queues.tear_down().await;
        })
        .await;
    previous.finish().await.unwrap();

    let next =
        test_api::TestContext::acquire("worker poll slot reuse", test_api::Isolation::Isolated)
            .await
            .unwrap();
    next.context()
        .scope(async {
            let queues = WorkerSqs::new("product-content-assessment", 60);
            queues.set_up().await;
            get_sqs_client()
                .await
                .send_message()
                .queue_url(queues.queue_url())
                .message_body("next test message")
                .send()
                .await
                .unwrap();
            let received = get_sqs_client()
                .await
                .receive_message()
                .queue_url(queues.queue_url())
                .wait_time_seconds(1)
                .send()
                .await
                .unwrap();
            queues.tear_down().await;
            assert_eq!(
                1,
                received.messages().len(),
                "cancelled old receive must not hide new test work"
            );
            assert_eq!(Some("next test message"), received.messages()[0].body());
        })
        .await;
    next.finish().await.unwrap();
}

#[aura_integration_test(services = [SQS], isolated = true)]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_post_to_sqs() {
    let client = get_sqs_client().await;
    let _ = client
        .send_message()
        .queue_url(SQS.queue_url())
        .message_body(r#"{"foo":"bar"}"#)
        .send()
        .await
        .unwrap();

    let res = client
        .receive_message()
        .queue_url(SQS.queue_url())
        .send()
        .await
        .unwrap();

    assert_eq!(
        r#"{"foo":"bar"}"#,
        res.messages.unwrap().remove(0).body.unwrap()
    )
}

#[aura_integration_test(services = [SQS, Sqs { name: "test-sqs-foo" }], isolated = true)]
#[ignore = "fixture self-test; run explicitly with --ignored"]
async fn should_create_multiple_sqs() {
    let client = get_sqs_client().await;

    let list_qs = client.list_queues().max_results(1000).send().await.unwrap();

    // 2 Q + 2 DLQ
    let expected = [
        SQS.queue_url(),
        SQS.dead_letter_queue_url(),
        Sqs {
            name: "test-sqs-foo",
        }
        .queue_url(),
        Sqs {
            name: "test-sqs-foo",
        }
        .dead_letter_queue_url(),
    ];
    let actual = list_qs.queue_urls.unwrap();
    assert_eq!(
        4,
        expected
            .iter()
            .filter(|url| actual
                .iter()
                .any(|returned| returned.rsplit('/').next() == url.rsplit('/').next()))
            .count()
    );
}
