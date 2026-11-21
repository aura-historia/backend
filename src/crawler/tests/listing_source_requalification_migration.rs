//! The one-off migration of #3008 that puts what the review agents decided under the old e-shop
//! rule back in the qualification queue: an agent's NOT_AN_ESHOP or OTHER label and its
//! qualification, and an agent's NEEDS_HUMAN, are cleared into the histories as SYSTEM, while what
//! a human decided and the agents' other labels stay.

use async_trait::async_trait;
use crawler::review::model::{
    Qualification, QualificationDecision, ReviewActor, SourceLabel, SourceLabelNote,
    SourceLabelRequest,
};
use crawler::review::readers::QualificationQueueReader;
use crawler::review::repository::CrawlerReviewRepository;
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepositoryImpl, ListingSourceRegistrationService,
    ListingSourceRegistrationSource, ListingSourceSyncError, QualificationGate,
    RegisteredListingSource,
};
use listing_source_core::{ListingSourceId, ListingSourceName, ListingSourceSlugId};
use sqlx::PgPool;
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");
const MIGRATION: &str =
    include_str!("../migrations/20261117000000_requalify_under_enquiry_policy.sql");

struct FixedScope(Vec<RegisteredListingSource>);

#[async_trait]
impl ListingSourceRegistrationSource for FixedScope {
    async fn fetch_registered_listing_sources(
        &self,
    ) -> Result<Vec<RegisteredListingSource>, ListingSourceSyncError> {
        Ok(self.0.clone())
    }
}

async fn sync(pool: &PgPool, scope: &[ListingSourceId]) {
    let registered = scope
        .iter()
        .enumerate()
        .map(|(index, listing_source_id)| RegisteredListingSource {
            listing_source_id: *listing_source_id,
            listing_source_name: ListingSourceName::try_from("Shop").unwrap(),
            listing_source_slug: ListingSourceSlugId::raw("shop").unwrap(),
            crawl_enabled: true,
            presentation_url: Some(format!("https://shop-{index}.example/")),
        })
        .collect();
    ListingSourceRegistrationService::new(
        Box::new(FixedScope(registered)),
        Box::new(ListingSourceRegistrationRepositoryImpl::new(
            pool.clone(),
            QualificationGate::Required,
        )),
    )
    .sync()
    .await
    .unwrap_or_else(|error| panic!("sync the scope: {error}"));
}

fn note(text: &str) -> SourceLabelNote {
    SourceLabelNote::try_from(text.to_owned()).unwrap()
}

async fn decide(
    reviews: &CrawlerReviewRepository,
    listing_source_id: ListingSourceId,
    actor: ReviewActor,
    qualification: Qualification,
    label: Option<SourceLabel>,
) {
    reviews
        .decide_qualification(
            &listing_source_id,
            actor,
            &QualificationDecision::new(qualification, label, note("read / and /item/1")).unwrap(),
        )
        .await
        .unwrap();
}

/// The source's qualification and its actor, its label and its actor, and whether it is crawled.
type SourceState = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
);

