//! The qualification of a ListingSource as a real art and antiques e-shop against PostgreSQL: the
//! gate the scope sync applies in `agent` mode, the queue the author agent works, what a decision
//! does to the source's label, and the append-only history of each
//! (`docs/crawler/README.md#listingsource-qualification`).

use async_trait::async_trait;
use crawler::authoring::CrawlerAuthoringMode;
use crawler::review::agent_decisions::{AgentDecisionPolicy, AgentLimits};
use crawler::review::model::{
    Qualification, QualificationDecision, QualificationLabelEffect, ReviewActor, SourceLabel,
    SourceLabelNote, SourceLabelRequest,
};
use crawler::review::readers::QualificationQueueReader;
use crawler::review::repository::{CrawlerReviewRepository, ReviewRepositoryError};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepositoryImpl, ListingSourceRegistrationService,
    ListingSourceRegistrationSource, ListingSourceSyncError, QualificationGate,
    RegisteredListingSource,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

struct FixedScope(Vec<RegisteredListingSource>);

#[async_trait]
impl ListingSourceRegistrationSource for FixedScope {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        Ok(self.0.clone())
    }
}

fn registered(listing_source_id: ListingSourceId, url: &str) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id,
        listing_source_name: ListingSourceName::try_from("Shop").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("shop").unwrap(),
        crawl_enabled: true,
        presentation_url: Some(url.to_owned()),
    }
}

async fn sync(pool: &PgPool, gate: QualificationGate, scope: Vec<RegisteredListingSource>) {
    ListingSourceRegistrationService::new(
        Box::new(FixedScope(scope)),
        Box::new(ListingSourceRegistrationRepositoryImpl::new(
            pool.clone(),
            gate,
        )),
    )
    .sync()
    .await
    .unwrap_or_else(|error| panic!("sync the scope: {error}"));
}

