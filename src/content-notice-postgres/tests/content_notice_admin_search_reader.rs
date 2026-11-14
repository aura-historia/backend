use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    pagination::Cursor,
    transaction::{Transaction, UnitOfWork},
};
use content_notice_core::{
    additional_url::AdditionalUrls,
    content_notice::{ContentNotice, NewContentNotice},
    content_notice_id::ContentNoticeId,
    content_notice_state::ContentNoticeState,
    content_notice_target::{
        ContentNoticeTarget, ContentNoticeTargetType, ListingSourceSnapshot,
        ProductListingSnapshot, ProductListingTitleSnapshot, TargetSnapshot,
    },
    good_faith_statement_version::GoodFaithStatementVersion,
    notice_explanation::NoticeExplanation,
    reason_category::ReasonCategory,
    submitter::{Submitter, SubmitterEmail, SubmitterName},
    submitter_role::SubmitterRole,
};
use content_notice_postgres::{
    SqlxContentNoticeAdminSearchReaderFactory, SqlxContentNoticeRepositoryFactory,
};
use content_notice_service::{
    ports::{
        ContentNoticeAdminSearchReader, ContentNoticeAdminSearchReaderFactory,
        ContentNoticeRepository, ContentNoticeRepositoryFactory,
        ContentNoticeSubmissionFingerprint,
    },
    use_cases::queries::list_admin_content_notices::{
        ContentNoticeCursor, ContentNoticeSearch, ListAdminContentNoticesError,
        ListAdminContentNoticesHandler, ListAdminContentNoticesRequest,
        ListAdminContentNoticesResult, ListAdminContentNoticesUseCase, SortContentNoticeField,
    },
};
use domain_primitives::{
    query::range_query::RangeQuery,
    sort::{Sort, SortOrder},
};
use listing_source_core::{ListingSourceId, ListingSourceName};
use localization::Language;
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::product_listing_id::ProductListingId;
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{OffsetDateTime, macros::datetime};
use url::Url;
use user_core::user_id::UserId;
use user_service::ports::{
    UserAdminActorView, UserAdminReadError, UserAdminReader, UserAdminReaderFactory,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const FINGERPRINT: ContentNoticeSubmissionFingerprint =
    ContentNoticeSubmissionFingerprint::from_sha256([7; 32]);

struct Seed {
    target: ContentNoticeTarget,
    role: SubmitterRole,
    category: ReasonCategory,
    created: OffsetDateTime,
}

impl Seed {
    fn source(listing_source_id: ListingSourceId, created: OffsetDateTime) -> Self {
        Self {
            target: ContentNoticeTarget::ListingSource { listing_source_id },
            role: SubmitterRole::Provider,
            category: ReasonCategory::ProviderOptOut,
            created,
        }
    }

    fn listing(
        listing_source_id: ListingSourceId,
        product_listing_id: ProductListingId,
        created: OffsetDateTime,
    ) -> Self {
        Self {
            target: ContentNoticeTarget::ProductListing {
                product_listing_id,
                listing_source_id,
            },
            role: SubmitterRole::RightsHolder,
            category: ReasonCategory::RightsInfringement,
            created,
        }
    }

    fn reported(mut self, role: SubmitterRole, category: ReasonCategory) -> Self {
        self.role = role;
        self.category = category;
        self
    }

    fn notice(&self) -> ContentNotice {
        ContentNotice::submit(NewContentNotice {
            id: ContentNoticeId::new(),
            target: self.target,
            target_snapshot: TargetSnapshot {
                listing_source: ListingSourceSnapshot {
                    name: ListingSourceName::try_from("Antiquitäten Muster").unwrap(),
                    url: Some(Url::parse("https://antik.example/").unwrap()),
                    partner: true,
                },
                product_listing: self
                    .target
                    .product_listing_id()
                    .map(|_| ProductListingSnapshot {
                        title: Some(
                            ProductListingTitleSnapshot::try_from("Jugendstil-Vase").unwrap(),
                        ),
                        url: Url::parse("https://antik.example/vase").unwrap(),
                    }),
            },
            submitter_role: self.role,
            reason_category: self.category,
            explanation: self
                .category
                .requires_explanation()
                .then(|| NoticeExplanation::try_from("Bitte prüfen.").unwrap()),
            additional_urls: AdditionalUrls::default(),
            submitter: Submitter {
                name: SubmitterName::try_from("Erika Mustermann").unwrap(),
                email: SubmitterEmail::try_from("erika@antik.example").unwrap(),
                language: Language::De,
            },
            good_faith_statement_version: GoodFaithStatementVersion::GoodFaithV1,
        })
        .unwrap()
    }
}

async fn begin(pool: &PgPool) -> SqlxTransaction {
    SqlxUnitOfWork::new(pool.clone())
        .begin()
        .await
        .unwrap_or_else(|error| panic!("begin content notice transaction: {error}"))
}

/// Stores the notices with the given creation times and returns their IDs in seed order.
async fn seed(pool: &PgPool, seeds: &[Seed]) -> Vec<ContentNoticeId> {
    let repository = SqlxContentNoticeRepositoryFactory::new();
    let mut transaction = begin(pool).await;
    let mut ids = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let notice = seed.notice();
        repository
            .in_transaction(&mut transaction)
            .insert(&notice, FINGERPRINT)
            .await
            .unwrap();
        sqlx::query("UPDATE content_notices SET created = $1 WHERE content_notice_id = $2")
            .bind(seed.created)
            .bind(notice.id().into_uuid())
            .execute(transaction.connection())
            .await
            .unwrap();
        ids.push(notice.id());
    }
    transaction.commit().await.unwrap();
    ids
}

