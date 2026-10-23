use application::operation_context::{CorrelationId, OperationContext, Principal, RequestId};
use application::patch_field::PatchField;
use application::transaction::{Transaction, UnitOfWork};
use embedding::{
    EmbeddingError, EmbeddingGenerator, EmbeddingImageUrl, EmbeddingText, EmbeddingVector,
};
use localization::Language;
use money::Currency;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_search::ProductListingSearch;
use search_filter_core::{MatchingEvidencePolicy, search_filter_state::SearchFilterState};
use search_filter_postgres::{
    SqlxPeriodicSearchFilterProgressFactory, SqlxSearchFilterQuotaReaderFactory,
    SqlxSearchFilterReader, SqlxSearchFilterRepositoryFactory,
};
use search_filter_service::ports::{
    PeriodicSearchFilterProgress, PeriodicSearchFilterProgressFactory,
    PeriodicSearchFilterProgressLockOutcome,
};
use search_filter_service::use_cases::{
    CreateSearchFilterCommand, CreateSearchFilterError, CreateSearchFilterHandler,
    CreateSearchFilterUseCase, ProductListingSearchPatch, UpdateOwnedSearchFilterCommand,
    UpdateOwnedSearchFilterError, UpdateOwnedSearchFilterHandler, UpdateOwnedSearchFilterUseCase,
};
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::{tier::UserTier, user_id::UserId};
use user_postgres::SqlxUserTierEntitlementsFactory;
use user_service::ports::{UserTierEntitlements, UserTierEntitlementsFactory};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

struct TestEmbeddings;
#[async_trait::async_trait]
impl EmbeddingGenerator for TestEmbeddings {
    async fn embed_product(
        &self,
        _: &EmbeddingText,
        _: Option<&EmbeddingText>,
        _: Option<&EmbeddingImageUrl>,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        unreachable!("saved-filter writes embed only search intent")
    }
    async fn embed_search_query(
        &self,
        query: &EmbeddingText,
    ) -> Result<EmbeddingVector, EmbeddingError> {
        assert!(!query.as_str().is_empty());
        EmbeddingVector::try_new(vec![1.0; embedding::EMBEDDING_DIMENSIONS])
    }
}

fn context(principal: Principal) -> OperationContext {
    OperationContext {
        principal,
        request_id: RequestId::new("policy-test"),
        correlation_id: CorrelationId::new("policy-test"),
    }
}

