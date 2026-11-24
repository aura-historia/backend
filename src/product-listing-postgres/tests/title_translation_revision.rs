//! Title translations across content revisions: a description-only change carries the current
//! translations and pending work onto the new revision; a title change discards them.

use application::transaction::{Transaction, UnitOfWork};
use domain_primitives::event_id::EventId;
use indexmap::{IndexMap, IndexSet};
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use money::{Currency, MonetaryAmount, Price};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    description::Description,
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::ProductListingId,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
    SqlxProductListingTitleTranslationQueueFactory, SqlxProductListingTranslationWriterFactory,
};
use product_listing_service::ports::{
    ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
    ProductListingRepositoryFactory, ProductListingTitleTranslationQueue,
    ProductListingTitleTranslationQueueFactory, ProductListingTranslationWrite,
    ProductListingTranslationWriteOutcome, ProductListingTranslationWriter,
    ProductListingTranslationWriterFactory, ProductListingWriteEffects,
    TitleTranslationScheduleOutcome, TitleTranslationWorkRef, stamp_product_listing_event,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
type TestError = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), TestError>;

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_carry_current_translations_and_their_completion_when_only_the_description_changes()
{
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let (listing, discovered) = insert_listing(&pool, &unit_of_work).await?;
        assert_eq!(
            ProductListingTranslationWriteOutcome::Applied,
            translate(&unit_of_work, listing, discovered).await?
        );

        let changed = change(&unit_of_work, listing, |product| {
            product
                .replace_description(Some(text(Language::De, "Neue Beschreibung")))
                .map(|_| ())
        })
        .await?;

        assert_eq!(
            vec![
                (
                    "en".to_owned(),
                    "Antique chair".to_owned(),
                    changed.into_uuid()
                ),
                (
                    "fr".to_owned(),
                    "Chaise ancienne".to_owned(),
                    changed.into_uuid()
                ),
            ],
            translations(&pool, listing).await?
        );
        let (completion, payload) = latest_completion(&pool, listing).await?;
        assert_eq!(
            serde_json::json!({
                "sourceEventId": changed.as_uuid().to_string(),
                "sourceLanguage": "de",
                "targetLanguages": ["en", "fr"],
            }),
            payload
        );
        let (current_event, content_source) = revisions(&pool, listing).await?;
        assert_eq!(completion, current_event);
        assert_eq!(changed.into_uuid(), content_source);
        assert_eq!(
            TitleTranslationScheduleOutcome::AlreadyTranslated,
            schedule(&unit_of_work, listing, changed).await?
        );
        assert_eq!(
            ProductListingTranslationWriteOutcome::Duplicate,
            translate(&unit_of_work, listing, changed).await?
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "description-only carry failed: {result:?}");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_translate_again_when_a_description_change_follows_an_untranslated_title_change() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let (listing, discovered) = insert_listing(&pool, &unit_of_work).await?;
        translate(&unit_of_work, listing, discovered).await?;

        let retitled = change(&unit_of_work, listing, |product| {
            product
                .replace_title(Some(text(Language::De, "Antiker Sessel")))
                .map(|_| ())
        })
        .await?;
        assert!(translations(&pool, listing).await?.is_empty());
        assert_eq!(
            TitleTranslationScheduleOutcome::Scheduled,
            schedule(&unit_of_work, listing, retitled).await?
        );
        sqlx::query(
            "UPDATE product_listing_title_translation_work SET attempts = 3, parked = now() WHERE product_listing_id = $1",
        )
        .bind(listing.as_uuid())
        .execute(&pool)
        .await?;
        let completions_before = completion_count(&pool, listing).await?;

        let described = change(&unit_of_work, listing, |product| {
            product
                .replace_description(Some(text(Language::De, "Neue Beschreibung")))
                .map(|_| ())
        })
        .await?;

        assert!(translations(&pool, listing).await?.is_empty());
        assert_eq!(completions_before, completion_count(&pool, listing).await?);
        let (current_event, _) = revisions(&pool, listing).await?;
        assert_eq!(described.into_uuid(), current_event);
        let work = sqlx::query_as::<_, (uuid::Uuid, i16, bool)>(
            "SELECT source_event_id, attempts, parked IS NOT NULL FROM product_listing_title_translation_work WHERE product_listing_id = $1",
        )
        .bind(listing.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert_eq!((described.into_uuid(), 3, true), work);
        assert_eq!(
            TitleTranslationScheduleOutcome::Scheduled,
            schedule(&unit_of_work, listing, described).await?
        );
        let parked = sqlx::query_scalar::<_, bool>(
            "SELECT parked IS NOT NULL FROM product_listing_title_translation_work WHERE product_listing_id = $1",
        )
        .bind(listing.as_uuid())
        .fetch_one(&pool)
        .await?;
        assert!(parked, "the carried work must keep its parking");
        assert_eq!(
            ProductListingTranslationWriteOutcome::Applied,
            translate(&unit_of_work, listing, described).await?
        );
        Ok(())
    }
    .await;
    assert!(
        result.is_ok(),
        "untranslated title carry failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_discard_translations_and_translate_again_when_the_title_changes() {
    let result: TestResult = async {
        let pool = get_postgres_client().await;
        let unit_of_work = SqlxUnitOfWork::new(pool.clone());
        let (listing, discovered) = insert_listing(&pool, &unit_of_work).await?;
        translate(&unit_of_work, listing, discovered).await?;
        let completions_before = completion_count(&pool, listing).await?;

        let retitled = change(&unit_of_work, listing, |product| {
            product.replace_title(Some(text(Language::En, "Antiker Stuhl")))?;
            product
                .replace_description(Some(text(Language::De, "Neue Beschreibung")))
                .map(|_| ())
        })
        .await?;

        assert!(translations(&pool, listing).await?.is_empty());
        assert_eq!(completions_before, completion_count(&pool, listing).await?);
        assert_eq!(
            TitleTranslationScheduleOutcome::Scheduled,
            schedule(&unit_of_work, listing, retitled).await?
        );
        Ok(())
    }
    .await;
    assert!(result.is_ok(), "title change discard failed: {result:?}");
}

fn text<T: From<&'static str>>(language: Language, value: &'static str) -> Localized<Language, T> {
    Localized::new(language, T::from(value))
}