async fn state(pool: &PgPool, listing_source_id: ListingSourceId) -> SourceState {
    sqlx::query_as(
        "SELECT qualification, qualified_by, source_label, source_labelled_by, crawl_enabled \
         FROM listing_sources WHERE listing_source_id = $1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

fn owned(
    qualification: Option<&str>,
    qualified_by: Option<&str>,
    label: Option<&str>,
    labelled_by: Option<&str>,
    crawl_enabled: bool,
) -> SourceState {
    (
        qualification.map(str::to_owned),
        qualified_by.map(str::to_owned),
        label.map(str::to_owned),
        labelled_by.map(str::to_owned),
        crawl_enabled,
    )
}

/// The latest label and qualification history entries of a source: their kind or decision and
/// their actor.
type LatestEvents = (Option<(String, String)>, Option<(Option<String>, String)>);

async fn latest_events(pool: &PgPool, listing_source_id: ListingSourceId) -> LatestEvents {
    let label = sqlx::query_as(
        "SELECT event_kind, actor FROM listing_source_label_events \
         WHERE listing_source_id = $1 ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_optional(pool)
    .await
    .unwrap();
    let qualification = sqlx::query_as(
        "SELECT qualification, actor FROM listing_source_qualification_events \
         WHERE listing_source_id = $1 ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(listing_source_id.as_uuid())
    .fetch_optional(pool)
    .await
    .unwrap();
    (label, qualification)
}

fn events(
    label: Option<(&str, &str)>,
    qualification: Option<(Option<&str>, &str)>,
) -> LatestEvents {
    (
        label.map(|(kind, actor)| (kind.to_owned(), actor.to_owned())),
        qualification.map(|(decision, actor)| (decision.map(str::to_owned), actor.to_owned())),
    )
}

#[aura_integration_test(services = [POSTGRES], isolated = true)]
async fn should_requeue_the_agents_old_rule_decisions_and_keep_the_humans() {
    let pool = get_postgres_client().await;
    let reviews = CrawlerReviewRepository::new(pool.clone());
    let enquiry_dealer = ListingSourceId::new();
    let auction_house = ListingSourceId::new();
    let unreachable = ListingSourceId::new();
    let human_brochure = ListingSourceId::new();
    let new_furniture = ListingSourceId::new();
    let human_qualified = ListingSourceId::new();
    let scope = [
        enquiry_dealer,
        auction_house,
        unreachable,
        human_brochure,
        new_furniture,
        human_qualified,
    ];
    sync(&pool, &scope).await;
    use Qualification::{Disqualified, NeedsHuman, Qualified};
    use ReviewActor::{AgentAuthor, Human};
    #[rustfmt::skip]
    let decisions = [
        (enquiry_dealer, AgentAuthor, Disqualified, Some(SourceLabel::NotAnEshop)),
        (auction_house, AgentAuthor, Qualified, Some(SourceLabel::Other)),
        (unreachable, AgentAuthor, NeedsHuman, None),
        (human_brochure, Human, Disqualified, Some(SourceLabel::NotAnEshop)),
        (new_furniture, AgentAuthor, Disqualified, Some(SourceLabel::NotAnAntiqueShop)),
        (human_qualified, Human, Qualified, None),
    ];
    for (listing_source_id, actor, qualification, label) in decisions {
        decide(&reviews, listing_source_id, actor, qualification, label).await;
    }
    reviews
        .label_source(
            &human_qualified,
            AgentAuthor,
            &SourceLabelRequest::new(SourceLabel::NotAnEshop, Some(note("enquiry form only")))
                .unwrap(),
        )
        .await
        .unwrap();

    // The migration ran before qualifications stored a retry time (#2990), so it is replayed on
    // rows without one, as it found them.
    sqlx::query("UPDATE listing_sources SET qualification_retry_at = NULL")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(MIGRATION).execute(&pool).await.unwrap();

    let mut queued: Vec<ListingSourceId> = QualificationQueueReader::new(pool.clone())
        .read()
        .await
        .unwrap()
        .sources
        .into_iter()
        .map(|source| source.listing_source_id)
        .collect();
    queued.sort();
    let mut requeued = vec![enquiry_dealer, auction_house, unreachable];
    requeued.sort();
    assert_eq!(requeued, queued);
    for requeued in [enquiry_dealer, auction_house] {
        assert_eq!(
            owned(None, None, None, None, false),
            state(&pool, requeued).await
        );
        assert_eq!(
            events(Some(("CLEARED", "SYSTEM")), Some((None, "SYSTEM"))),
            latest_events(&pool, requeued).await
        );
    }
    assert_eq!(
        owned(None, None, None, None, false),
        state(&pool, unreachable).await
    );
    assert_eq!(
        events(None, Some((None, "SYSTEM"))),
        latest_events(&pool, unreachable).await
    );
    assert_eq!(
        owned(
            Some("DISQUALIFIED"),
            Some("HUMAN"),
            Some("NOT_AN_ESHOP"),
            Some("HUMAN"),
            false
        ),
        state(&pool, human_brochure).await
    );
    assert_eq!(
        owned(
            Some("DISQUALIFIED"),
            Some("AGENT_AUTHOR"),
            Some("NOT_AN_ANTIQUE_SHOP"),
            Some("AGENT_AUTHOR"),
            false
        ),
        state(&pool, new_furniture).await
    );
    assert_eq!(
        owned(Some("QUALIFIED"), Some("HUMAN"), None, None, false),
        state(&pool, human_qualified).await
    );
    assert_eq!(
        events(
            Some(("CLEARED", "SYSTEM")),
            Some((Some("QUALIFIED"), "HUMAN"))
        ),
        latest_events(&pool, human_qualified).await
    );

    sync(&pool, &scope).await;
    assert!(
        state(&pool, human_qualified).await.4,
        "a human's qualification is crawled again without the agent's label"
    );
}