async fn crawl_enabled(pool: &PgPool, listing_source_id: ListingSourceId) -> bool {
    sqlx::query_scalar("SELECT crawl_enabled FROM listing_sources WHERE listing_source_id = $1")
        .bind(listing_source_id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn queued(pool: &PgPool) -> Vec<(ListingSourceId, bool)> {
    QualificationQueueReader::new(pool.clone())
        .read()
        .await
        .unwrap()
        .sources
        .into_iter()
        .map(|source| (source.listing_source_id, source.backfill))
        .collect()
}

fn note(text: &str) -> SourceLabelNote {
    SourceLabelNote::try_from(text.to_owned()).unwrap()
}

fn decision(
    qualification: Qualification,
    label: Option<SourceLabel>,
    text: &str,
) -> QualificationDecision {
    QualificationDecision::new(qualification, label, note(text)).unwrap()
}

/// Marks `listing_source_id` as a source that existed before qualification did, as the migration
/// marks every source it finds.
async fn make_backfill(pool: &PgPool, listing_source_id: ListingSourceId) {
    sqlx::query(
        "UPDATE listing_sources SET qualification_backfill = TRUE WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .execute(pool)
    .await
    .unwrap();
}

/// Under the gate a new source waits in the queue uncrawled until the author agent qualifies it,
/// and the next sync enables it; a source the agent cannot decide stays uncrawled. Without the
/// gate a new source is crawled at once and still waits in the queue.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_crawl_a_new_source_only_once_it_is_qualified_under_the_gate() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let antique = ListingSourceId::new();
    let unclear = ListingSourceId::new();
    let scope = || {
        vec![
            registered(antique, "https://antique.example/"),
            registered(unclear, "https://unclear.example/"),
        ]
    };

    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(!crawl_enabled(&pool, antique).await);
    assert!(!crawl_enabled(&pool, unclear).await);
    let mut waiting = queued(&pool).await;
    waiting.sort();
    let mut expected = vec![(antique, false), (unclear, false)];
    expected.sort();
    assert_eq!(waiting, expected);

    let outcome = reviews
        .decide_qualification(
            &antique,
            ReviewActor::AgentAuthor,
            &decision(
                Qualification::Qualified,
                None,
                "/ sells 18th-century furniture with prices and a cart",
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome.label_effect, QualificationLabelEffect::None);
    reviews
        .decide_qualification(
            &unclear,
            ReviewActor::AgentAuthor,
            &decision(
                Qualification::NeedsHuman,
                None,
                "/ answered a bot challenge",
            ),
        )
        .await
        .unwrap();
    assert!(queued(&pool).await.is_empty());

    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(crawl_enabled(&pool, antique).await);
    assert!(!crawl_enabled(&pool, unclear).await, "fails closed");

    let ungated = ListingSourceId::new();
    sync(
        &pool,
        QualificationGate::Off,
        vec![registered(ungated, "https://ungated.example/")],
    )
    .await;
    assert!(crawl_enabled(&pool, ungated).await);
    assert_eq!(queued(&pool).await, vec![(ungated, false)]);
}

/// A source that existed before qualification did keeps being crawled while it waits for its
/// first qualification, and stops once it is disqualified, with the label and both histories.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_keep_crawling_a_backfill_source_until_it_is_disqualified() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let furniture = ListingSourceId::new();
    let scope = || vec![registered(furniture, "https://furniture.example/")];
    sync(&pool, QualificationGate::Off, scope()).await;
    make_backfill(&pool, furniture).await;

    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(crawl_enabled(&pool, furniture).await);
    assert_eq!(queued(&pool).await, vec![(furniture, true)]);

    let outcome = reviews
        .decide_qualification(
            &furniture,
            ReviewActor::AgentAuthor,
            &decision(
                Qualification::Disqualified,
                Some(SourceLabel::NotAnAntiqueShop),
                "/ and /sofas sell new furniture only",
            ),
        )
        .await
        .unwrap();
    assert_eq!(outcome.label_effect, QualificationLabelEffect::Labelled);
    assert!(!crawl_enabled(&pool, furniture).await);
    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(!crawl_enabled(&pool, furniture).await);

    let qualifications: Vec<(Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT qualification, source_label, actor FROM listing_source_qualification_events \
         WHERE listing_source_id = $1 ORDER BY occurred_at",
    )
    .bind(furniture.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        qualifications,
        vec![(
            Some("DISQUALIFIED".to_owned()),
            Some("NOT_AN_ANTIQUE_SHOP".to_owned()),
            "AGENT_AUTHOR".to_owned()
        )]
    );
    let labels: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT event_kind, source_label_note FROM listing_source_label_events \
         WHERE listing_source_id = $1",
    )
    .bind(furniture.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        labels,
        vec![(
            "LABELLED".to_owned(),
            Some("/ and /sofas sell new furniture only".to_owned())
        )]
    );
}

/// A review agent decides a source once, never as the reviewer, and replaces a label a human set
/// only in the backfill; it never clears a label: qualifying a labelled source keeps the label
/// for a human, who clears it by qualifying the source.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_let_an_agent_replace_a_human_label_in_the_backfill_only_and_clear_none() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let new_source = ListingSourceId::new();
    let backfill_source = ListingSourceId::new();
    let scope = || {
        vec![
            registered(new_source, "https://new.example/"),
            registered(backfill_source, "https://old.example/"),
        ]
    };
    sync(&pool, QualificationGate::Required, scope()).await;
    make_backfill(&pool, backfill_source).await;
    let human_label = SourceLabelRequest::new(SourceLabel::Other, Some(note("odd shop"))).unwrap();
    for listing_source_id in [new_source, backfill_source] {
        reviews
            .label_source(&listing_source_id, ReviewActor::Human, &human_label)
            .await
            .unwrap();
    }
    assert_eq!(
        queued(&pool).await,
        vec![(backfill_source, true)],
        "a human label holds a new source out of the queue"
    );

    let shop_closed = decision(
        Qualification::Disqualified,
        Some(SourceLabel::ShopClosed),
        "/ shows a domain-for-sale page",
    );
    let refused = reviews
        .decide_qualification(&new_source, ReviewActor::AgentAuthor, &shop_closed)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, ReviewRepositoryError::HumanLabelStands(_)),
        "{refused}"
    );
    let refused = reviews
        .decide_qualification(&backfill_source, ReviewActor::AgentReviewer, &shop_closed)
        .await
        .unwrap_err();
    assert!(
        matches!(
            refused,
            ReviewRepositoryError::ReviewerMayNotChangeCandidates
        ),
        "{refused}"
    );

    let qualified = decision(
        Qualification::Qualified,
        None,
        "/ sells antique clocks with prices and a cart",
    );
    let kept = reviews
        .decide_qualification(&backfill_source, ReviewActor::AgentAuthor, &qualified)
        .await
        .unwrap();
    assert_eq!(kept.label_effect, QualificationLabelEffect::KeptLabel);
    assert_eq!(kept.label, Some(SourceLabel::Other));
    assert_eq!(
        kept.previous_label.map(|label| label.labelled_by),
        Some(ReviewActor::Human)
    );
    let refused = reviews
        .decide_qualification(&backfill_source, ReviewActor::AgentAuthor, &shop_closed)
        .await
        .unwrap_err();
    assert!(
        matches!(refused, ReviewRepositoryError::SourceAlreadyQualified(_)),
        "{refused}"
    );

    let cleared = reviews
        .decide_qualification(&backfill_source, ReviewActor::Human, &qualified)
        .await
        .unwrap();
    assert_eq!(cleared.label_effect, QualificationLabelEffect::ClearedLabel);
    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(crawl_enabled(&pool, backfill_source).await);

    let replacing_backfill = ListingSourceId::new();
    sync(
        &pool,
        QualificationGate::Required,
        vec![registered(replacing_backfill, "https://gallery.example/")],
    )
    .await;
    make_backfill(&pool, replacing_backfill).await;
    reviews
        .label_source(&replacing_backfill, ReviewActor::Human, &human_label)
        .await
        .unwrap();
    let replaced = reviews
        .decide_qualification(
            &replacing_backfill,
            ReviewActor::AgentAuthor,
            &decision(
                Qualification::Disqualified,
                Some(SourceLabel::NotAnEshop),
                "/werke shows works with enquiry forms only",
            ),
        )
        .await
        .unwrap();
    assert_eq!(replaced.label_effect, QualificationLabelEffect::Labelled);
    assert_eq!(replaced.label, Some(SourceLabel::NotAnEshop));
}