async fn insert_listing(
    pool: &sqlx::PgPool,
    unit_of_work: &SqlxUnitOfWork,
) -> Result<(ProductListingId, EventId), TestError> {
    let listing_source_id = seed_listing_source(pool).await?;
    let id = ProductListingId::new();
    let mut product = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::from_title_and_suffix("antiker-stuhl", "a1b2c3")?,
        listing_source_id,
        source_listing_id: SourceListingId::try_from("antiker-stuhl")?,
        title: Some(text(Language::De, "Antiker Stuhl")),
        description: Some(text::<Description>(Language::De, "Eiche, um 1900")),
        pricing: ProductListingPricing {
            price: Some(Price::new(MonetaryAmount::from(1_200_u64), Currency::Eur).into()),
            price_estimate_min: None,
            price_estimate_max: None,
        },
        availability: None,
        url: "https://example.com/antiker-stuhl".parse()?,
        images: IndexSet::new(),
        auction: None,
    })?;
    let payload = product
        .take_pending_event_payload()
        .ok_or("created listing has no discovery")?;
    let event = stamp_product_listing_event(id, OffsetDateTime::now_utc(), payload);
    let mut tx = unit_of_work.begin().await?;
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&product, event.event_id)
        .await?;
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut tx)
        .append(&event)
        .await?;
    tx.commit().await?;
    Ok((id, event.event_id))
}

