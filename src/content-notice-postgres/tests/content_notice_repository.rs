use application::transaction::{Transaction, UnitOfWork};
use content_notice_core::{
    additional_url::{AdditionalUrl, AdditionalUrls},
    content_notice::{ContentNotice, NewContentNotice},
    content_notice_id::ContentNoticeId,
    content_notice_state::ContentNoticeState,
    content_notice_target::{
        ContentNoticeTarget, ListingSourceSnapshot, ProductListingSnapshot,
        ProductListingTitleSnapshot, TargetSnapshot,
    },
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_postgres::SqlxContentNoticeRepositoryFactory;
use content_notice_service::ports::{
    ContentNoticeRepository, ContentNoticeRepositoryError, ContentNoticeRepositoryFactory,
    ContentNoticeStorageVersion, ContentNoticeSubmissionFingerprint,
};
use listing_source_core::{ListingSourceId, ListingSourceName};
use localization::Language;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use url::Url;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const FINGERPRINT: ContentNoticeSubmissionFingerprint =
    ContentNoticeSubmissionFingerprint::from_sha256([7; 32]);

fn listing_source_snapshot() -> ListingSourceSnapshot {
    ListingSourceSnapshot {
        name: ListingSourceName::try_from("Antiquitäten Muster").unwrap(),
        url: Some(Url::parse("https://antik.example/").unwrap()),
        partner: true,
    }
}

fn submitter() -> Submitter {
    Submitter {
        name: SubmitterName::try_from("Erika Mustermann").unwrap(),
        email: SubmitterEmail::try_from("Erika@Antik.example").unwrap(),
        language: Language::De,
    }
}

fn provider_opt_out_for_listing_source() -> ContentNotice {
    ContentNotice::submit(NewContentNotice {
        id: ContentNoticeId::new(),
        target: ContentNoticeTarget::ListingSource {
            listing_source_id: ListingSourceId::new(),
        },
        target_snapshot: TargetSnapshot {
            listing_source: listing_source_snapshot(),
            product_listing: None,
        },
        submitter_role: SubmitterRole::Provider,
        reason_category: ReasonCategory::ProviderOptOut,
        explanation: None,
        additional_urls: AdditionalUrls::default(),
        submitter: submitter(),
        good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
    })
    .unwrap()
}

fn rights_infringement_for_product_listing() -> ContentNotice {
    ContentNotice::submit(NewContentNotice {
        id: ContentNoticeId::new(),
        target: ContentNoticeTarget::ProductListing {
            product_listing_id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
        },
        target_snapshot: TargetSnapshot {
            listing_source: ListingSourceSnapshot {
                url: None,
                partner: false,
                ..listing_source_snapshot()
            },
            product_listing: Some(ProductListingSnapshot {
                title: Some(ProductListingTitleSnapshot::try_from("Jugendstil-Vase").unwrap()),
                url: Url::parse("https://antik.example/vase?id=42").unwrap(),
            }),
        },
        submitter_role: SubmitterRole::RightsHolder,
        reason_category: ReasonCategory::RightsInfringement,
        explanation: Some(
            NoticeExplanation::try_from("Das Foto ist meins.\nKeine Lizenz erteilt.").unwrap(),
        ),
        additional_urls: AdditionalUrls::try_from(vec![
            AdditionalUrl::try_from("https://portfolio.example/vase").unwrap(),
            AdditionalUrl::try_from("http://archive.example/2019/vase").unwrap(),
        ])
        .unwrap(),
        submitter: submitter(),
        good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
    })
    .unwrap()
}

async fn begin(pool: &PgPool) -> SqlxTransaction {
    SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin content notice transaction: {error}"))
}

async fn insert_and_find(pool: &PgPool, notice: &ContentNotice) {
    let factory = SqlxContentNoticeRepositoryFactory::new();
    let mut transaction = begin(pool).await;
    let inserted = factory
        .in_transaction(&mut transaction)
        .insert(notice, FINGERPRINT, OffsetDateTime::now_utc())
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    let mut transaction = begin(pool).await;
    let found = factory
        .in_transaction(&mut transaction)
        .find_by_id(notice.id())
        .await
        .unwrap()
        .expect("inserted notice");

    assert_eq!(notice, &inserted.value);
    assert_eq!(ContentNoticeStorageVersion::INITIAL, inserted.version);
    assert_eq!(inserted, found);
    assert_eq!(ContentNoticeState::Submitted, found.value.state());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_insert_and_read_back_listing_source_notice_without_target_rows() {
    let pool = get_postgres_client().await;

    insert_and_find(&pool, &provider_opt_out_for_listing_source()).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_insert_and_read_back_product_listing_notice_with_its_listing_source() {
    let pool = get_postgres_client().await;
    let notice = rights_infringement_for_product_listing();

    insert_and_find(&pool, &notice).await;

    let (listing_source_id, email_key, fingerprint): (uuid::Uuid, String, Vec<u8>) =
        sqlx::query_as(
            "SELECT listing_source_id, submitter_email_key, submission_fingerprint FROM content_notices WHERE content_notice_id = $1",
        )
        .bind(notice.id().into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        notice.target().listing_source_id().into_uuid(),
        listing_source_id
    );
    assert_eq!("erika@antik.example", email_key);
    assert_eq!(FINGERPRINT.as_bytes().as_slice(), fingerprint.as_slice());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_return_none_for_unknown_notice() {
    let pool = get_postgres_client().await;
    let mut transaction = begin(&pool).await;

    let found = SqlxContentNoticeRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .find_by_id(ContentNoticeId::new())
        .await
        .unwrap();

    assert!(found.is_none());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_report_unknown_enum_value_as_invalid_persisted_state() {
    let pool = get_postgres_client().await;
    let notice = rights_infringement_for_product_listing();
    let factory = SqlxContentNoticeRepositoryFactory::new();
    let mut transaction = begin(&pool).await;
    factory
        .in_transaction(&mut transaction)
        .insert(&notice, FINGERPRINT, OffsetDateTime::now_utc())
        .await
        .unwrap();
    // Simulate a value written by a newer release that this release does not know.
    sqlx::query(
        "ALTER TABLE content_notices DROP CONSTRAINT content_notices_reason_category_check",
    )
    .execute(transaction.connection())
    .await
    .unwrap();
    sqlx::query("UPDATE content_notices SET reason_category = 'SPAM' WHERE content_notice_id = $1")
        .bind(notice.id().into_uuid())
        .execute(transaction.connection())
        .await
        .unwrap();

    let found = factory
        .in_transaction(&mut transaction)
        .find_by_id(notice.id())
        .await;

    assert!(matches!(
        found,
        Err(ContentNoticeRepositoryError::InvalidPersistedState { .. })
    ));
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_rows_whose_target_columns_disagree_with_target_type() {
    let pool = get_postgres_client().await;

    for (target_type, product_listing_id, product_listing_url) in [
        (
            "LISTING_SOURCE",
            Some(ProductListingId::new().into_uuid()),
            Some("https://antik.example/vase"),
        ),
        ("PRODUCT_LISTING", None, Some("https://antik.example/vase")),
        (
            "PRODUCT_LISTING",
            Some(ProductListingId::new().into_uuid()),
            None,
        ),
    ] {
        let error = sqlx::query(
            "INSERT INTO content_notices(content_notice_id, target_type, listing_source_id, product_listing_id, listing_source_name, listing_source_partner, product_listing_url, submitter_role, reason_category, submitter_name, submitter_email, submitter_language, good_faith_statement_version, submission_fingerprint, business_state) VALUES($1, $2, $3, $4, 'Antik', false, $5, 'PROVIDER', 'PROVIDER_OPT_OUT', 'Erika', 'erika@antik.example', 'de', 'GOOD_FAITH_V1', $6, 'SUBMITTED')",
        )
        .bind(ContentNoticeId::new().into_uuid())
        .bind(target_type)
        .bind(ListingSourceId::new().into_uuid())
        .bind(product_listing_id)
        .bind(product_listing_url)
        .bind(FINGERPRINT.as_bytes().as_slice())
        .execute(&pool)
        .await
        .expect_err("inconsistent target row");

        assert_eq!(
            Some("content_notices_target_check"),
            error
                .as_database_error()
                .and_then(|error| error.constraint()),
            "{target_type}"
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_store_values_at_the_upper_bounds_of_the_domain_types() {
    let pool = get_postgres_client().await;
    let url_prefix = "https://antik.example/";
    let mut explanation = "e".repeat(NoticeExplanation::MAX_CHARS - 3);
    explanation.insert_str(10, "\r\n\t");
    let notice = ContentNotice::submit(NewContentNotice {
        id: ContentNoticeId::new(),
        target: ContentNoticeTarget::ProductListing {
            product_listing_id: ProductListingId::new(),
            listing_source_id: ListingSourceId::new(),
        },
        target_snapshot: TargetSnapshot {
            listing_source: ListingSourceSnapshot {
                // 255 bytes, the most a listing source name may have.
                name: ListingSourceName::try_from(format!("{}a", "ä".repeat(127))).unwrap(),
                url: None,
                partner: false,
            },
            product_listing: Some(ProductListingSnapshot {
                title: Some(
                    ProductListingTitleSnapshot::try_from(
                        "ü".repeat(ProductListingTitleSnapshot::MAX_CHARS),
                    )
                    .unwrap(),
                ),
                url: Url::parse("https://antik.example/vase").unwrap(),
            }),
        },
        submitter_role: SubmitterRole::IllegalContentReporter,
        reason_category: ReasonCategory::Other,
        explanation: Some(NoticeExplanation::try_from(explanation).unwrap()),
        additional_urls: AdditionalUrls::try_from(
            (0..AdditionalUrls::MAX_ITEMS)
                .map(|index| {
                    let path = format!("{index}{}", "p".repeat(AdditionalUrl::MAX_CHARS));
                    AdditionalUrl::try_from(
                        format!("{url_prefix}{path}")[..AdditionalUrl::MAX_CHARS].to_owned(),
                    )
                    .unwrap()
                })
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        submitter: Submitter {
            name: SubmitterName::try_from("ß".repeat(SubmitterName::MAX_CHARS)).unwrap(),
            email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
            language: Language::Hi,
        },
        good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
    })
    .unwrap();

    insert_and_find(&pool, &notice).await;
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_stored_text_that_the_domain_types_would_reject() {
    let pool = get_postgres_client().await;
    let notice = rights_infringement_for_product_listing();
    let mut transaction = begin(&pool).await;
    SqlxContentNoticeRepositoryFactory::new()
        .in_transaction(&mut transaction)
        .insert(&notice, FINGERPRINT, OffsetDateTime::now_utc())
        .await
        .unwrap();
    transaction.commit().await.unwrap();

    for (column, value, constraint) in [
        (
            "submitter_name",
            "Erika\u{7}",
            "content_notices_submitter_name_check",
        ),
        (
            "submitter_name",
            " Erika",
            "content_notices_submitter_name_check",
        ),
        ("submitter_name", "", "content_notices_submitter_name_check"),
        (
            "explanation",
            "Bell\u{7}",
            "content_notices_explanation_check",
        ),
        ("explanation", "Text\n", "content_notices_explanation_check"),
        (
            "submitter_email",
            "Erika <erika@antik.example>",
            "content_notices_submitter_email_check",
        ),
        (
            "product_listing_title",
            "Vase\nRot",
            "content_notices_product_listing_title_check",
        ),
    ] {
        let error = sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE content_notices SET {column} = $2 WHERE content_notice_id = $1"
        )))
        .bind(notice.id().into_uuid())
        .bind(value)
        .execute(&pool)
        .await
        .expect_err("text the domain rejects");
        assert_eq!(
            Some(constraint),
            error
                .as_database_error()
                .and_then(|error| error.constraint()),
            "{column} = {value:?}"
        );
    }

    for urls in [
        vec!["https://a.example/", "https://a.example/"],
        vec!["ftp://a.example/"],
        vec![""],
    ] {
        let error = sqlx::query(
            "UPDATE content_notices SET additional_urls = $2 WHERE content_notice_id = $1",
        )
        .bind(notice.id().into_uuid())
        .bind(&urls)
        .execute(&pool)
        .await
        .expect_err("invalid additional URLs");
        assert_eq!(
            Some("content_notices_additional_urls_check"),
            error
                .as_database_error()
                .and_then(|error| error.constraint()),
            "{urls:?}"
        );
    }
}