async fn seed_user(pool: &sqlx::PgPool, tier: &str) -> UserId {
    let id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, $3, 'USER')")
        .bind(id.into_uuid())
        .bind(format!("{id}@example.test"))
        .bind(tier)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn policy_row(
    pool: &sqlx::PgPool,
    id: search_filter_core::user_search_filter_id::UserSearchFilterId,
) -> (String, i64, String) {
    sqlx::query_as("SELECT matching_evidence_policy, version, state FROM search_filters WHERE user_search_filter_id=$1")
        .bind(id.into_uuid()).fetch_one(pool).await.unwrap()
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn all_write_principals_resolve_policy_and_ordinary_embeddings_from_locked_tier() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let create = CreateSearchFilterHandler::new(
        unit,
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    );
    for (tier, expected) in [
        ("FREE", MatchingEvidencePolicy::TextOnly),
        ("PRO", MatchingEvidencePolicy::TextAndOneImage),
        (
            "ULTIMATE",
            MatchingEvidencePolicy::EnhancedTextAndFourImages,
        ),
    ] {
        for principal_kind in 0..3 {
            let user_id = seed_user(&pool, tier).await;
            let principal = match principal_kind {
                0 => Principal::User(user_id),
                1 => Principal::Service("fixture-service".into()),
                _ => Principal::System,
            };
            let result = create
                .execute(
                    &context(principal),
                    CreateSearchFilterCommand {
                        accept_shipping_destination: None,
                        user_id,
                        name: "ordinary filter".into(),
                        notifications: false,
                        search: ProductListingSearch::new(Language::En, Currency::Eur)
                            .with_product_listing_query("brass lamp".try_into().unwrap()),
                    },
                )
                .await
                .unwrap();
            assert_eq!(
                expected.as_str(),
                policy_row(&pool, result.filter.search_filter_id).await.0
            );
            assert_eq!(Some(768), result.filter.embedding.as_ref().map(Vec::len));
            assert_eq!(SearchFilterState::Active, result.filter.state);
        }
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn unauthorized_enhanced_intent_cannot_become_executable_via_system_writes() {
    let pool = get_postgres_client().await;
    let create = CreateSearchFilterHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    );
    for tier in ["FREE", "PRO"] {
        let user_id = seed_user(&pool, tier).await;
        let result = create
            .execute(
                &context(Principal::System),
                CreateSearchFilterCommand {
                    accept_shipping_destination: None,
                    user_id,
                    name: "unauthorized".into(),
                    notifications: true,
                    search: ProductListingSearch::new(Language::En, Currency::Eur)
                        .with_enhanced_search_description("originals only".try_into().unwrap()),
                },
            )
            .await;
        assert!(matches!(
            result,
            Err(CreateSearchFilterError::SearchFilterFeatureRestricted { .. })
        ));
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM search_filters WHERE user_id=$1")
            .bind(user_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(0, count);
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn downgrade_preserves_intent_and_reactivation_resolves_current_policy() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "ULTIMATE").await;
    let create = CreateSearchFilterHandler::new(
        unit.clone(),
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    );
    let update = UpdateOwnedSearchFilterHandler::new(
        unit.clone(),
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterReader::new(pool.clone()),
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    );
    let created = create
        .execute(
            &context(Principal::System),
            CreateSearchFilterCommand {
                accept_shipping_destination: None,
                user_id,
                name: "enhanced filter".into(),
                notifications: false,
                search: ProductListingSearch::new(Language::En, Currency::Eur)
                    .with_product_listing_query("lamp".try_into().unwrap())
                    .with_enhanced_search_description("Prefer blue glass".try_into().unwrap()),
            },
        )
        .await
        .unwrap()
        .filter;
    let mut tx = unit.begin().await.unwrap();
    SqlxUserTierEntitlementsFactory
        .in_transaction(&mut tx)
        .lock_user_tier(user_id)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET tier='PRO' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(tx.connection())
        .await
        .unwrap();
    SqlxUserTierEntitlementsFactory
        .in_transaction(&mut tx)
        .reconcile_for_tier(user_id, UserTier::Pro)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        (
            "TEXT_AND_ONE_IMAGE".into(),
            2,
            "INACTIVE_BY_RESTRICTED_PLAN".into()
        ),
        policy_row(&pool, created.search_filter_id).await
    );
    let mut command = UpdateOwnedSearchFilterCommand {
        accept_shipping_destination: None,
        user_id,
        search_filter_id: created.search_filter_id,
        name: PatchField::Unchanged,
        notifications: PatchField::Unchanged,
        state: PatchField::Set(SearchFilterState::Active),
        search: ProductListingSearchPatch::default(),
    };
    let denied = update
        .execute(&context(Principal::System), command.clone())
        .await;
    assert!(matches!(
        denied,
        Err(UpdateOwnedSearchFilterError::SearchFilterFeatureRestricted { .. })
    ));
    let preserved = SqlxSearchFilterReader::new(pool.clone());
    use search_filter_service::ports::SearchFilterReader;
    assert_eq!(
        created.search.enhanced_search_description,
        preserved
            .find_for_user_by_id(user_id, created.search_filter_id)
            .await
            .unwrap()
            .unwrap()
            .search
            .enhanced_search_description
    );
    command.search.enhanced_search_description = PatchField::Clear;
    let restored = update
        .execute(&context(Principal::System), command)
        .await
        .unwrap()
        .filter;
    assert_eq!(SearchFilterState::Active, restored.state);
    assert!(restored.search.enhanced_search_description.is_none());
    assert_eq!(
        "TEXT_AND_ONE_IMAGE",
        policy_row(&pool, created.search_filter_id).await.0
    );
    assert!(restored.embedding.is_some());
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn active_policy_upgrade_fences_inflight_snapshot_and_unchanged_writes_repair_embeddings() {
    let pool = get_postgres_client().await;
    let unit = SqlxUnitOfWork::new(pool.clone());
    let user_id = seed_user(&pool, "FREE").await;
    let create = CreateSearchFilterHandler::new(
        unit.clone(),
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    );
    let filter = create
        .execute(
            &context(Principal::System),
            CreateSearchFilterCommand {
                accept_shipping_destination: None,
                user_id,
                name: "query".into(),
                notifications: false,
                search: ProductListingSearch::new(Language::En, Currency::Eur)
                    .with_product_listing_query("lamp".try_into().unwrap()),
            },
        )
        .await
        .unwrap()
        .filter;
    let mut tier_tx = unit.begin().await.unwrap();
    SqlxUserTierEntitlementsFactory
        .in_transaction(&mut tier_tx)
        .lock_user_tier(user_id)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET tier='ULTIMATE' WHERE user_id=$1")
        .bind(user_id.into_uuid())
        .execute(tier_tx.connection())
        .await
        .unwrap();
    SqlxUserTierEntitlementsFactory
        .in_transaction(&mut tier_tx)
        .reconcile_for_tier(user_id, UserTier::Ultimate)
        .await
        .unwrap();
    tier_tx.commit().await.unwrap();
    assert_eq!(
        ("ENHANCED_TEXT_AND_FOUR_IMAGES".into(), 2, "ACTIVE".into()),
        policy_row(&pool, filter.search_filter_id).await
    );
    let mut final_tx = unit.begin().await.unwrap();
    assert_eq!(
        PeriodicSearchFilterProgressLockOutcome::ChangedOrInactive,
        SqlxPeriodicSearchFilterProgressFactory
            .in_transaction(&mut final_tx)
            .lock_and_read(
                filter.search_filter_id,
                1,
                filter.created,
                filter.created + time::Duration::hours(1)
            )
            .await
            .unwrap()
    );
    drop(final_tx);
    sqlx::query("UPDATE search_filters SET embedding=NULL WHERE user_search_filter_id=$1")
        .bind(filter.search_filter_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let update = UpdateOwnedSearchFilterHandler::new(
        unit,
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterReader::new(pool.clone()),
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    );
    let repaired = update
        .execute(
            &context(Principal::Service("repair-fixture".into())),
            UpdateOwnedSearchFilterCommand {
                accept_shipping_destination: None,
                user_id,
                search_filter_id: filter.search_filter_id,
                name: PatchField::Unchanged,
                notifications: PatchField::Unchanged,
                state: PatchField::Unchanged,
                search: ProductListingSearchPatch::default(),
            },
        )
        .await
        .unwrap()
        .filter;
    assert_eq!(Some(768), repaired.embedding.as_ref().map(Vec::len));
    assert_eq!(3, policy_row(&pool, filter.search_filter_id).await.1);
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn saved_shipping_accepts_authorized_country_values_and_survives_bookmark_changes_until_erasure()
 {
    use geo::{AddressText, CountryCode, GeographicDescription, reference::ReferenceRelease};
    use product_listing_core::shipping_search::ShippingSearch;
    use search_filter_service::{
        ports::SearchFilterReader, shipping_destination::AcceptShippingDestination,
    };
    use user_core::user_address::*;
    use user_service::address_book::*;
    let pool = get_postgres_client().await;
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default) VALUES('SAVED_SHIPPING_TEST',true)").execute(&pool).await.unwrap();
    let owner = seed_user(&pool, "PRO").await;
    let foreign = seed_user(&pool, "FREE").await;
    let ctx = context(Principal::User(owner));
    let unit = SqlxUnitOfWork::new(pool.clone());
    let book = std::sync::Arc::new(AddressBookHandler::new(
        unit.clone(),
        user_postgres::SqlxAddressBookFactory,
        geo_postgres::SqlxResolutionRepositoryFactory,
    ));
    let geography = |country| {
        GeographicDescription::new(
            Some(AddressText::new("private shipping snapshot street").unwrap()),
            Some(country),
            None,
            Some(geo::PostalCode::new("10115").unwrap()),
        )
        .unwrap()
        .unwrap()
    };
    let address = book
        .create(
            &ctx,
            CreateUserAddress {
                label: UserAddressLabel::new("private name".into()).unwrap(),
                geography: geography(CountryCode::DEU),
            },
        )
        .await
        .unwrap();
    let foreign_address = book
        .create(
            &context(Principal::User(foreign)),
            CreateUserAddress {
                label: UserAddressLabel::new("foreign".into()).unwrap(),
                geography: geography(CountryCode::DEU),
            },
        )
        .await
        .unwrap();
    let mut search = ProductListingSearch::new(Language::En, Currency::Eur);
    search.shipping = Some(
        ShippingSearch::new(
            ReferenceRelease::CURRENT,
            Some(CountryCode::DEU),
            None,
            Some(money::Price::new(9000_u64.into(), Currency::Eur)),
            false,
        )
        .unwrap(),
    );
    let create = CreateSearchFilterHandler::new(
        unit.clone(),
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    )
    .with_shipping_destinations(book.clone());
    let command = CreateSearchFilterCommand {
        accept_shipping_destination: Some(AcceptShippingDestination::Address(address.id)),
        user_id: owner,
        name: "country snapshot".into(),
        notifications: false,
        search,
    };
    let mut invalid = command.clone();
    invalid.accept_shipping_destination =
        Some(AcceptShippingDestination::Address(foreign_address.id));
    assert!(matches!(
        create.execute(&ctx, invalid).await,
        Err(CreateSearchFilterError::ShippingDestination(
            AddressBookError::NotFound
        ))
    ));
    let delegated = context(Principal::DelegatedUser {
        user_id: owner,
        capabilities: [application::operation_context::CredentialCapability::SearchFiltersWrite]
            .into(),
    });
    assert!(matches!(
        create.execute(&delegated, command.clone()).await,
        Err(CreateSearchFilterError::ShippingDestination(
            AddressBookError::Forbidden
        ))
    ));
    let filter = create.execute(&ctx, command).await.unwrap().filter;
    // Address ownership and a saved predicate do not select any browsing destination.
    assert_eq!(
        BrowsingDestination::None,
        book.destination(&ctx).await.unwrap().destination
    );
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT search FROM search_filters WHERE user_search_filter_id=$1")
            .bind(filter.search_filter_id.into_uuid())
            .fetch_one(&pool)
            .await
            .unwrap();
    let text = stored.to_string();
    for private in [
        address.id.to_string().as_str(),
        "private shipping snapshot street",
        "10115",
        "private name",
    ] {
        assert!(!text.contains(private));
    }
    assert_eq!("DE", stored["shipping"]["country"]);
    assert_eq!(
        ShippingSearch::INTERPRETATION,
        stored["shipping"]["interpretation"]
    );
    book.set_destination(
        &ctx,
        BrowsingDestination::CountryPostal {
            country: CountryCode::FRA,
            postal_code: None,
        },
        1,
    )
    .await
    .unwrap();
    book.update(
        &ctx,
        UpdateUserAddress {
            id: address.id,
            expected_version: address.version,
            label: UserAddressLabel::new("edited".into()).unwrap(),
            geography: geography(CountryCode::FRA),
        },
    )
    .await
    .unwrap();
    book.delete(&ctx, address.id, address.version + 1)
        .await
        .unwrap();
    let reader = search_filter_postgres::SqlxSearchFilterReader::new(pool.clone());
    let unchanged = reader
        .find_for_user_by_id(owner, filter.search_filter_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(filter.search, unchanged.search);
    let update = UpdateOwnedSearchFilterHandler::new(
        unit,
        SqlxSearchFilterRepositoryFactory,
        TestEmbeddings,
        reader,
        SqlxSearchFilterQuotaReaderFactory,
        SqlxUserTierEntitlementsFactory,
    )
    .with_shipping_destinations(book.clone());
    let patch = UpdateOwnedSearchFilterCommand {
        accept_shipping_destination: Some(AcceptShippingDestination::Preference),
        user_id: owner,
        search_filter_id: filter.search_filter_id,
        name: PatchField::Unchanged,
        notifications: PatchField::Unchanged,
        state: PatchField::Unchanged,
        search: ProductListingSearchPatch::default(),
    };
    // Acceptance is explicit and guarded against a preference that has moved since authoring.
    assert!(matches!(
        update.execute(&ctx, patch.clone()).await,
        Err(UpdateOwnedSearchFilterError::ShippingDestination(
            AddressBookError::InvalidInput
        ))
    ));
    let mut accepted = patch;
    accepted.search.shipping = PatchField::Set(
        ShippingSearch::new(
            ReferenceRelease::CURRENT,
            Some(CountryCode::FRA),
            None,
            None,
            false,
        )
        .unwrap(),
    );
    let changed = update.execute(&ctx, accepted).await.unwrap().filter;
    assert_eq!(
        Some(CountryCode::FRA),
        changed.search.shipping.unwrap().country()
    );
    sqlx::query("DELETE FROM users WHERE user_id=$1")
        .bind(owner.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM search_filters WHERE user_id=$1")
        .bind(owner.into_uuid())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, remaining);
}