/// Applies `mutate` as one listing change through the repository and returns its event.
async fn change(
    unit_of_work: &SqlxUnitOfWork,
    listing: ProductListingId,
    mutate: impl FnOnce(
        &mut ProductListing,
    )
        -> Result<(), product_listing_core::product_listing::ChangeProductListingError>,
) -> Result<EventId, TestError> {
    let products = SqlxProductListingRepositoryFactory::new();
    let mut tx = unit_of_work.begin().await?;
    let stored = products
        .in_transaction(&mut tx)
        .find_by_id(listing)
        .await?
        .ok_or("listing is missing")?;
    let mut product = stored.value;
    mutate(&mut product)?;
    let payload = product
        .take_pending_event_payload()
        .ok_or("change produced no event")?;
    let event = stamp_product_listing_event(listing, OffsetDateTime::now_utc(), payload);
    products
        .in_transaction(&mut tx)
        .update(
            &product,
            stored.version,
            event.event_id,
            ProductListingWriteEffects::from(&event.payload),
        )
        .await?;
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut tx)
        .append(&event)
        .await?;
    tx.commit().await?;
    Ok(event.event_id)
}

async fn translate(
    unit_of_work: &SqlxUnitOfWork,
    listing: ProductListingId,
    source_event_id: EventId,
) -> Result<ProductListingTranslationWriteOutcome, TestError> {
    let write = ProductListingTranslationWrite {
        product_listing_id: listing,
        source_event_id,
        enrichment_event_id: EventId::new(),
        source_language: Language::De,
        titles: IndexMap::from([
            (Language::En, Title::from("Antique chair")),
            (Language::Fr, Title::from("Chaise ancienne")),
        ]),
    };
    let mut tx = unit_of_work.begin().await?;
    let outcome = SqlxProductListingTranslationWriterFactory::new()
        .in_transaction(&mut tx)
        .apply(&write)
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

async fn schedule(
    unit_of_work: &SqlxUnitOfWork,
    listing: ProductListingId,
    source_event_id: EventId,
) -> Result<TitleTranslationScheduleOutcome, TestError> {
    let mut tx = unit_of_work.begin().await?;
    let outcome = SqlxProductListingTitleTranslationQueueFactory::new()
        .in_transaction(&mut tx)
        .schedule(TitleTranslationWorkRef {
            product_listing_id: listing,
            source_event_id,
        })
        .await?;
    tx.commit().await?;
    Ok(outcome)
}

async fn translations(
    pool: &sqlx::PgPool,
    listing: ProductListingId,
) -> Result<Vec<(String, String, uuid::Uuid)>, TestError> {
    Ok(sqlx::query_as(
        "SELECT language, title, source_event_id FROM product_listing_translations WHERE product_listing_id = $1 ORDER BY language",
    )
    .bind(listing.as_uuid())
    .fetch_all(pool)
    .await?)
}

async fn latest_completion(
    pool: &sqlx::PgPool,
    listing: ProductListingId,
) -> Result<(uuid::Uuid, serde_json::Value), TestError> {
    Ok(sqlx::query_as(
        "SELECT event_id, payload FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES' ORDER BY event_id DESC LIMIT 1",
    )
    .bind(listing.as_uuid())
    .fetch_one(pool)
    .await?)
}

async fn completion_count(
    pool: &sqlx::PgPool,
    listing: ProductListingId,
) -> Result<i64, TestError> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1 AND event_type = 'ENRICHMENT_TRANSLATED_TITLES'",
    )
    .bind(listing.as_uuid())
    .fetch_one(pool)
    .await?)
}

/// The listing's current event and content revision.
async fn revisions(
    pool: &sqlx::PgPool,
    listing: ProductListingId,
) -> Result<(uuid::Uuid, uuid::Uuid), TestError> {
    Ok(sqlx::query_as(
        "SELECT current_event_id, content_source_event_id FROM product_listings WHERE product_listing_id = $1",
    )
    .bind(listing.as_uuid())
    .fetch_one(pool)
    .await?)
}

async fn seed_listing_source(pool: &sqlx::PgPool) -> Result<ListingSourceId, TestError> {
    let party_id = uuid::Uuid::now_v7();
    let listing_source_id = ListingSourceId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, 'carry-party', 'Carry party')")
        .bind(party_id)
        .execute(pool)
        .await?;
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id, listing_source_type) VALUES ($1, 'carry-source', 'Carry source', $2, 'DEALER')")
        .bind(listing_source_id.into_uuid())
        .bind(party_id)
        .execute(pool)
        .await?;
    Ok(listing_source_id)
}