async fn search(
    pool: &PgPool,
    request: &ListAdminContentNoticesRequest,
) -> ListAdminContentNoticesResult {
    let reader = SqlxContentNoticeAdminSearchReaderFactory::new();
    let mut transaction = begin(pool).await;
    let result = reader
        .in_transaction(&mut transaction)
        .search(request)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    result
}

async fn search_ids(pool: &PgPool, search: ContentNoticeSearch) -> Vec<ContentNoticeId> {
    let request = ListAdminContentNoticesRequest {
        search,
        ..Default::default()
    };
    ids_of(&self::search(pool, &request).await)
}

fn ids_of(result: &ListAdminContentNoticesResult) -> Vec<ContentNoticeId> {
    result.items.iter().map(|item| item.id).collect()
}

fn sorted(order: SortOrder) -> Option<Sort<SortContentNoticeField>> {
    Some(Sort {
        sort: SortContentNoticeField::Created,
        order,
    })
}

fn page(
    size: u64,
    search_after: Option<ContentNoticeCursor>,
) -> Option<Cursor<ContentNoticeCursor>> {
    Some(Cursor { size, search_after })
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_order_by_created_and_break_ties_by_id_in_the_same_direction() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    let tie = datetime!(2026-11-13 10:00:00.123456 UTC);
    let ids = seed(
        &pool,
        &[
            Seed::source(source, datetime!(2026-11-12 10:00 UTC)),
            Seed::source(source, tie),
            Seed::source(source, tie),
            Seed::source(source, tie),
            Seed::source(source, datetime!(2026-11-14 10:00 UTC)),
        ],
    )
    .await;
    let mut ties = ids[1..4].to_vec();
    ties.sort_by_key(|id| id.into_uuid());
    let ascending = [vec![ids[0]], ties, vec![ids[4]]].concat();
    let descending = ascending.iter().rev().copied().collect::<Vec<_>>();

    for (sort, expected) in [
        (None, &descending),
        (sorted(SortOrder::Desc), &descending),
        (sorted(SortOrder::Asc), &ascending),
    ] {
        let request = ListAdminContentNoticesRequest {
            sort,
            ..Default::default()
        };
        assert_eq!(*expected, ids_of(&search(&pool, &request).await));

        // Pages of one walk through the tie without skipping or repeating a notice.
        let mut walked = Vec::new();
        let mut cursor = page(1, None);
        loop {
            let result = search(
                &pool,
                &ListAdminContentNoticesRequest {
                    sort,
                    cursor: cursor.clone(),
                    ..Default::default()
                },
            )
            .await;
            walked.extend(ids_of(&result));
            match result.cursor.search_after {
                Some(after) => cursor = page(1, Some(after)),
                None => break,
            }
        }
        assert_eq!(*expected, walked);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_apply_every_filter() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    let other_source = ListingSourceId::new();
    let listing = ProductListingId::new();
    let ids = seed(
        &pool,
        &[
            Seed::source(source, datetime!(2026-01-01 00:00 UTC)),
            Seed::listing(source, listing, datetime!(2026-02-01 00:00 UTC)),
            Seed::listing(
                other_source,
                ProductListingId::new(),
                datetime!(2026-03-01 00:00 UTC),
            )
            .reported(
                SubmitterRole::IllegalContentReporter,
                ReasonCategory::IllegalContent,
            ),
        ],
    )
    .await;
    let [source_notice, listing_notice, reported_notice] = [ids[0], ids[1], ids[2]];

    let cases = [
        (
            ContentNoticeSearch::default(),
            vec![reported_notice, listing_notice, source_notice],
        ),
        (
            ContentNoticeSearch {
                target_type: Some(ContentNoticeTargetType::ListingSource),
                ..Default::default()
            },
            vec![source_notice],
        ),
        (
            ContentNoticeSearch {
                target_type: Some(ContentNoticeTargetType::ProductListing),
                ..Default::default()
            },
            vec![reported_notice, listing_notice],
        ),
        (
            ContentNoticeSearch {
                listing_source_id: Some(source),
                ..Default::default()
            },
            vec![listing_notice, source_notice],
        ),
        (
            ContentNoticeSearch {
                product_listing_id: Some(listing),
                ..Default::default()
            },
            vec![listing_notice],
        ),
        (
            ContentNoticeSearch {
                reason_category: Some(ReasonCategory::IllegalContent),
                ..Default::default()
            },
            vec![reported_notice],
        ),
        (
            ContentNoticeSearch {
                submitter_role: Some(SubmitterRole::RightsHolder),
                ..Default::default()
            },
            vec![listing_notice],
        ),
        (
            ContentNoticeSearch {
                created: Some(RangeQuery {
                    min: Some(datetime!(2026-02-01 00:00 UTC)),
                    max: None,
                }),
                ..Default::default()
            },
            vec![reported_notice, listing_notice],
        ),
        (
            ContentNoticeSearch {
                created: Some(RangeQuery {
                    min: None,
                    max: Some(datetime!(2026-02-01 00:00 UTC)),
                }),
                ..Default::default()
            },
            vec![listing_notice, source_notice],
        ),
        (
            ContentNoticeSearch {
                target_type: Some(ContentNoticeTargetType::ListingSource),
                listing_source_id: Some(other_source),
                ..Default::default()
            },
            vec![],
        ),
    ];

    for (search, expected) in cases {
        assert_eq!(
            expected,
            search_ids(&pool, search.clone()).await,
            "{search:?}"
        );
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_map_summary_from_the_submission_snapshot() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    let listing = ProductListingId::new();
    let created = datetime!(2026-11-13 10:44:18.123 UTC);
    let ids = seed(
        &pool,
        &[
            Seed::source(source, created),
            Seed::listing(source, listing, created),
        ],
    )
    .await;

    let result = search(&pool, &ListAdminContentNoticesRequest::default()).await;

    let source_summary = result.items.iter().find(|item| item.id == ids[0]).unwrap();
    assert_eq!(
        ContentNoticeTarget::ListingSource {
            listing_source_id: source
        },
        source_summary.target
    );
    assert_eq!(None, source_summary.product_listing_title);
    assert_eq!(
        ReasonCategory::ProviderOptOut,
        source_summary.reason_category
    );

    let listing_summary = result.items.iter().find(|item| item.id == ids[1]).unwrap();
    assert_eq!(
        ContentNoticeTarget::ProductListing {
            product_listing_id: listing,
            listing_source_id: source,
        },
        listing_summary.target
    );
    assert_eq!(ContentNoticeState::Submitted, listing_summary.state);
    assert_eq!(created, listing_summary.created);
    assert_eq!(
        "Antiquitäten Muster",
        listing_summary.listing_source_name.as_ref()
    );
    assert_eq!(
        Some("Jugendstil-Vase"),
        listing_summary
            .product_listing_title
            .as_ref()
            .map(|title| title.as_str())
    );
    assert!(listing_summary.listing_source_partner);
    assert_eq!(SubmitterRole::RightsHolder, listing_summary.submitter_role);
    assert_eq!("Erika Mustermann", listing_summary.submitter_name.as_str());
    assert_eq!(
        "erika@antik.example",
        listing_summary.submitter_email.as_str()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_continue_across_three_pages_without_gaps_or_duplicates() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    let tie = datetime!(2026-11-13 10:00:00.123456 UTC);
    let seeds = (0..7)
        .map(|index| {
            let created = if (2..5).contains(&index) {
                tie
            } else {
                tie + time::Duration::minutes(index - 3)
            };
            Seed::source(source, created)
        })
        .collect::<Vec<_>>();
    seed(&pool, &seeds).await;
    let filters = ContentNoticeSearch {
        listing_source_id: Some(source),
        ..Default::default()
    };
    let all = search_ids(&pool, filters.clone()).await;
    assert_eq!(7, all.len());

    let mut pages = Vec::new();
    let mut cursor = page(3, None);
    loop {
        let result = search(
            &pool,
            &ListAdminContentNoticesRequest {
                search: filters.clone(),
                sort: None,
                cursor: cursor.clone(),
            },
        )
        .await;
        pages.push(ids_of(&result));
        match result.cursor.search_after {
            Some(after) => cursor = page(3, Some(after)),
            None => break,
        }
    }

    assert_eq!(
        vec![3, 3, 1],
        pages.iter().map(Vec::len).collect::<Vec<_>>()
    );
    assert_eq!(all, pages.concat());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_return_an_empty_terminal_page_when_nothing_matches() {
    let pool = get_postgres_client().await;

    let result = search(&pool, &ListAdminContentNoticesRequest::default()).await;

    assert!(result.items.is_empty());
    assert_eq!(None, result.cursor.search_after);
    assert_eq!(21, result.cursor.size);
    assert_eq!(None, result.total);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_clamp_page_size_to_one_through_one_hundred() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    let start = datetime!(2026-11-13 10:00 UTC);
    let seeds = (0..101)
        .map(|index| Seed::source(source, start + time::Duration::seconds(index)))
        .collect::<Vec<_>>();
    seed(&pool, &seeds).await;

    for (requested, expected) in [(0, 1), (1, 1), (100, 100), (1_000, 100)] {
        let result = search(
            &pool,
            &ListAdminContentNoticesRequest {
                cursor: page(requested, None),
                ..Default::default()
            },
        )
        .await;

        assert_eq!(expected, result.items.len(), "size {requested}");
        assert_eq!(expected as u64, result.cursor.size, "size {requested}");
        assert!(result.cursor.search_after.is_some(), "size {requested}");
    }
}

/// Internal principals skip the role lookup, so no user rows are needed.
struct NoAdminLookup;

impl UserAdminReaderFactory<SqlxTransaction> for NoAdminLookup {
    fn in_transaction<'tx>(&'tx self, _tx: &'tx mut SqlxTransaction) -> impl UserAdminReader + 'tx {
        NoAdminLookup
    }
}

#[async_trait::async_trait]
impl UserAdminReader for NoAdminLookup {
    async fn find_admin_actor(
        &mut self,
        _user_id: UserId,
    ) -> Result<Option<UserAdminActorView>, UserAdminReadError> {
        panic!("an internal principal must not look up an admin role")
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_reject_a_cursor_issued_for_other_filters() {
    let pool = get_postgres_client().await;
    let source = ListingSourceId::new();
    let start = datetime!(2026-11-13 10:00 UTC);
    seed(
        &pool,
        &[
            Seed::source(source, start),
            Seed::source(source, start + time::Duration::minutes(1)),
        ],
    )
    .await;
    let handler = ListAdminContentNoticesHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxContentNoticeAdminSearchReaderFactory::new(),
        NoAdminLookup,
    );
    let context = OperationContext {
        principal: Principal::System,
        request_id: RequestId::new("request"),
        correlation_id: CorrelationId::new("correlation"),
    };
    let filtered = ListAdminContentNoticesRequest {
        search: ContentNoticeSearch {
            listing_source_id: Some(source),
            ..Default::default()
        },
        sort: None,
        cursor: page(1, None),
    };
    let first = handler.execute(&context, filtered.clone()).await.unwrap();
    let continuation = page(1, first.cursor.search_after.clone());

    let same_filters = handler
        .execute(
            &context,
            ListAdminContentNoticesRequest {
                cursor: continuation.clone(),
                ..filtered.clone()
            },
        )
        .await
        .unwrap();
    let other_filters = handler
        .execute(
            &context,
            ListAdminContentNoticesRequest {
                search: ContentNoticeSearch::default(),
                cursor: continuation.clone(),
                ..filtered.clone()
            },
        )
        .await;
    let other_order = handler
        .execute(
            &context,
            ListAdminContentNoticesRequest {
                sort: sorted(SortOrder::Asc),
                cursor: continuation,
                ..filtered
            },
        )
        .await;

    assert_eq!(1, same_filters.items.len());
    assert_ne!(ids_of(&first), ids_of(&same_filters));
    assert!(matches!(
        other_filters,
        Err(ListAdminContentNoticesError::CursorScopeMismatch)
    ));
    assert!(matches!(
        other_order,
        Err(ListAdminContentNoticesError::CursorScopeMismatch)
    ));
}
