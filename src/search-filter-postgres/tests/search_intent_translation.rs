use application::transaction::{Transaction, UnitOfWork};
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::{
    EnglishSearchIntent, MatchingEvidencePolicy, NewSearchFilter, SearchFilter,
    SearchIntentTranslation, search_filter_state::SearchFilterState,
    user_search_filter_id::UserSearchFilterId,
};
use search_filter_postgres::{
    SqlxPeriodicSearchFilterCandidateReader, SqlxPeriodicSearchFilterProgressFactory,
    SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory, SqlxSearchIntentTranslationStore,
};
use search_filter_service::ports::{
    PeriodicSearchFilterCandidate, PeriodicSearchFilterCandidatePageRequest,
    PeriodicSearchFilterCandidateReader, PeriodicSearchFilterProgress,
    PeriodicSearchFilterProgressFactory, SearchFilterReader, SearchFilterRepository,
    SearchFilterRepositoryFactory, SearchIntentTranslationRecordOutcome,
    SearchIntentTranslationStore, UntranslatedSearchIntentPageRequest,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::user_id::UserId;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn german_search(query: &str) -> ProductListingSearch {
    ProductListingSearch::new(Language::En, Currency::Eur)
        .with_product_listing_query(query.try_into().unwrap())
}

fn translation_of(search: &ProductListingSearch, english: &str) -> SearchIntentTranslation {
    SearchIntentTranslation::english(
        search,
        EnglishSearchIntent::new(vec![english.try_into().unwrap()], None),
    )
    .unwrap()
}

fn filter(
    user_id: UserId,
    search: ProductListingSearch,
    intent_translation: Option<SearchIntentTranslation>,
) -> SearchFilter {
    SearchFilter::create(NewSearchFilter {
        user_search_filter_id: UserSearchFilterId::new(),
        user_id,
        name: "translated".into(),
        notifications: true,
        state: SearchFilterState::Active,
        search,
        embedding: None,
        matching_evidence_policy: MatchingEvidencePolicy::TextOnly,
        intent_translation,
    })
}

async fn insert(unit: &SqlxUnitOfWork, filter: &SearchFilter) {
    let mut tx = unit.begin().await.unwrap();
    SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .insert(filter)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn seed_user(pool: &sqlx::PgPool, email: &str) -> UserId {
    let id = UserId::new();
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'ULTIMATE', 'USER')",
    )
    .bind(id.into_uuid())
    .bind(email)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn candidates(pool: &sqlx::PgPool) -> Vec<PeriodicSearchFilterCandidate> {
    SqlxPeriodicSearchFilterCandidateReader::new(pool.clone())
        .find_active_page(PeriodicSearchFilterCandidatePageRequest {
            after: None,
            page_size: 10,
            eligible_at_or_before: OffsetDateTime::now_utc() + Duration::hours(1),
        })
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_replace_the_translation_with_the_text_and_keep_it_otherwise() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "intent-translation-repository@example.test").await;
    let search = german_search("Manschettenknöpfe");
    let mut filter = filter(
        user_id,
        search.clone(),
        Some(translation_of(&search, "cufflinks")),
    );
    insert(&unit, &filter).await;

    let mut tx = unit.begin().await.unwrap();
    let persisted = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .find_by_id(filter.id())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(filter, persisted.filter);
    let mut other_criteria = search.clone();
    other_criteria.currency = Currency::Usd;
    filter.replace_search(other_criteria, None);
    let persisted = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .update(&filter, persisted.version)
        .await
        .unwrap();
    assert_eq!(
        Some(&translation_of(&search, "cufflinks")),
        persisted.filter.intent_translation()
    );
    let edited = german_search("Broschen");
    filter.replace_search(edited.clone(), None);
    filter
        .record_intent_translation(translation_of(&edited, "brooches"))
        .unwrap();
    let persisted = SqlxSearchFilterRepositoryFactory
        .in_transaction(&mut tx)
        .update(&filter, persisted.version)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        Some(&translation_of(&edited, "brooches")),
        persisted.filter.intent_translation()
    );
    // Presentation reads echo exactly what the user typed.
    let views = SqlxSearchFilterReader::new(pool)
        .find_for_user(user_id)
        .await
        .unwrap();
    assert_eq!(edited, views[0].search);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_fail_only_the_filter_whose_translation_is_invalid() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "intent-translation-invalid@example.test").await;
    let search = german_search("Vase");
    let valid = filter(
        user_id,
        search.clone(),
        Some(translation_of(&search, "vase")),
    );
    let corrupt = filter(user_id, search.clone(), None);
    insert(&unit, &valid).await;
    insert(&unit, &corrupt).await;
    sqlx::query("UPDATE search_filters SET intent_translation = '{\"english\": null}'::jsonb WHERE user_search_filter_id = $1")
        .bind(corrupt.id().into_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let candidates = candidates(&pool).await;

    assert_eq!(2, candidates.len());
    for candidate in candidates {
        if candidate.search_filter_id == valid.id() {
            assert_eq!(
                Ok(Some(translation_of(&search, "vase"))),
                candidate.intent_translation
            );
        } else {
            assert!(candidate.intent_translation.is_err());
        }
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_backfill_untranslated_semantic_filters_behind_a_version_guard() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "intent-translation-backfill@example.test").await;
    let search = german_search("Leuchter");
    let untranslated = filter(user_id, search.clone(), None);
    let translated = filter(
        user_id,
        search.clone(),
        Some(SearchIntentTranslation::already_english(&search)),
    );
    let structured_only = filter(
        user_id,
        ProductListingSearch::new(Language::De, Currency::Eur),
        None,
    );
    let mut inactive = filter(user_id, search.clone(), None);
    inactive.change_state(SearchFilterState::InactiveByUser);
    for filter in [&untranslated, &translated, &structured_only, &inactive] {
        insert(&unit, filter).await;
    }
    let store = SqlxSearchIntentTranslationStore::new(pool.clone());

    let page = store
        .find_untranslated_page(UntranslatedSearchIntentPageRequest {
            after: None,
            page_size: 10,
        })
        .await
        .unwrap();
    assert_eq!(
        vec![untranslated.id()],
        page.iter()
            .map(|item| item.search_filter_id)
            .collect::<Vec<_>>()
    );
    let (version, updated): (i64, OffsetDateTime) = sqlx::query_as(
        "SELECT version, updated FROM search_filters WHERE user_search_filter_id = $1",
    )
    .bind(untranslated.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();

    let translation = translation_of(&search, "candlestick");
    assert_eq!(
        SearchIntentTranslationRecordOutcome::FilterChanged,
        store
            .record(untranslated.id(), version + 1, &translation)
            .await
            .unwrap()
    );
    assert_eq!(
        SearchIntentTranslationRecordOutcome::Recorded,
        store
            .record(untranslated.id(), version, &translation)
            .await
            .unwrap()
    );

    let after: (i64, OffsetDateTime) = sqlx::query_as(
        "SELECT version, updated FROM search_filters WHERE user_search_filter_id = $1",
    )
    .bind(untranslated.id().into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((version, updated), after);
    assert!(
        store
            .find_untranslated_page(UntranslatedSearchIntentPageRequest {
                after: None,
                page_size: 10,
            })
            .await
            .unwrap()
            .is_empty()
    );
    // A run frozen afterwards carries the translation in its snapshot.
    let mut tx = unit.begin().await.unwrap();
    SqlxPeriodicSearchFilterProgressFactory
        .in_transaction(&mut tx)
        .begin_run(OffsetDateTime::now_utc())
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let frozen = candidates(&pool).await;
    let frozen = frozen
        .iter()
        .find(|candidate| candidate.search_filter_id == untranslated.id())
        .unwrap();
    assert_eq!(Ok(Some(translation)), frozen.intent_translation);
}
