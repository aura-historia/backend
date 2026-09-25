//! Independent expectations for every retained worker fixture and production queue contract.
#[allow(dead_code)]
mod support;

use aura_historia_worker::{WorkerScope, queue::SqsQueueConfig};
use aws_sdk_sqs::types::QueueAttributeName;

#[test]
fn should_match_all_ten_worker_fixture_visibility_contracts() {
    use WorkerScope as S;
    let expected = [
        (S::SearchFilterProjection, 300),
        (S::SearchFilterPercolator, 300),
        (S::SearchFilterMatchNotification, 300),
        (S::WatchlistNotification, 300),
        (S::ProductListingContentAssessment, 270),
        (S::ProductListingTranslation, 300),
        (S::ProductListingEmbedding, 360),
        (S::ProductListingOpenSearch, 300),
        (S::ProductListingRawNormalization, 270),
        (S::NotificationDelivery, 330),
    ];
    assert_eq!(10, WorkerScope::ALL.len());
    assert_eq!(WorkerScope::ALL, expected.map(|(scope, _)| scope));

    for (scope, visibility) in expected {
        let fixture = support::queues(scope).queues();
        assert_eq!(
            Some(&visibility.to_string()),
            fixture
                .attributes
                .get(&QueueAttributeName::VisibilityTimeout),
            "fixture visibility for {}",
            scope.as_str()
        );
        assert_eq!(5, fixture.max_receive_count, "{} redrive", scope.as_str());
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
        assert_eq!(
            visibility as u64,
            config.visibility_timeout().as_secs(),
            "{} production visibility",
            scope.as_str()
        );
    }
}