/// A human who clears a label qualifies the source, so it is crawled again without waiting for a
/// qualification; a cleared qualification puts the source back in the queue.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_qualify_a_source_whose_label_a_human_clears() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    let scope = || vec![registered(listing_source_id, "https://shop.example/")];
    sync(&pool, QualificationGate::Required, scope()).await;
    reviews
        .label_source(
            &listing_source_id,
            ReviewActor::Human,
            &SourceLabelRequest::new(SourceLabel::BotProtection, None).unwrap(),
        )
        .await
        .unwrap();

    reviews
        .clear_source_label(&listing_source_id, ReviewActor::Human)
        .await
        .unwrap();
    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(crawl_enabled(&pool, listing_source_id).await);
    assert!(queued(&pool).await.is_empty());

    reviews
        .clear_qualification(&listing_source_id, ReviewActor::Human)
        .await
        .unwrap();
    assert_eq!(queued(&pool).await, vec![(listing_source_id, false)]);
    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(!crawl_enabled(&pool, listing_source_id).await);

    let history: Vec<(Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT qualification, note, actor FROM listing_source_qualification_events \
         WHERE listing_source_id = $1 ORDER BY occurred_at",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        history,
        vec![
            (
                Some("QUALIFIED".to_owned()),
                Some("Qualified by clearing the label BOT_PROTECTION.".to_owned()),
                "HUMAN".to_owned()
            ),
            (None, None, "HUMAN".to_owned()),
        ]
    );
}

