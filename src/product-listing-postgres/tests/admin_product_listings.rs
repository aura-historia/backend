use application::{
    operation_context::{CorrelationId, OperationContext, Principal, RequestId},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use auction_postgres::SqlxAuctionReferenceValidatorFactory;
use domain_primitives::{change_outcome::ChangeOutcome, event_id::EventId};
use indexmap::IndexSet;
use listing_source_core::ListingSourceId;
use localization::{Language, Localized};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use product_listing_core::{
    description::Description,
    listing_availability::ListingAvailability,
    listing_lifecycle::ListingLifecycle,
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_image::ProductListingImage,
    product_listing_slug_id::ProductListingSlugId,
    source_listing_id::SourceListingId,
    title::Title,
};
use product_listing_postgres::{
    SqlxAdminProductListingReader, SqlxProductListingEventAppenderFactory,
    SqlxProductListingRepositoryFactory,
};
use product_listing_service::{
    ports::{admin_product_listing_reader::*, *},
    use_cases::{
        admin_product_listing_error::AdminProductListingError,
        commands::{
            admin_product_listings::*, update_product_listing::UpdateProductListingCommand,
        },
        queries::admin_product_listings::*,
    },
};
use std::sync::Arc;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;
use tokio::sync::Barrier;
use url::Url;
use user_core::user_id::UserId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
struct Admin;
#[async_trait::async_trait]
impl CheckUserAdminUseCase for Admin {
    async fn execute(
        &self,
        _: &OperationContext,
        _: CheckUserAdminRequest,
    ) -> Result<CheckUserAdminResult, CheckUserAdminError> {
        Ok(CheckUserAdminResult)
    }
}
fn context() -> OperationContext {
    OperationContext {
        command_key: None,
        principal: Principal::User(UserId::new()),
        request_id: RequestId::new("admin-test"),
        correlation_id: CorrelationId::new("admin-test"),
    }
}
fn handler(
    pool: &sqlx::PgPool,
) -> AdminProductListingMutationHandler<
    SqlxUnitOfWork,
    SqlxProductListingRepositoryFactory,
    SqlxProductListingEventAppenderFactory,
    Admin,
    SqlxAuctionReferenceValidatorFactory,
> {
    AdminProductListingMutationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory::new(),
        SqlxProductListingEventAppenderFactory::new(),
        Admin,
        SqlxAuctionReferenceValidatorFactory::new(),
    )
}
async fn seed(pool: &sqlx::PgPool, source: Option<ListingSourceId>, key: &str) -> ProductListingId {
    let source = if let Some(id) = source {
        id
    } else {
        let party_id = uuid::Uuid::now_v7();
        let id = ListingSourceId::new();
        sqlx::query(
            "INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Admin fixture')",
        )
        .bind(party_id)
        .bind(format!("party-{party_id}"))
        .execute(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, 'Admin source', $3)").bind(id.as_uuid()).bind(format!("source-{}", id.as_uuid())).bind(party_id).execute(pool).await.unwrap();
        id
    };
    let id = ProductListingId::new();
    let mut product = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::from_title_and_suffix(
            &format!("Original title {}", id.as_uuid()),
            "a1b2c3",
        )
        .unwrap(),
        listing_source_id: source,
        source_listing_id: SourceListingId::try_from(key).unwrap(),
        title: Some(Localized::new(Language::En, Title::from("Original title"))),
        description: Some(Localized::new(
            Language::En,
            Description::from("Original description"),
        )),
        pricing: ProductListingPricing::default(),
        availability: Some(ListingAvailability::Available),
        url: Url::parse("https://source.example.test/listing").unwrap(),
        images: IndexSet::from([ProductListingImage::new(
            Url::parse("https://source.example.test/original.jpg").unwrap(),
        )]),
        auction: None,
    })
    .unwrap();
    let event = stamp_product_listing_event(
        id,
        OffsetDateTime::now_utc(),
        product.take_pending_event_payload().unwrap(),
    );
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxProductListingRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&product, event.event_id)
        .await
        .unwrap();
    SqlxProductListingEventAppenderFactory::new()
        .in_transaction(&mut tx)
        .append(&event)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    id
}
async fn detail(pool: &sqlx::PgPool, id: ProductListingId) -> AdminProductListingDetails {
    SqlxAdminProductListingReader::new(pool.clone())
        .find_by_id(id)
        .await
        .unwrap()
        .unwrap()
}
fn update(
    id: ProductListingId,
    version: ProductListingStorageVersion,
    patch: UpdateProductListingCommand,
) -> UpdateAdminProductListingCommand {
    UpdateAdminProductListingCommand {
        product_listing_id: id,
        expected_version: version,
        patch,
    }
}
fn lifecycle(
    id: ProductListingId,
    version: ProductListingStorageVersion,
) -> AdminProductListingLifecycleCommand {
    AdminProductListingLifecycleCommand {
        product_listing_id: id,
        expected_version: version,
    }
}
async fn events(pool: &sqlx::PgPool, id: ProductListingId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events WHERE product_listing_id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn admin_mutations_coalesce_preserve_originals_and_retain_lifecycle() {
    let pool = get_postgres_client().await;
    let id = seed(&pool, None, "SKU-42").await;
    let before = detail(&pool, id).await;
    let handler = handler(&pool);
    let context = context();
    let result = UpdateAdminProductListingUseCase::execute(
        &handler,
        &context,
        update(
            id,
            before.summary.version,
            UpdateProductListingCommand::default(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, result.outcome);
    assert_eq!(before, detail(&pool, id).await);
    assert_eq!(1, events(&pool, id).await);
    let images = IndexSet::from([ProductListingImage::new(
        Url::parse("https://source.example.test/replacement.jpg").unwrap(),
    )]);
    let patch = UpdateProductListingCommand {
        price: PatchField::Set(money::Price::new(100_u64.into(), money::Currency::Eur).into()),
        availability: PatchField::Set(ListingAvailability::SoldOut),
        images: PatchField::Set(images.clone()),
        ..Default::default()
    };
    let result = UpdateAdminProductListingUseCase::execute(
        &handler,
        &context,
        update(id, result.expected_version, patch),
    )
    .await
    .unwrap();
    assert_eq!(ChangeOutcome::Changed, result.outcome);
    assert_eq!(2, result.expected_version.into_inner());
    assert_eq!(2, events(&pool, id).await);
    let changed = detail(&pool, id).await;
    assert_eq!(None, changed.sale_observation);
    assert_eq!(before.summary.title, changed.summary.title);
    assert_eq!(before.description, changed.description);
    assert_eq!(
        before.summary.product_listing_title_slug_id,
        changed.summary.product_listing_title_slug_id
    );
    assert_eq!(
        before.summary.source_listing_id,
        changed.summary.source_listing_id
    );
    assert_eq!(images, changed.images);
    let markers: (uuid::Uuid, uuid::Uuid) = sqlx::query_as("SELECT current_event_id, embedding_source_event_id FROM product_listings WHERE product_listing_id = $1").bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(markers.0, markers.1);
    let result = WithdrawAdminProductListingUseCase::execute(
        &handler,
        &context,
        lifecycle(id, result.expected_version),
    )
    .await
    .unwrap();
    let withdrawn = detail(&pool, id).await;
    assert_eq!(ListingLifecycle::Withdrawn, withdrawn.summary.lifecycle);
    assert_eq!(None, withdrawn.summary.availability);
    assert_eq!(3, events(&pool, id).await);
    assert!(matches!(
        UpdateAdminProductListingUseCase::execute(
            &handler,
            &context,
            update(id, result.expected_version, Default::default())
        )
        .await,
        Err(AdminProductListingError::ListingWithdrawn)
    ));
    assert!(matches!(
        WithdrawAdminProductListingUseCase::execute(
            &handler,
            &context,
            lifecycle(id, before.summary.version)
        )
        .await,
        Err(AdminProductListingError::ConcurrencyConflict)
    ));
    let noop = WithdrawAdminProductListingUseCase::execute(
        &handler,
        &context,
        lifecycle(id, result.expected_version),
    )
    .await
    .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, noop.outcome);
    assert_eq!(withdrawn, detail(&pool, id).await);
    let restored = RestoreAdminProductListingUseCase::execute(
        &handler,
        &context,
        lifecycle(id, result.expected_version),
    )
    .await
    .unwrap();
    assert_eq!(4, restored.expected_version.into_inner());
    let active = detail(&pool, id).await;
    assert_eq!(ListingLifecycle::Active, active.summary.lifecycle);
    assert_eq!(None, active.summary.availability);
    let noop = RestoreAdminProductListingUseCase::execute(
        &handler,
        &context,
        lifecycle(id, restored.expected_version),
    )
    .await
    .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, noop.outcome);
    assert_eq!(active, detail(&pool, id).await);
    assert_eq!(4, events(&pool, id).await);
    for id in [ProductListingId::new()] {
        assert!(matches!(
            RestoreAdminProductListingUseCase::execute(
                &handler,
                &context,
                lifecycle(id, ProductListingStorageVersion::INITIAL)
            )
            .await,
            Err(AdminProductListingError::NotFound)
        ));
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn admin_image_reordering_commits_one_event_and_advances_embedding_source() {
    let pool = get_postgres_client().await;
    let id = seed(&pool, None, "Ordered-images").await;
    let handler = handler(&pool);
    let context = context();
    let images = [
        "https://source.example.test/first.jpg",
        "https://source.example.test/second.jpg",
    ]
    .into_iter()
    .map(|url| ProductListingImage::new(Url::parse(url).unwrap()))
    .collect::<IndexSet<_>>();
    let initial = UpdateAdminProductListingUseCase::execute(
        &handler,
        &context,
        update(
            id,
            ProductListingStorageVersion::INITIAL,
            UpdateProductListingCommand {
                images: PatchField::Set(images.clone()),
                ..Default::default()
            },
        ),
    )
    .await
    .unwrap();
    let reordered = images.iter().rev().cloned().collect::<IndexSet<_>>();
    let command = update(
        id,
        initial.expected_version,
        UpdateProductListingCommand {
            images: PatchField::Set(reordered.clone()),
            ..Default::default()
        },
    );
    let result = UpdateAdminProductListingUseCase::execute(&handler, &context, command.clone())
        .await
        .unwrap();
    assert_eq!(ChangeOutcome::Changed, result.outcome);
    assert_eq!(3, result.expected_version.into_inner());
    assert!(detail(&pool, id).await.images.iter().eq(reordered.iter()));
    let (current_event, embedding_source): (uuid::Uuid, uuid::Uuid) = sqlx::query_as(
        "SELECT current_event_id, embedding_source_event_id FROM product_listings WHERE product_listing_id = $1"
    ).bind(id.as_uuid()).fetch_one(&pool).await.unwrap();
    assert_eq!(current_event, embedding_source);
    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM product_listing_events WHERE event_id = $1 AND event_type = 'PRODUCT_LISTING_CHANGED'"
    ).bind(current_event).fetch_one(&pool).await.unwrap();
    assert_eq!(
        serde_json::json!({"images":{"previousCount":2,"currentCount":2}}),
        payload
    );
    assert_eq!(3, events(&pool, id).await);
    let before_noop = detail(&pool, id).await;
    let noop = UpdateAdminProductListingUseCase::execute(
        &handler,
        &context,
        UpdateAdminProductListingCommand {
            expected_version: result.expected_version,
            ..command
        },
    )
    .await
    .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, noop.outcome);
    assert_eq!(before_noop, detail(&pool, id).await);
    assert_eq!(3, events(&pool, id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn admin_directory_uses_source_scope_literals_and_timestamp_ties() {
    use application::pagination::Cursor;
    use domain_primitives::sort::SortOrder;
    let pool = get_postgres_client().await;
    let first = seed(&pool, None, "Same-%_ID").await;
    let first_detail = detail(&pool, first).await;
    let source = first_detail.summary.source.listing_source_id;
    let second = seed(&pool, Some(source), "Same-Other").await;
    let other_source = seed(&pool, None, "Same-%_ID").await;
    WithdrawAdminProductListingUseCase::execute(
        &handler(&pool),
        &context(),
        lifecycle(second, ProductListingStorageVersion::INITIAL),
    )
    .await
    .unwrap();
    sqlx::query(
        "UPDATE product_listings SET updated = '2026-01-01T00:00:00Z' WHERE listing_source_id = $1",
    )
    .bind(source.as_uuid())
    .execute(&pool)
    .await
    .unwrap();
    let reader = SqlxAdminProductListingReader::new(pool.clone());
    let scope = AdminProductListingSearchScope {
        query: None,
        listing_source_id: Some(source),
        source_listing_id: None,
        lifecycle: None,
        availability: None,
        auction_id: None,
        sort: AdminProductListingSort::Updated,
        order: SortOrder::Asc,
    };
    let request = AdminProductListingSearchRequest {
        scope: scope.clone(),
        cursor: Cursor {
            size: 1,
            search_after: None,
        },
    };
    let page = reader.search(&request).await.unwrap();
    assert_eq!(1, page.items.len());
    assert!(page.cursor.search_after.is_some());
    let next = reader
        .search(&AdminProductListingSearchRequest {
            scope: scope.clone(),
            cursor: page.cursor,
        })
        .await
        .unwrap();
    assert_eq!(1, next.items.len());
    assert!(next.cursor.search_after.is_none());
    assert_ne!(
        page.items[0].product_listing_id,
        next.items[0].product_listing_id
    );
    assert!(
        page.items[0].product_listing_id.as_uuid() < next.items[0].product_listing_id.as_uuid()
    );
    let mut exact = scope.clone();
    exact.query = Some("%_".to_owned());
    let page = reader
        .search(&AdminProductListingSearchRequest {
            scope: exact,
            cursor: Cursor {
                size: 21,
                search_after: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(
        vec![first],
        page.items
            .iter()
            .map(|item| item.product_listing_id)
            .collect::<Vec<_>>()
    );
    let mut key = scope.clone();
    key.source_listing_id = Some(SourceListingId::try_from("Same-%_ID").unwrap());
    let result = reader
        .search(&AdminProductListingSearchRequest {
            scope: key,
            cursor: Cursor {
                size: 21,
                search_after: None,
            },
        })
        .await
        .unwrap();
    assert_eq!(first, result.items[0].product_listing_id);
    assert_ne!(other_source, result.items[0].product_listing_id);
    let mut mismatched = scope.clone();
    mismatched.lifecycle = Some(ListingLifecycle::Active);
    let handler = SearchAdminProductListingsHandler::new(reader, Admin);
    let error = handler
        .execute(
            &context(),
            AdminProductListingSearchRequest {
                scope: mismatched,
                cursor: Cursor {
                    size: 1,
                    search_after: Some(AdminProductListingSearchCursor {
                        product_listing_id: first,
                        created: first_detail.summary.created,
                        updated: first_detail.summary.updated,
                        scope,
                    }),
                },
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdminProductListingError::CursorScopeMismatch
    ));
}

struct RacingFactory {
    barrier: Arc<Barrier>,
}
struct RacingRepository<R> {
    inner: R,
    barrier: Arc<Barrier>,
}
impl ProductListingRepositoryFactory<SqlxTransaction> for RacingFactory {
    fn in_transaction<'tx>(
        &'tx self,
        tx: &'tx mut SqlxTransaction,
    ) -> impl ProductListingRepository + 'tx {
        RacingRepository {
            inner: SqlxProductListingRepositoryFactory.in_transaction(tx),
            barrier: self.barrier.clone(),
        }
    }
}
#[async_trait::async_trait]
impl<R: ProductListingRepository> ProductListingRepository for RacingRepository<R> {
    async fn find_by_id(
        &mut self,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_id(id).await
    }
    async fn find_by_id_in_source(
        &mut self,
        source: listing_source_core::ListingSourceId,
        id: ProductListingId,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_id_in_source(source, id).await
    }

    async fn find_by_key(
        &mut self,
        key: &ProductListingKey,
    ) -> Result<Option<VersionedProductListing>, ProductListingRepositoryError> {
        self.inner.find_by_key(key).await
    }
    async fn insert(
        &mut self,
        product: &ProductListing,
        event: EventId,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        self.inner.insert(product, event).await
    }
    async fn update(
        &mut self,
        product: &ProductListing,
        version: ProductListingStorageVersion,
        event: EventId,
        effects: ProductListingWriteEffects,
    ) -> Result<VersionedProductListing, ProductListingRepositoryError> {
        self.barrier.wait().await;
        self.inner.update(product, version, event, effects).await
    }
}
#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn racing_admin_sessions_preserve_typed_cas_conflict() {
    let pool = get_postgres_client().await;
    let id = seed(&pool, None, "Race").await;
    let barrier = Arc::new(Barrier::new(2));
    let handler = AdminProductListingMutationHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        RacingFactory { barrier },
        SqlxProductListingEventAppenderFactory,
        Admin,
        SqlxAuctionReferenceValidatorFactory,
    );
    let context = context();
    let make = |amount: u64| {
        update(
            id,
            ProductListingStorageVersion::INITIAL,
            UpdateProductListingCommand {
                price: PatchField::Set(
                    money::Price::new(amount.into(), money::Currency::Eur).into(),
                ),
                ..Default::default()
            },
        )
    };
    let (left, right) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        tokio::join!(
            UpdateAdminProductListingUseCase::execute(&handler, &context, make(100_u64)),
            UpdateAdminProductListingUseCase::execute(&handler, &context, make(200_u64))
        )
    })
    .await
    .unwrap();
    assert_eq!(1, usize::from(left.is_ok()) + usize::from(right.is_ok()));
    let error = left.err().or_else(|| right.err()).unwrap();
    assert!(matches!(
        error,
        AdminProductListingError::ConcurrencyConflict
    ));
    assert_eq!(2, detail(&pool, id).await.summary.version.into_inner());
    assert_eq!(2, events(&pool, id).await);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn update_append_and_commit_failures_rollback_state_and_event() {
    let pool = get_postgres_client().await;
    let id = seed(&pool, None, "Failure-2038").await;
    let before = detail(&pool, id).await;
    sqlx::query("CREATE FUNCTION reject_admin_change_2038() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'injected admin write failure' USING ERRCODE = '23514'; END $$").execute(&pool).await.unwrap();
    for (create_trigger, commit_failure) in [
        (
            "CREATE TRIGGER reject_admin_update_2038 BEFORE UPDATE ON product_listings FOR EACH ROW WHEN (NEW.source_listing_id = 'Failure-2038') EXECUTE FUNCTION reject_admin_change_2038()",
            false,
        ),
        (
            "CREATE TRIGGER reject_admin_event_2038 BEFORE INSERT ON product_listing_events FOR EACH ROW WHEN (NEW.event_type = 'PRODUCT_LISTING_CHANGED') EXECUTE FUNCTION reject_admin_change_2038()",
            false,
        ),
        (
            "CREATE CONSTRAINT TRIGGER reject_admin_commit_2038 AFTER UPDATE ON product_listings DEFERRABLE INITIALLY DEFERRED FOR EACH ROW WHEN (NEW.source_listing_id = 'Failure-2038') EXECUTE FUNCTION reject_admin_change_2038()",
            true,
        ),
    ] {
        sqlx::query(create_trigger).execute(&pool).await.unwrap();
        let error = UpdateAdminProductListingUseCase::execute(
            &handler(&pool),
            &context(),
            update(
                id,
                before.summary.version,
                UpdateProductListingCommand {
                    availability: PatchField::Clear,
                    ..Default::default()
                },
            ),
        )
        .await
        .unwrap_err();
        assert!(std::error::Error::source(&error).is_some());
        if commit_failure {
            assert!(matches!(
                error,
                AdminProductListingError::CommitTransactionFailed(_)
            ));
        }
        assert_eq!(before, detail(&pool, id).await);
        assert_eq!(1, events(&pool, id).await);
        sqlx::query("DROP TRIGGER IF EXISTS reject_admin_update_2038 ON product_listings")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TRIGGER IF EXISTS reject_admin_event_2038 ON product_listing_events")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DROP TRIGGER IF EXISTS reject_admin_commit_2038 ON product_listings")
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("DROP FUNCTION reject_admin_change_2038()")
        .execute(&pool)
        .await
        .unwrap();
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn auction_patches_preserve_lot_facts_and_reject_cross_source_references_atomically() {
    use auction_core::AuctionId;
    use product_listing_service::product_listing_auction_patch::ProductListingAuctionPatch;
    let pool = get_postgres_client().await;
    let id = seed(&pool, None, "Lot").await;
    let before = detail(&pool, id).await;
    let source = before.summary.source.listing_source_id;
    let other = seed(&pool, None, "Other-source").await;
    let other_source = detail(&pool, other).await.summary.source.listing_source_id;
    let auction = AuctionId::new();
    let wrong = AuctionId::new();
    for (auction_id, source_id) in [(auction, source), (wrong, other_source)] {
        sqlx::query("INSERT INTO auctions (auction_id, listing_source_id, source_auction_id) VALUES ($1,$2,'Auction-42')").bind(auction_id.as_uuid()).bind(source_id.as_uuid()).execute(&pool).await.unwrap();
    }
    let command = UpdateProductListingCommand {
        auction: PatchField::Set(ProductListingAuctionPatch {
            auction_id: PatchField::Set(auction),
            lot_number: PatchField::Set(
                product_listing_core::product_listing_auction::LotNumber::try_from("Lot A")
                    .unwrap(),
            ),
            ..Default::default()
        }),
        ..Default::default()
    };
    let changed = UpdateAdminProductListingUseCase::execute(
        &handler(&pool),
        &context(),
        update(id, before.summary.version, command),
    )
    .await
    .unwrap();
    let assigned = detail(&pool, id).await;
    let command = UpdateProductListingCommand {
        auction: PatchField::Set(ProductListingAuctionPatch {
            auction_id: PatchField::Clear,
            ..Default::default()
        }),
        ..Default::default()
    };
    let detached = UpdateAdminProductListingUseCase::execute(
        &handler(&pool),
        &context(),
        update(id, changed.expected_version, command),
    )
    .await
    .unwrap();
    let lot = detail(&pool, id).await.auction.unwrap();
    assert_eq!(None, lot.auction_id());
    assert_eq!(assigned.auction.unwrap().lot_number(), lot.lot_number());
    let before_invalid = detail(&pool, id).await;
    for (auction_id, missing) in [(wrong, false), (AuctionId::new(), true)] {
        let command = UpdateProductListingCommand {
            price: PatchField::Set(money::Price::new(999_u64.into(), money::Currency::Eur).into()),
            auction: PatchField::Set(ProductListingAuctionPatch {
                auction_id: PatchField::Set(auction_id),
                ..Default::default()
            }),
            ..Default::default()
        };
        let result = UpdateAdminProductListingUseCase::execute(
            &handler(&pool),
            &context(),
            update(id, detached.expected_version, command),
        )
        .await;
        if missing {
            assert!(matches!(
                result,
                Err(AdminProductListingError::AuctionNotFound)
            ));
        } else {
            assert!(matches!(
                result,
                Err(AdminProductListingError::AuctionSourceMismatch)
            ));
        }
        assert_eq!(before_invalid, detail(&pool, id).await);
        assert_eq!(3, events(&pool, id).await);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn withdrawal_and_restore_preserve_sale_observations_watchlist_and_later_ingestion_precedence()
 {
    use product_listing_service::use_cases::{
        UpsertProductListingCommand, UpsertProductListingHandler, UpsertProductListingUseCase,
    };
    let pool = get_postgres_client().await;
    let id = seed(&pool, None, "Retained").await;
    let fx = uuid::Uuid::now_v7();
    let watcher = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO fx_rates (fx_rate_id,captured_at,source,source_event_id) VALUES ($1,'2026-01-01T00:00:00Z','fixture',$2)").bind(fx).bind(fx.to_string()).execute(&pool).await.unwrap();
    sqlx::query("UPDATE product_listings SET sale_observed_at='2026-01-01T00:00:00Z',sale_observation_fx_rate_id=$2 WHERE product_listing_id=$1").bind(id.as_uuid()).bind(fx).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO users (user_id,email,role,tier) VALUES ($1,$2,'USER','FREE')")
        .bind(watcher)
        .bind(format!("{watcher}@example.test"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO product_listing_watchlist (user_id,product_listing_id,state,active_since,notifications_enabled_since) VALUES ($1,$2,'ACTIVE',now(),now())").bind(watcher).bind(id.as_uuid()).execute(&pool).await.unwrap();
    let before = detail(&pool, id).await;
    let withdrawn = WithdrawAdminProductListingUseCase::execute(
        &handler(&pool),
        &context(),
        lifecycle(id, before.summary.version),
    )
    .await
    .unwrap();
    assert_eq!(
        before.sale_observation,
        detail(&pool, id).await.sale_observation
    );
    RestoreAdminProductListingUseCase::execute(
        &handler(&pool),
        &context(),
        lifecycle(id, withdrawn.expected_version),
    )
    .await
    .unwrap();
    assert_eq!(
        before.sale_observation,
        detail(&pool, id).await.sale_observation
    );
    let mut system = context();
    system.principal = Principal::System;
    let upsert = UpsertProductListingHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        product_listing_postgres::SqlxPartnerProductListingAuthorizerFactory,
        SqlxAuctionReferenceValidatorFactory,
    );
    let ingest = || UpsertProductListingCommand {
        listing_source_id: before.summary.source.listing_source_id,
        source_listing_id: before.summary.source_listing_id.clone(),
        title: None,
        description: None,
        price: PatchField::Set(money::Price::new(555_u64.into(), money::Currency::Eur).into()),
        price_estimate_min: PatchField::Unchanged,
        price_estimate_max: PatchField::Unchanged,
        availability: PatchField::Set(ListingAvailability::InStock),
        url: None,
        images: PatchField::Unchanged,
        auction: PatchField::Unchanged,
    };
    upsert.execute(&system, ingest()).await.unwrap();
    let ingested = detail(&pool, id).await;
    assert_eq!(
        Some(ListingAvailability::InStock),
        ingested.summary.availability
    );
    WithdrawAdminProductListingUseCase::execute(
        &handler(&pool),
        &context(),
        lifecycle(id, ingested.summary.version),
    )
    .await
    .unwrap();
    upsert.execute(&system, ingest()).await.unwrap();
    let restored = detail(&pool, id).await;
    assert_eq!(ListingLifecycle::Active, restored.summary.lifecycle);
    assert_eq!(
        Some(ListingAvailability::InStock),
        restored.summary.availability
    );
    assert_eq!(before.sale_observation, restored.sale_observation);
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM product_listing_watchlist WHERE product_listing_id=$1 AND user_id=$2",
    )
    .bind(id.as_uuid())
    .bind(watcher)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(1, count);
}