/// The backfill ends with a source's first decision: once a human clears that qualification, the
/// source waits uncrawled like a new one, and an agent no longer replaces a label a human set.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_end_the_backfill_of_a_source_with_its_first_decision() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let unlabelled = ListingSourceId::new();
    let human_labelled = ListingSourceId::new();
    let scope = || {
        vec![
            registered(unlabelled, "https://clocks.example/"),
            registered(human_labelled, "https://odd.example/"),
        ]
    };
    sync(&pool, QualificationGate::Off, scope()).await;
    make_backfill(&pool, unlabelled).await;
    make_backfill(&pool, human_labelled).await;
    reviews
        .label_source(
            &human_labelled,
            ReviewActor::Human,
            &SourceLabelRequest::new(SourceLabel::Other, Some(note("odd shop"))).unwrap(),
        )
        .await
        .unwrap();
    let qualified = decision(
        Qualification::Qualified,
        None,
        "/ sells antique clocks with prices and a cart",
    );
    for listing_source_id in [unlabelled, human_labelled] {
        reviews
            .decide_qualification(&listing_source_id, ReviewActor::AgentAuthor, &qualified)
            .await
            .unwrap();
        reviews
            .clear_qualification(&listing_source_id, ReviewActor::Human)
            .await
            .unwrap();
    }

    sync(&pool, QualificationGate::Required, scope()).await;
    assert!(
        !crawl_enabled(&pool, unlabelled).await,
        "a cleared source waits uncrawled"
    );
    assert_eq!(queued(&pool).await, vec![(unlabelled, false)]);
    let refused = reviews
        .decide_qualification(
            &human_labelled,
            ReviewActor::AgentAuthor,
            &decision(
                Qualification::Disqualified,
                Some(SourceLabel::ShopClosed),
                "/ shows a domain-for-sale page",
            ),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refused, ReviewRepositoryError::HumanLabelStands(_)),
        "{refused}"
    );
}

/// Moves the time the source's current qualification was decided back by `hours`, as if it had
/// stood that long.
async fn age_qualification(pool: &PgPool, listing_source_id: ListingSourceId, hours: i32) {
    sqlx::query(
        "UPDATE listing_sources SET qualified_at = qualified_at - make_interval(hours => $2) \
         WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .bind(hours)
    .execute(pool)
    .await
    .unwrap();
}

/// A review agent's `NEEDS_HUMAN` stands for a day, after which the agents try the source again
/// with the earlier note as evidence, up to three `NEEDS_HUMAN` in all since a human last decided
/// it; after the third the source waits for a human for good, and a human's decision starts the
/// count afresh.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_retry_an_agent_s_needs_human_a_day_later_up_to_three_times() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let listing_source_id = ListingSourceId::new();
    sync(
        &pool,
        QualificationGate::Required,
        vec![registered(listing_source_id, "https://flaky.example/")],
    )
    .await;
    let unreachable = decision(Qualification::NeedsHuman, None, "/ timed out twice");
    let queue = || async {
        QualificationQueueReader::new(pool.clone())
            .read()
            .await
            .unwrap()
            .sources
    };

    for round in 1..=3 {
        reviews
            .decide_qualification(&listing_source_id, ReviewActor::AgentAuthor, &unreachable)
            .await
            .unwrap_or_else(|error| panic!("round {round}: {error}"));
        assert!(queue().await.is_empty(), "round {round}: waits a day");
        let refused = reviews
            .decide_qualification(&listing_source_id, ReviewActor::AgentAuthor, &unreachable)
            .await
            .unwrap_err();
        assert!(
            matches!(refused, ReviewRepositoryError::SourceAlreadyQualified(_)),
            "round {round}: {refused}"
        );
        age_qualification(&pool, listing_source_id, 25).await;
        let due = queue().await;
        if round < 3 {
            assert_eq!(due.len(), 1, "round {round}: tried again");
            assert_eq!(due[0].earlier_note.as_deref(), Some("/ timed out twice"));
        } else {
            assert!(due.is_empty(), "the third NEEDS_HUMAN waits for a human");
        }
    }

    reviews
        .clear_qualification(&listing_source_id, ReviewActor::Human)
        .await
        .unwrap();
    let fresh = queue().await;
    assert_eq!(fresh.len(), 1, "a human's clear starts the count afresh");
    assert_eq!(fresh[0].earlier_note, None);
}

/// The agents' qualifications count against their hourly decision cap with their approvals and
/// rejections.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_count_the_agents_qualifications_against_their_hourly_cap() {
    let pool = get_postgres_client().await;
    let reviews =
        CrawlerReviewRepository::new(pool.clone()).with_agent_decisions(AgentDecisionPolicy::new(
            CrawlerAuthoringMode::Agent,
            AgentLimits {
                max_decisions_per_hour: std::num::NonZeroU32::new(2).unwrap(),
                ..AgentLimits::default()
            },
        ));
    let sources = [
        ListingSourceId::new(),
        ListingSourceId::new(),
        ListingSourceId::new(),
    ];
    sync(
        &pool,
        QualificationGate::Required,
        sources
            .iter()
            .enumerate()
            .map(|(index, id)| registered(*id, &format!("https://shop{index}.example/")))
            .collect(),
    )
    .await;
    let qualified = decision(
        Qualification::Qualified,
        None,
        "/ sells antiques with a cart",
    );

    for listing_source_id in &sources[..2] {
        reviews
            .decide_qualification(listing_source_id, ReviewActor::AgentAuthor, &qualified)
            .await
            .unwrap();
    }
    let refused = reviews
        .decide_qualification(&sources[2], ReviewActor::AgentAuthor, &qualified)
        .await
        .unwrap_err();

    assert!(
        matches!(
            refused,
            ReviewRepositoryError::AgentDecisionLimitReached { limit: 2, .. }
        ),
        "{refused}"
    );
    reviews
        .decide_qualification(&sources[2], ReviewActor::Human, &qualified)
        .await
        .expect("a human is not capped");
}

/// A crawl root that moves to another site clears the source's qualification and its backfill
/// pass, so the source waits for a new qualification; the history records the clear as the
/// crawler's.
#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_requalify_a_source_whose_crawl_root_moves() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let qualified = ListingSourceId::new();
    let backfill = ListingSourceId::new();
    sync(
        &pool,
        QualificationGate::Required,
        vec![
            registered(qualified, "https://old-shop.example/"),
            registered(backfill, "https://old-backfill.example/"),
        ],
    )
    .await;
    make_backfill(&pool, backfill).await;
    reviews
        .decide_qualification(
            &qualified,
            ReviewActor::AgentAuthor,
            &decision(
                Qualification::Qualified,
                None,
                "/ sells antiques with a cart",
            ),
        )
        .await
        .unwrap();
    let moved = || {
        vec![
            registered(qualified, "https://new-shop.example/"),
            registered(backfill, "https://new-backfill.example/"),
        ]
    };

    sync(&pool, QualificationGate::Required, moved()).await;

    for listing_source_id in [qualified, backfill] {
        assert!(!crawl_enabled(&pool, listing_source_id).await);
    }
    let mut waiting = queued(&pool).await;
    waiting.sort();
    let mut expected = vec![(qualified, false), (backfill, false)];
    expected.sort();
    assert_eq!(waiting, expected);
    let cleared: Vec<(Option<String>, String)> = sqlx::query_as(
        "SELECT qualification, actor FROM listing_source_qualification_events \
         WHERE listing_source_id = $1 ORDER BY occurred_at",
    )
    .bind(qualified.as_uuid())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        cleared,
        vec![
            (Some("QUALIFIED".to_owned()), "AGENT_AUTHOR".to_owned()),
            (None, "SYSTEM".to_owned()),
        ]
    );
}
