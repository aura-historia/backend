use application::{
    operation_context::{OperationContext, Principal},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use domain_primitives::change_outcome::ChangeOutcome;
use listing_source_core::ListingSourceId;
use party_core::party_id::PartyId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    delivery::{DeliveryIntegration, ListingDelivery},
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_slug_id::ProductListingSlugId,
};
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
    delivery::{
        SqlxProductListingDeliveryReaderFactory, SqlxProductListingDeliveryReferencesFactory,
    },
};
use product_listing_service::{
    crawler_shipping::{
        ApplyCrawlerShippingHandler, ApplyCrawlerShippingUseCase, CrawlerShippingObservation,
    },
    ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, ProductListingStorageVersion, stamp_product_listing_event,
    },
    use_cases::*,
};
use shipping_core::{
    assessment::EffectiveDeliveryTerms,
    fixtures::gallery_terms,
    profile::*,
    selection::{ComponentOverride, ProfileSelection, SourceDefaultProfile},
    terms::*,
};
use shipping_service::{ports::ShippingDefaultRevision, use_cases::*};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
const SCHEMA: Postgres = Postgres::new("migrations");
fn context() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: "synthetic-delivery".into(),
        correlation_id: "synthetic-delivery".into(),
    }
}

fn crawler_observation(
    key: &ProductListingKey,
    version: ProductListingStorageVersion,
    text: &str,
) -> CrawlerShippingObservation {
    let product_listing_normalization::shipping::ShippingQuickCheck::Resolved(assertions) =
        product_listing_normalization::shipping::quick_check_shipping_with_context(&[text], &[])
    else {
        panic!("supported item fixture");
    };
    CrawlerShippingObservation {
        raw_revision_id: product_listing_service::ports::ProductListingRawRevisionId::new(),
        product_key: key.clone(),
        expected_version: version,
        acquired_at: Some(time::OffsetDateTime::now_utc()),
        assertions,
    }
}

#[aura_integration_test(services=[SCHEMA])]
async fn crawler_prohibition_preserves_shared_collection_and_producer_fences() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let profile = create_profile(&pool, source, 8000).await;
    AssignShippingDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        AssignDefaultCommand {
            import: None,
            source_id: source,
            profile_id: profile.id,
            expected_revision: ShippingDefaultRevision::INITIAL,
        },
    )
    .await
    .unwrap();
    let key = listing(&pool, source, "crawler-cabinet").await;
    let shipping = ApplyCrawlerShippingHandler::new(
        product_listing_postgres::delivery::SqlxCrawlerShippingSourceReader::new(pool.clone()),
        changer(&pool),
    );
    let first = crawler_observation(
        &key,
        ProductListingStorageVersion::INITIAL,
        "Collection only. Shipping is not available for this item.",
    );
    assert_eq!("APPLIED", shipping.execute(first.clone()).await.unwrap());
    let current = view(&pool, &key).await;
    let terms = effective(&current, source, &profile);
    assert_eq!(Some(&ShippingPolicy::Prohibited), terms.terms().shipping());
    assert_eq!(
        profile.content.terms.collection(),
        terms.terms().collection()
    );
    let event_count = events(&pool, &key).await;
    // A committed lost-response retry retains the original request and expected revision.
    assert_eq!("APPLIED", shipping.execute(first.clone()).await.unwrap());
    assert_eq!(event_count, events(&pool, &key).await);
    let stale = crawler_observation(
        &key,
        ProductListingStorageVersion::INITIAL,
        "Shipping for this item is available worldwide",
    );
    assert!(matches!(
        shipping.execute(stale).await,
        Err(ProductListingDeliveryError::StaleRevision)
    ));
    assert_eq!(current, view(&pool, &key).await);
    assert_eq!(event_count, events(&pool, &key).await);
    let mut fresh = crawler_observation(&key, current.version, "This item is collection only");
    assert_eq!("APPLIED", shipping.execute(fresh.clone()).await.unwrap());
    assert_eq!(
        event_count,
        events(&pool, &key).await,
        "a new observation of equal terms is not a semantic change"
    );

    // Another request cannot relabel the old acquisition as a newer observation.
    let mut delayed = first;
    delayed.raw_revision_id = product_listing_service::ports::ProductListingRawRevisionId::new();
    assert!(matches!(
        shipping.execute(delayed).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportOutOfOrder
        ))
    ));

    // Equal manual edits take ownership without needing a semantic change.
    let manual = apply(
        &pool,
        &key,
        current.version,
        ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        },
    )
    .await;
    fresh = crawler_observation(&key, manual.version, "This item is collection only");
    assert!(matches!(
        shipping.execute(fresh).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportOwnershipConflict
        ))
    ));

    // Manual removal remains protected even when a later crawler obtains the latest version.
    let cleared = apply(
        &pool,
        &key,
        manual.version,
        ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Inherit),
            ..Default::default()
        },
    )
    .await;
    let after_clear = crawler_observation(&key, cleared.version, "This item is collection only");
    assert!(matches!(
        shipping.execute(after_clear).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportOwnershipConflict
        ))
    ));
    let restored_shared = effective(&view(&pool, &key).await, source, &profile);
    assert_eq!(
        profile.content.terms.shipping(),
        restored_shared.terms().shipping()
    );
    assert_eq!(
        profile.content.terms.collection(),
        restored_shared.terms().collection()
    );

    let partner_key = listing(&pool, source, "partner-managed-cabinet").await;
    let partner_context = OperationContext {
        principal: Principal::Service("partner-producer".into()),
        ..context()
    };
    let partner = changer(&pool)
        .execute(
            &partner_context,
            ChangeProductListingDeliveryCommand {
                product_key: partner_key.clone(),
                expected_version: ProductListingStorageVersion::INITIAL,
                import: Some(import_metadata(1, "partner-shipping")),
                delivery: PatchField::Set(ProductListingDeliveryPatch {
                    shipping: PatchField::Set(ComponentOverride::Replace(
                        ShippingPolicy::Prohibited,
                    )),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    let crawler = crawler_observation(
        &partner_key,
        partner.version,
        "This item is collection only",
    );
    assert!(matches!(
        shipping.execute(crawler).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportOwnershipConflict
        ))
    ));
}
async fn seed_source(pool: &PgPool) -> ListingSourceId {
    let party = PartyId::new();
    let source = ListingSourceId::new();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,$2,'Synthetic gallery')",
    )
    .bind(party.into_uuid())
    .bind(format!("gallery-{}", party.as_uuid().simple()))
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO listing_sources(listing_source_id,listing_source_slug_id,name,operator_party_id) VALUES($1,$2,'Synthetic gallery',$3)").bind(source.into_uuid()).bind(format!("gallery-{}",source.as_uuid().simple())).bind(party.into_uuid()).execute(pool).await.unwrap();
    source
}
async fn create_profile(pool: &PgPool, source: ListingSourceId, rate: u64) -> ProfileView {
    CreateShippingProfileHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        CreateProfileCommand {
            import: None,
            source_id: source,
            creation_key: format!("synthetic-{rate}"),
            content: ShippingProfileContent {
                name: ShippingProfileName::new("Gallery delivery".into()).unwrap(),
                attributed_party: None,
                terms: gallery_terms(rate),
            },
            evidence: vec![],
        },
    )
    .await
    .unwrap()
}
async fn listing(pool: &PgPool, source: ListingSourceId, code: &str) -> ProductListingKey {
    let id = ProductListingId::new();
    let key = ProductListingKey::new(source, code.try_into().unwrap());
    let mut listing = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::raw(&format!("{code}-a1b2c3")).unwrap(),
        listing_source_id: source,
        source_listing_id: key.source_listing_id.clone(),
        title: None,
        description: None,
        pricing: ProductListingPricing::default(),
        availability: None,
        url: url::Url::parse("https://example.test/painting").unwrap(),
        images: Default::default(),
        auction: None,
    })
    .unwrap();
    let event = stamp_product_listing_event(
        id,
        time::OffsetDateTime::UNIX_EPOCH,
        listing.take_pending_event_payload().unwrap(),
    );
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    SqlxProductListingRepositoryFactory
        .in_transaction(&mut tx)
        .insert(&listing, event.event_id)
        .await
        .unwrap();
    SqlxProductListingEventAppenderFactory
        .in_transaction(&mut tx)
        .append(&event)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    key
}
fn changer(pool: &PgPool) -> impl ChangeProductListingDeliveryUseCase {
    ChangeProductListingDeliveryHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        SqlxProductListingDeliveryReferencesFactory,
    )
}
async fn view(pool: &PgPool, key: &ProductListingKey) -> ProductListingDeliveryView {
    GetProductListingDeliveryHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingDeliveryReaderFactory,
        SqlxProductListingDeliveryReferencesFactory,
    )
    .execute(&context(), key.clone())
    .await
    .unwrap()
}
async fn apply(
    pool: &PgPool,
    key: &ProductListingKey,
    version: ProductListingStorageVersion,
    patch: ProductListingDeliveryPatch,
) -> ChangeProductListingDeliveryResult {
    changer(pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: None,
                product_key: key.clone(),
                expected_version: version,
                delivery: PatchField::Set(patch),
            },
        )
        .await
        .unwrap()
}
async fn events(pool: &PgPool, key: &ProductListingKey) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM product_listing_events e JOIN product_listings p USING(product_listing_id) WHERE p.listing_source_id=$1 AND p.source_listing_id=$2").bind(key.listing_source_id.into_uuid()).bind(key.source_listing_id.as_ref()).fetch_one(pool).await.unwrap()
}
fn effective(
    view: &ProductListingDeliveryView,
    source: ListingSourceId,
    profile: &ProfileView,
) -> EffectiveDeliveryTerms {
    let profile = ShippingProfile::rehydrate(
        profile.id,
        profile.source_id,
        profile.revision,
        profile.lifecycle,
        profile.content.clone(),
    )
    .unwrap();
    let default = SourceDefaultProfile::rehydrate(source, Some(profile.id()));
    let configuration = view
        .delivery
        .clone()
        .unwrap_or_else(|| ListingDelivery::inherited(source))
        .configuration();
    EffectiveDeliveryTerms::resolve(&configuration, Some(&default), &[profile])
}
#[aura_integration_test(services=[SCHEMA])]
async fn gallery_inherits_rates_without_copying_and_cabinet_retains_collection() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let profile = create_profile(&pool, source, 8000).await;
    AssignShippingDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        AssignDefaultCommand {
            import: None,
            source_id: source,
            profile_id: profile.id,
            expected_revision: ShippingDefaultRevision::INITIAL,
        },
    )
    .await
    .unwrap();
    let painting = listing(&pool, source, "painting").await;
    let cabinet = listing(&pool, source, "cabinet").await;
    assert!(view(&pool, &painting).await.delivery.is_none());
    let result = apply(
        &pool,
        &cabinet,
        ProductListingStorageVersion::INITIAL,
        ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(ChangeOutcome::Changed, result.outcome);
    let current = view(&pool, &cabinet).await;
    let terms = effective(&current, source, &profile);
    assert_eq!(Some(&ShippingPolicy::Prohibited), terms.terms().shipping());
    assert_eq!(
        profile.content.terms.collection(),
        terms.terms().collection()
    );
    let stored: serde_json::Value = sqlx::query_scalar(
        "SELECT delivery_configuration FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(current.product_listing_id.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        serde_json::json!({"kind":"REPLACE","value":{"kind":"PROHIBITED"}}),
        stored["shipping"]
    );
    assert!(!stored.to_string().contains("8000"));
    let before = (
        events(&pool, &painting).await,
        events(&pool, &cabinet).await,
        current.version,
    );
    let updated = ReplaceShippingProfileHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        ReplaceProfileCommand {
            import: None,
            source_id: source,
            profile_id: profile.id,
            expected_revision: profile.management_revision,
            content: ShippingProfileContent {
                terms: gallery_terms(9500),
                ..profile.content.clone()
            },
            evidence: vec![],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        before,
        (
            events(&pool, &painting).await,
            events(&pool, &cabinet).await,
            view(&pool, &cabinet).await.version
        )
    );
    assert_eq!(
        Some(&ShippingPolicy::Prohibited),
        effective(&view(&pool, &cabinet).await, source, &updated)
            .terms()
            .shipping()
    );
    assert_eq!(
        updated.content.terms.shipping(),
        effective(&view(&pool, &painting).await, source, &updated)
            .terms()
            .shipping()
    );
    let removed = apply(
        &pool,
        &cabinet,
        result.version,
        ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Inherit),
            ..Default::default()
        },
    )
    .await;
    assert!(view(&pool, &cabinet).await.delivery.is_none());
    assert_eq!(3, removed.version.into_inner());
    assert_eq!(3, events(&pool, &cabinet).await);
    let versions:Vec<i16>=sqlx::query_scalar("SELECT event_type_schema_version FROM product_listing_events WHERE product_listing_id=$1 ORDER BY event_time,event_id").bind(current.product_listing_id.into_uuid()).fetch_all(&pool).await.unwrap();
    assert_eq!(vec![1, 2, 2], versions);
}
#[aura_integration_test(services=[SCHEMA])]
async fn replacement_suppression_assertion_removal_and_noops_have_distinct_revisions() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let profile = create_profile(&pool, source, 8000).await;
    let other = create_profile(&pool, source, 9500).await;
    let key = listing(&pool, source, "painting").await;
    let mut version = ProductListingStorageVersion::INITIAL;
    for selection in [
        Some(ProfileSelection::Explicit(profile.id)),
        Some(ProfileSelection::Explicit(other.id)),
        Some(ProfileSelection::SuppressInheritance),
        None,
        Some(ProfileSelection::InheritSourceDefault),
    ] {
        let result = apply(
            &pool,
            &key,
            version,
            ProductListingDeliveryPatch {
                selection: selection.clone().map_or(PatchField::Clear, PatchField::Set),
                ..Default::default()
            },
        )
        .await;
        assert_eq!(ChangeOutcome::Changed, result.outcome);
        version = result.version;
        let current = view(&pool, &key).await;
        assert_eq!(
            selection.as_ref(),
            current
                .delivery
                .as_ref()
                .map_or(Some(&ProfileSelection::InheritSourceDefault), |d| d
                    .selection())
        );
    }
    let before = events(&pool, &key).await;
    let no_op = apply(&pool, &key, version, ProductListingDeliveryPatch::default()).await;
    assert_eq!(ChangeOutcome::Unchanged, no_op.outcome);
    assert_eq!(version, no_op.version);
    assert_eq!(before, events(&pool, &key).await);
    // Public history retains the semantic notice without any protected configuration.
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let stored = SqlxProductListingRepositoryFactory
        .in_transaction(&mut tx)
        .find_by_key(&key)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.value.delivery().is_none());
    assert!(stored.value.title().is_none());
    assert_eq!(ProductListingPricing::default(), stored.value.pricing());
    assert!(stored.value.auction().is_none());
    use product_listing_service::ports::{
        ProductListingHistoryReader, ProductListingHistoryReaderFactory,
    };
    let history = product_listing_postgres::SqlxProductListingHistoryReaderFactory
        .in_transaction(&mut tx)
        .find_history(&ProductListingHistoryLookup::ById(stored.value.id()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(6, history.len());
    for entry in history.iter().skip(1) {
        let ProductListingHistoryEntryKind::Changed(changes) = &entry.kind else {
            panic!("delivery change history");
        };
        assert_eq!(
            &[ProductListingHistoryChange::DeliveryChanged],
            changes.as_slice()
        );
    }
    tx.commit().await.unwrap();
}

#[aura_integration_test(services=[SCHEMA])]
async fn unchanged_native_inheritance_revalidates_default_dependencies_and_clear_remains_possible()
{
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let profile = create_profile(&pool, source, 8000).await;
    AssignShippingDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        AssignDefaultCommand {
            import: None,
            source_id: source,
            profile_id: profile.id,
            expected_revision: ShippingDefaultRevision::INITIAL,
        },
    )
    .await
    .unwrap();
    let key = listing(&pool, source, "painting").await;
    let command = ChangeProductListingDeliveryCommand {
        import: None,
        product_key: key.clone(),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Unchanged,
    };
    let unchanged = changer(&pool)
        .execute(&context(), command.clone())
        .await
        .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, unchanged.outcome);
    assert!(view(&pool, &key).await.delivery.is_none());

    sqlx::query("UPDATE shipping_profiles SET lifecycle='RETIRED' WHERE shipping_profile_id=$1")
        .bind(profile.id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        changer(&pool).execute(&context(), command.clone()).await,
        Err(ProductListingDeliveryError::ReferenceIneligible)
    ));
    let cleared = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: None,
                delivery: PatchField::Clear,
                ..command
            },
        )
        .await
        .unwrap();
    assert_eq!(unchanged, cleared);
    assert_eq!(1, events(&pool, &key).await);
}
#[aura_integration_test(services=[SCHEMA])]
async fn foreign_missing_retired_references_withdrawal_corruption_and_races_fail_safely() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let key = listing(&pool, source, "painting").await;
    let foreign = seed_source(&pool).await;
    let profile = create_profile(&pool, foreign, 8000).await;
    for id in [profile.id, ShippingProfileId::new()] {
        let error = changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
                    import: None,
                    product_key: key.clone(),
                    expected_version: ProductListingStorageVersion::INITIAL,
                    delivery: PatchField::Set(ProductListingDeliveryPatch {
                        selection: PatchField::Set(ProfileSelection::Explicit(id)),
                        ..Default::default()
                    }),
                },
            )
            .await;
        assert!(matches!(
            error,
            Err(ProductListingDeliveryError::ReferenceIneligible)
        ));
    }
    let profile = create_profile(&pool, source, 8000).await;
    sqlx::query("UPDATE shipping_profiles SET lifecycle='RETIRED' WHERE shipping_profile_id=$1")
        .bind(profile.id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
                    import: None,
                    product_key: key.clone(),
                    expected_version: ProductListingStorageVersion::INITIAL,
                    delivery: PatchField::Set(ProductListingDeliveryPatch {
                        selection: PatchField::Set(ProfileSelection::Explicit(profile.id)),
                        ..Default::default()
                    })
                }
            )
            .await,
        Err(ProductListingDeliveryError::ReferenceIneligible)
    ));
    let command = ChangeProductListingDeliveryCommand {
        import: None,
        product_key: key.clone(),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        }),
    };
    let a = changer(&pool);
    let b = changer(&pool);
    let c = context();
    let (a, b) = tokio::join!(
        a.execute(&c, command.clone()),
        b.execute(&c, command.clone())
    );
    assert_eq!(1, usize::from(a.is_ok()) + usize::from(b.is_ok()));
    assert!(
        matches!(a, Err(ProductListingDeliveryError::StaleRevision))
            || matches!(b, Err(ProductListingDeliveryError::StaleRevision))
    );
    let current = view(&pool, &key).await;
    assert_eq!(2, current.version.into_inner());
    assert_eq!(2, events(&pool, &key).await);
    let id = current.product_listing_id;
    sqlx::query("UPDATE product_listings SET lifecycle='WITHDRAWN',availability=NULL WHERE product_listing_id=$1").bind(id.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
                    import: None,
                    expected_version: current.version,
                    delivery: PatchField::Unchanged,
                    ..command.clone()
                }
            )
            .await,
        Err(ProductListingDeliveryError::ListingWithdrawn)
    ));
    sqlx::query("UPDATE product_listings SET delivery_configuration=jsonb_set(delivery_configuration,'{selection}', '{\"kind\":\"EXPLICIT\",\"profile_id\":\"invalid\"}') WHERE product_listing_id=$1").bind(id.into_uuid()).execute(&pool).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    assert!(matches!(SqlxProductListingRepositoryFactory.in_transaction(&mut tx).find_by_id(id).await,Err(product_listing_service::ports::ProductListingRepositoryError::InvalidDeliveryPersisted {..})));
}

#[aura_integration_test(services=[SCHEMA])]
async fn current_integration_and_location_scope_are_checked_and_provider_clear_never_inherits() {
    use shipping_core::{inputs::Applicability, selection::DeliveryOverrides};
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let key = listing(&pool, source, "provider-product").await;
    sqlx::query("INSERT INTO listing_source_ingestion_methods(listing_source_id,ingestion_method) VALUES($1,'SHOPIFY')").bind(source.into_uuid()).execute(&pool).await.unwrap();
    let integration = DeliveryIntegration::Provider {
        method: listing_source_core::ListingIngestionMethod::Shopify,
    };
    let pending = apply(
        &pool,
        &key,
        ProductListingStorageVersion::INITIAL,
        ProductListingDeliveryPatch {
            integration: integration.clone(),
            ..Default::default()
        },
    )
    .await;
    let before = view(&pool, &key).await;
    assert!(before.delivery.as_ref().unwrap().selection().is_none());
    let cleared = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: None,
                product_key: key.clone(),
                expected_version: pending.version,
                delivery: PatchField::Clear,
            },
        )
        .await
        .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, cleared.outcome);
    assert_eq!(before, view(&pool, &key).await);
    assert!(matches!(
        before.delivery.unwrap().configuration().selection(),
        ProfileSelection::SuppressInheritance
    ));
    let foreign_integration = DeliveryIntegration::Provider {
        method: listing_source_core::ListingIngestionMethod::Woocommerce,
    };
    assert!(matches!(
        changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
                    import: None,
                    product_key: key.clone(),
                    expected_version: pending.version,
                    delivery: PatchField::Set(ProductListingDeliveryPatch {
                        integration: foreign_integration,
                        ..Default::default()
                    })
                }
            )
            .await,
        Err(ProductListingDeliveryError::IntegrationMismatch)
    ));
    // Exact operator ownership and current location lifecycle are required for inline sites.
    let local_key = listing(&pool, source, "source-product").await;
    let owner: uuid::Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
    )
    .bind(source.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let location = party_core::party_location_id::PartyLocationId::new();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Synthetic collection site','ACTIVE','test','test')").bind(location.into_uuid()).bind(owner).execute(&pool).await.unwrap();
    let collection = CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Independent,
        Some(ShippingEligibility::Offered),
        Some(DeliverySite::PartyLocation(location)),
        None,
    );
    let result = apply(
        &pool,
        &local_key,
        ProductListingStorageVersion::INITIAL,
        ProductListingDeliveryPatch {
            collection: PatchField::Set(ComponentOverride::Replace(collection)),
            ..Default::default()
        },
    )
    .await;
    sqlx::query("UPDATE party_locations SET lifecycle='RETIRED' WHERE party_location_id=$1")
        .bind(location.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
                    import: None,
                    product_key: local_key.clone(),
                    expected_version: result.version,
                    delivery: PatchField::Unchanged
                }
            )
            .await,
        Err(ProductListingDeliveryError::ReferenceIneligible)
    ));
    let partial = CollectionPolicy::new(
        Applicability::default(),
        CollectionDependency::Independent,
        None,
        None,
        None,
    );
    let replacement = apply(
        &pool,
        &local_key,
        result.version,
        ProductListingDeliveryPatch {
            collection: PatchField::Set(ComponentOverride::Replace(partial.clone())),
            ..Default::default()
        },
    )
    .await;
    let local_view = view(&pool, &local_key).await;
    assert_eq!(replacement.version, local_view.version);
    assert_eq!(
        &ComponentOverride::Replace(partial),
        &local_view.delivery.unwrap().overrides().collection
    );
    // Rehydration is pure; explicit no-assignment remains distinct from the absent native config.
    assert!(
        ListingDelivery::new(
            source,
            integration,
            None,
            Default::default(),
            DeliveryOverrides::default()
        )
        .unwrap()
        .selection()
        .is_none()
    );
    sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_source_ingestion_methods VALUES($1,'WOOCOMMERCE')")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    // Reliable removal is possible after the old integration identity becomes ineligible.
    let removal = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: None,
                product_key: key.clone(),
                expected_version: pending.version,
                delivery: PatchField::Clear,
            },
        )
        .await
        .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, removal.outcome);
    let new_integration = DeliveryIntegration::Provider {
        method: listing_source_core::ListingIngestionMethod::Woocommerce,
    };
    let inputs = shipping_core::inputs::ListingRateInputs::new(
        None,
        Some(vec![
            shipping_core::inputs::SourceVariant::new(source, "opaque/Variant:2".into()).unwrap(),
        ]),
    )
    .unwrap();
    let changed = apply(
        &pool,
        &key,
        removal.version,
        ProductListingDeliveryPatch {
            integration: new_integration.clone(),
            rate_inputs: PatchField::Set(inputs.clone()),
            ..Default::default()
        },
    )
    .await;
    let current = view(&pool, &key).await;
    assert_eq!(changed.version, current.version);
    assert_eq!(
        &new_integration,
        current.delivery.as_ref().unwrap().integration()
    );
    assert_eq!(&inputs, current.delivery.as_ref().unwrap().rate_inputs());
    assert!(current.delivery.unwrap().selection().is_none());
}

#[aura_integration_test(services=[SCHEMA])]
async fn delivery_queue_replay_uses_the_committed_receipt_and_rejects_changed_retry_content() {
    use product_listing_service::ports::{
        ProductListingIngestionPublishError, ProductListingIngestionPublisher,
    };
    #[derive(Clone, Default)]
    struct Capture(std::sync::Arc<std::sync::Mutex<Vec<ProductListingIngestionMessage>>>);
    #[async_trait::async_trait]
    impl ProductListingIngestionPublisher for Capture {
        async fn publish(
            &self,
            messages: Vec<ProductListingIngestionMessage>,
        ) -> Result<Vec<ProductListingIngestionItemOutcome>, ProductListingIngestionPublishError>
        {
            let outcomes = messages
                .iter()
                .map(|m| ProductListingIngestionItemOutcome {
                    index: m.metadata.index,
                    command_id: m.metadata.command_id.clone(),
                    outcome: ProductListingIngestionOutcome::Accepted,
                })
                .collect();
            self.0.lock().unwrap().extend(messages);
            Ok(outcomes)
        }
    }
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let key = listing(&pool, source, "painting").await;
    let publisher = Capture::default();
    let intake = SubmitInternalProductListingIngestionHandler::new(publisher.clone());
    let command = ChangeProductListingDeliveryCommand {
        import: None,
        product_key: key.clone(),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        }),
    };
    let submission = ProductListingIngestionSubmission {
        listing_source_id: source,
        original_input_count: 1,
        idempotency_key: Some(
            ProductListingIngestionIdempotencyKey::new("synthetic-delivery-retry").unwrap(),
        ),
        items: vec![IndexedProductListingIngestionIntent {
            index: 0,
            intent: ProductListingIngestionIntent::ChangeDelivery(command),
        }],
    };
    let first = intake
        .execute(&context(), submission.clone())
        .await
        .unwrap();
    let second = intake
        .execute(&context(), submission.clone())
        .await
        .unwrap();
    assert_eq!(first.submission_id, second.submission_id);
    assert_eq!(first.items, second.items);
    let worker = ProcessProductListingIngestionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        product_listing_postgres::SqlxPartnerProductListingAuthorizerFactory,
        auction_postgres::SqlxAuctionReferenceValidatorFactory,
        product_listing_postgres::SqlxProductListingRawCaptureWriterFactory,
        product_listing_postgres::SqlxProductListingCommandReceiptStoreFactory,
    )
    .with_delivery_references(SqlxProductListingDeliveryReferencesFactory);
    let messages = publisher.0.lock().unwrap().clone();
    let envelope = |m: &ProductListingIngestionMessage| {
        product_listing_ingestion_sqs::codec::envelope(m)
            .unwrap()
            .into_service_envelope()
            .unwrap()
    };
    assert!(matches!(
        worker.execute(envelope(&messages[0])).await.unwrap(),
        ProductListingIngestionCompletion::Applied(ProductListingIngestionEffect::DeliveryChanged(
            _
        ))
    ));
    assert_eq!(
        ProductListingIngestionCompletion::AlreadyCompleted,
        worker.execute(envelope(&messages[1])).await.unwrap()
    );
    assert_eq!(2, events(&pool, &key).await);
    assert_eq!(2, view(&pool, &key).await.version.into_inner());
    let mut conflicting = submission;
    let ProductListingIngestionIntent::ChangeDelivery(ref mut c) = conflicting.items[0].intent
    else {
        panic!()
    };
    c.delivery = PatchField::Clear;
    intake.execute(&context(), conflicting).await.unwrap();
    let changed = publisher.0.lock().unwrap()[2].clone();
    assert!(matches!(
        worker.execute(envelope(&changed)).await,
        Err(ProductListingIngestionError::FingerprintConflict)
    ));
}

#[aura_integration_test(services=[SCHEMA])]
async fn execution_rechecks_partner_source_grants_and_rejects_missing_capability() {
    use application::operation_context::CredentialCapability;
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let key = listing(&pool, source, "painting").await;
    let owner: uuid::Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
    )
    .bind(source.into_uuid())
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = user_core::user_id::UserId::new();
    let partnership = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users(user_id,email,tier,role) VALUES($1,'synthetic-delivery-user@example.test','FREE','USER')").bind(user.into_uuid()).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO partnerships(partnership_id,party_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members(partnership_id,user_id) VALUES($1,$2)")
        .bind(partnership)
        .bind(user.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_listing_source_grants(partnership_id,listing_source_id) VALUES($1,$2)").bind(partnership).bind(source.into_uuid()).execute(&pool).await.unwrap();
    let ctx = OperationContext {
        principal: Principal::DelegatedUser {
            user_id: user,
            capabilities: [CredentialCapability::ProductListingsWrite].into(),
        },
        ..context()
    };
    let command = ChangeProductListingDeliveryCommand {
        import: None,
        product_key: key.clone(),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        }),
    };
    let result = changer(&pool).execute(&ctx, command.clone()).await.unwrap();
    let reader = OperationContext {
        principal: Principal::DelegatedUser {
            user_id: user,
            capabilities: [CredentialCapability::ProductListingsRead].into(),
        },
        ..context()
    };
    assert!(matches!(
        changer(&pool)
            .execute(
                &reader,
                ChangeProductListingDeliveryCommand {
                    import: None,
                    expected_version: result.version,
                    ..command.clone()
                }
            )
            .await,
        Err(ProductListingDeliveryError::Forbidden)
    ));
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE partnership_id=$1 AND listing_source_id=$2").bind(partnership).bind(source.into_uuid()).execute(&pool).await.unwrap();
    assert!(matches!(
        changer(&pool)
            .execute(
                &ctx,
                ChangeProductListingDeliveryCommand {
                    import: None,
                    expected_version: result.version,
                    delivery: PatchField::Clear,
                    ..command
                }
            )
            .await,
        Err(ProductListingDeliveryError::Forbidden)
    ));
    assert_eq!(2, events(&pool, &key).await);
}

fn import_metadata(sequence: u64, key: &str) -> shipping_service::imports::ShippingImportWrite {
    let at = time::OffsetDateTime::now_utc();
    shipping_service::imports::ShippingImportWrite {
        integration: DeliveryIntegration::Source,
        sequence,
        acquired_at: at,
        request_key: key.into(),
        object_key: None,
        observation: Some(shipping_service::imports::ShippingObservation { acquired_at: at }),
    }
}
async fn dependencies(
    pool: &PgPool,
    ids: &[ProductListingId],
) -> shipping_service::assessment::AssessmentDependencies {
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(pool.clone()).begin().await.unwrap();
    let d = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(ids, None)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    d
}
#[aura_integration_test(services=[SCHEMA])]
async fn explicit_manual_absence_is_protected_before_the_first_import() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let cases = [
        PatchField::Clear,
        PatchField::Set(ProductListingDeliveryPatch {
            selection: PatchField::Clear,
            ..Default::default()
        }),
        PatchField::Set(ProductListingDeliveryPatch {
            selection: PatchField::Set(ProfileSelection::InheritSourceDefault),
            ..Default::default()
        }),
        PatchField::Set(ProductListingDeliveryPatch {
            rate_inputs: PatchField::Clear,
            ..Default::default()
        }),
        PatchField::Set(ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Inherit),
            ..Default::default()
        }),
        PatchField::Set(ProductListingDeliveryPatch {
            collection: PatchField::Set(ComponentOverride::Inherit),
            ..Default::default()
        }),
    ];
    for (index, delivery) in cases.into_iter().enumerate() {
        let key = listing(&pool, source, &format!("manual-absence-{index}")).await;
        let command = ChangeProductListingDeliveryCommand {
            import: None,
            product_key: key.clone(),
            expected_version: ProductListingStorageVersion::INITIAL,
            delivery,
        };
        let manual = changer(&pool)
            .execute(&context(), command.clone())
            .await
            .unwrap();
        let before = events(&pool, &key).await;
        let mut imported = command;
        imported.expected_version = manual.version;
        imported.import = Some(import_metadata(1, &format!("import-{index}")));
        assert!(matches!(
            changer(&pool).execute(&context(), imported).await,
            Err(ProductListingDeliveryError::Import(
                shipping_service::ShippingError::ImportOwnershipConflict
            ))
        ));
        assert_eq!(before, events(&pool, &key).await);
    }
}

#[aura_integration_test(services=[SCHEMA])]
async fn implicit_source_selection_does_not_block_imports_of_independent_components() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let key = listing(&pool, source, "manual-prohibition").await;
    let manual = apply(
        &pool,
        &key,
        ProductListingStorageVersion::INITIAL,
        ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        },
    )
    .await;
    let mut unobserved = import_metadata(1, "unobserved-selection");
    unobserved.observation = None;
    assert!(matches!(
        changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
                    import: Some(unobserved),
                    product_key: key.clone(),
                    expected_version: manual.version,
                    delivery: PatchField::Set(ProductListingDeliveryPatch {
                        selection: PatchField::Set(ProfileSelection::InheritSourceDefault),
                        ..Default::default()
                    }),
                }
            )
            .await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::InvalidInput
        ))
    ));
    changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: Some(import_metadata(1, "selection")),
                product_key: key.clone(),
                expected_version: manual.version,
                delivery: PatchField::Set(ProductListingDeliveryPatch {
                    selection: PatchField::Set(ProfileSelection::InheritSourceDefault),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        ComponentOverride::Replace(ShippingPolicy::Prohibited),
        view(&pool, &key)
            .await
            .delivery
            .unwrap()
            .overrides()
            .shipping
    );
    let asserted = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: Some(import_metadata(2, "collection")),
                product_key: key.clone(),
                expected_version: manual.version,
                delivery: PatchField::Set(ProductListingDeliveryPatch {
                    collection: PatchField::Set(ComponentOverride::Clear),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    let mut removal = import_metadata(3, "remove-collection-override");
    removal.observation = None;
    let removed = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: Some(removal),
                product_key: key.clone(),
                expected_version: asserted.version,
                delivery: PatchField::Set(ProductListingDeliveryPatch {
                    collection: PatchField::Set(ComponentOverride::Inherit),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(ChangeOutcome::Changed, removed.outcome);
    let current = view(&pool, &key).await.delivery.unwrap();
    assert_eq!(ComponentOverride::Inherit, current.overrides().collection);
    assert_eq!(
        ComponentOverride::Replace(ShippingPolicy::Prohibited),
        current.overrides().shipping
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn incremental_imports_preserve_manual_components_and_observations_without_listing_events() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    let mut metadata = import_metadata(1, "create-profile");
    metadata.object_key = Some("main-policy".into());
    let profile = CreateShippingProfileHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        CreateProfileCommand {
            import: Some(metadata),
            source_id: source,
            creation_key: "create-main".into(),
            content: ShippingProfileContent {
                name: ShippingProfileName::new("Imported gallery".into()).unwrap(),
                attributed_party: None,
                terms: gallery_terms(8000),
            },
            evidence: vec![],
        },
    )
    .await
    .unwrap();
    AssignShippingDefaultHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        AssignDefaultCommand {
            import: Some(import_metadata(2, "default")),
            source_id: source,
            profile_id: profile.id,
            expected_revision: ShippingDefaultRevision::INITIAL,
        },
    )
    .await
    .unwrap();
    let key = listing(&pool, source, "imported-painting").await;
    let key2 = listing(&pool, source, "manual-cabinet").await;
    let manual = apply(
        &pool,
        &key2,
        ProductListingStorageVersion::INITIAL,
        ProductListingDeliveryPatch {
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            ..Default::default()
        },
    )
    .await;
    let command = ChangeProductListingDeliveryCommand {
        import: Some(import_metadata(3, "membership")),
        product_key: key.clone(),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            selection: PatchField::Set(ProfileSelection::InheritSourceDefault),
            ..Default::default()
        }),
    };
    let applied = changer(&pool)
        .execute(&context(), command.clone())
        .await
        .unwrap();
    let ids = [applied.product_listing_id, manual.product_listing_id];
    assert_eq!(
        applied.version,
        changer(&pool)
            .execute(&context(), command.clone())
            .await
            .unwrap()
            .version
    );
    let before = events(&pool, &key).await;
    let mut observed = command.clone();
    observed.expected_version = applied.version;
    observed.import = Some(import_metadata(4, "membership-observed"));
    let refreshed = changer(&pool).execute(&context(), observed).await.unwrap();
    assert_eq!(ChangeOutcome::Unchanged, refreshed.outcome);
    assert_eq!(before, events(&pool, &key).await);
    assert_eq!(applied.version, refreshed.version);
    let d = dependencies(&pool, &ids).await;
    assert!(
        d.listings
            .iter()
            .find(|l| l.id == applied.product_listing_id)
            .unwrap()
            .evidence
            .is_some()
    );
    let updated = ReplaceShippingProfileHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        ReplaceProfileCommand {
            import: Some(import_metadata(5, "price")),
            source_id: source,
            profile_id: profile.id,
            expected_revision: profile.management_revision,
            content: ShippingProfileContent {
                terms: gallery_terms(9500),
                ..profile.content.clone()
            },
            evidence: vec![],
        },
    )
    .await
    .unwrap();
    assert_eq!(
        updated.content.terms.shipping(),
        effective(&view(&pool, &key).await, source, &updated)
            .terms()
            .shipping()
    );
    assert_eq!(
        Some(&ShippingPolicy::Prohibited),
        effective(&view(&pool, &key2).await, source, &updated)
            .terms()
            .shipping()
    );
    assert_eq!(
        updated.content.terms.collection(),
        effective(&view(&pool, &key2).await, source, &updated)
            .terms()
            .collection()
    );

    // An old imported selection remains authoritative until explicitly changed or revoked.
    sqlx::query("UPDATE listing_delivery_import_state SET verified_at=now()-interval '3650 days' WHERE product_listing_id=$1 AND component='SELECTION'").bind(applied.product_listing_id.into_uuid()).execute(&pool).await.unwrap();
    let current = dependencies(&pool, &ids).await;
    let painting = current
        .listings
        .iter()
        .find(|l| l.id == applied.product_listing_id)
        .unwrap();
    assert_eq!(
        Some(&ProfileSelection::InheritSourceDefault),
        painting.delivery.selection()
    );
    let mut renewed = command.clone();
    renewed.expected_version = applied.version;
    renewed.import = Some(import_metadata(5, "membership-renewed"));
    assert_eq!(
        ChangeOutcome::Unchanged,
        changer(&pool)
            .execute(&context(), renewed)
            .await
            .unwrap()
            .outcome
    );
    // A missing catalog record is a retained retryable dependency, never an implicit create.
    let mut missing = command.clone();
    missing.product_key = ProductListingKey::new(source, "not-yet-imported".try_into().unwrap());
    missing.import = Some(import_metadata(6, "missing"));
    assert!(matches!(
        changer(&pool).execute(&context(), missing).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportDependencyMissing
        ))
    ));

    let message = ProductListingIngestionMessage {
        intent: ProductListingIngestionIntent::ChangeDelivery(command.clone()),
        metadata: ProductListingIngestionMetadata {
            submission_id: ProductListingIngestionSubmissionId::from_wire(&format!(
                "plis1_{}",
                "1".repeat(64)
            ))
            .unwrap(),
            command_id: ProductListingIngestionCommandId::from_wire(&format!(
                "plic1_{}",
                "2".repeat(64)
            ))
            .unwrap(),
            index: 0,
            input_count: 1,
            listing_source_id: source,
            operation: ProductListingIngestionOperation::ChangeDelivery,
            actor: ProductListingIngestionActor::System,
            request_id: "shipping-import-replay".into(),
            correlation_id: "shipping-import-replay".into(),
        },
    };
    let envelope = || {
        product_listing_ingestion_sqs::codec::envelope(&message)
            .unwrap()
            .into_service_envelope()
            .unwrap()
    };
    let worker = ProcessProductListingIngestionHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        product_listing_postgres::SqlxPartnerProductListingAuthorizerFactory,
        auction_postgres::SqlxAuctionReferenceValidatorFactory,
        product_listing_postgres::SqlxProductListingRawCaptureWriterFactory,
        product_listing_postgres::SqlxProductListingCommandReceiptStoreFactory,
    )
    .with_delivery_references(SqlxProductListingDeliveryReferencesFactory);
    worker.execute(envelope()).await.unwrap();
    assert_eq!(
        ProductListingIngestionCompletion::AlreadyCompleted,
        worker.execute(envelope()).await.unwrap()
    );
    let manual_equal = apply(
        &pool,
        &key,
        applied.version,
        ProductListingDeliveryPatch {
            selection: PatchField::Set(ProfileSelection::InheritSourceDefault),
            ..Default::default()
        },
    )
    .await;
    assert_eq!(ChangeOutcome::Unchanged, manual_equal.outcome);
    assert!(sqlx::query_scalar::<_,bool>("SELECT owner_hash IS NULL AND verified_at IS NOT NULL FROM listing_delivery_import_state WHERE product_listing_id=$1 AND component='SELECTION'").bind(applied.product_listing_id.into_uuid()).fetch_one(&pool).await.unwrap());
    let manual_dependencies = dependencies(&pool, &ids).await;
    assert_eq!(
        Some(&ProfileSelection::InheritSourceDefault),
        manual_dependencies
            .listings
            .iter()
            .find(|l| l.id == applied.product_listing_id)
            .unwrap()
            .delivery
            .selection()
    );

    assert_eq!(before, events(&pool, &key).await);
    // A concurrent manual selection protects the component even if the producer adopts its new revision.
    let human = apply(
        &pool,
        &key,
        applied.version,
        ProductListingDeliveryPatch {
            selection: PatchField::Set(ProfileSelection::SuppressInheritance),
            ..Default::default()
        },
    )
    .await;
    let mut stale = command;
    stale.expected_version = human.version;
    stale.import = Some(import_metadata(7, "stale"));
    assert!(matches!(
        changer(&pool).execute(&context(), stale).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportOwnershipConflict
        ))
    ));
    // Disconnect removes imported dependencies while preserving the manual prohibition.
    sqlx::query("INSERT INTO listing_source_ingestion_methods VALUES($1,'SHOPIFY')")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let d = dependencies(&pool, &ids).await;
    assert!(matches!(
        worker.execute(envelope()).await,
        Err(ProductListingIngestionError::Delivery(
            ProductListingDeliveryError::Import(
                shipping_service::ShippingError::ImportAuthorityStale
            )
        ))
    ));
    assert!(d.profiles.is_empty());
    let cabinet = d
        .listings
        .iter()
        .find(|l| l.id == manual.product_listing_id)
        .unwrap();
    assert_eq!(
        &ComponentOverride::Replace(ShippingPolicy::Prohibited),
        &cabinet.delivery.overrides().shipping
    );
    assert!(
        d.defaults
            .iter()
            .all(|d| d.selection.profile_id().is_none())
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn manual_provider_overrides_survive_disconnect_without_prior_import_observations() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    sqlx::query("INSERT INTO listing_source_ingestion_methods VALUES($1,'SHOPIFY')")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let key = listing(&pool, source, "manual-provider-cabinet").await;
    let collection = CollectionPolicy::new(
        Default::default(),
        CollectionDependency::Independent,
        Some(ShippingEligibility::Offered),
        None,
        None,
    );
    let manual = apply(
        &pool,
        &key,
        ProductListingStorageVersion::INITIAL,
        ProductListingDeliveryPatch {
            integration: DeliveryIntegration::Provider {
                method: listing_source_core::ListingIngestionMethod::Shopify,
            },
            shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
            collection: PatchField::Set(ComponentOverride::Replace(collection.clone())),
            ..Default::default()
        },
    )
    .await;
    let before = events(&pool, &key).await;
    sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let current = dependencies(&pool, &[manual.product_listing_id]).await;
    let listing = &current.listings[0];
    assert!(
        listing.integration_current,
        "manual overrides remain assessable"
    );
    assert!(listing.delivery.selection().is_none());
    assert_eq!(
        &ComponentOverride::Replace(ShippingPolicy::Prohibited),
        &listing.delivery.overrides().shipping
    );
    assert_eq!(
        &ComponentOverride::Replace(collection.clone()),
        &listing.delivery.overrides().collection
    );
    let assessed = shipping_service::assessment::assess_snapshot(
        &current,
        vec![shipping_service::assessment::AssessListingRequest {
            listing_id: manual.product_listing_id,
            input: Default::default(),
        }],
        None,
    );
    assert_eq!(
        Some(&ShippingPolicy::Prohibited),
        assessed[0].effective.terms().shipping()
    );
    assert_eq!(
        Some(&collection),
        assessed[0].effective.terms().collection()
    );
    assert_eq!(before, events(&pool, &key).await);
    assert_eq!(manual.version, view(&pool, &key).await.version);
}

#[aura_integration_test(services=[SCHEMA])]
async fn integration_replacement_requires_new_acquisition_and_explicit_owned_component_removal() {
    let pool = get_postgres_client().await;
    let source = seed_source(&pool).await;
    sqlx::query("INSERT INTO listing_source_ingestion_methods VALUES($1,'SHOPIFY')")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let scope = |method| DeliveryIntegration::Provider { method };
    let scoped_metadata = |integration, sequence, key: &str| {
        let mut metadata = import_metadata(sequence, key);
        metadata.integration = integration;
        metadata
    };
    let original = scope(listing_source_core::ListingIngestionMethod::Shopify);
    let creator = CreateShippingProfileHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingRepositoryFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    );
    let mut metadata = scoped_metadata(original.clone(), 1, "original-create");
    metadata.object_key = Some("provider-policy".into());
    let command = CreateProfileCommand {
        import: Some(metadata),
        source_id: source,
        creation_key: "original-create".into(),
        content: ShippingProfileContent {
            name: ShippingProfileName::new("Provider delivery".into()).unwrap(),
            attributed_party: None,
            terms: gallery_terms(8000),
        },
        evidence: vec![],
    };
    let old_profile = creator.execute(&context(), command.clone()).await.unwrap();
    let key = listing(&pool, source, "existing-provider-item").await;
    let assignment = ChangeProductListingDeliveryCommand {
        import: Some(scoped_metadata(original.clone(), 2, "original-assignment")),
        product_key: key.clone(),
        expected_version: ProductListingStorageVersion::INITIAL,
        delivery: PatchField::Set(ProductListingDeliveryPatch {
            integration: original.clone(),
            selection: PatchField::Set(ProfileSelection::Explicit(old_profile.id)),
            ..Default::default()
        }),
    };
    let assigned = changer(&pool)
        .execute(&context(), assignment.clone())
        .await
        .unwrap();
    sqlx::query("DELETE FROM listing_source_ingestion_methods WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO listing_source_ingestion_methods VALUES($1,'WOOCOMMERCE')")
        .bind(source.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        changer(&pool).execute(&context(), assignment).await,
        Err(ProductListingDeliveryError::Import(
            shipping_service::ShippingError::ImportAuthorityStale
        ))
    ));
    let replacement = scope(listing_source_core::ListingIngestionMethod::Woocommerce);
    let mut removal = scoped_metadata(replacement.clone(), 1, "remove-old-membership");
    removal.observation = None;
    let cleared = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: Some(removal),
                product_key: key.clone(),
                expected_version: assigned.version,
                delivery: PatchField::Clear,
            },
        )
        .await
        .unwrap();
    let d = dependencies(&pool, &[assigned.product_listing_id]).await;
    assert!(d.listings[0].delivery.selection().is_none());
    let mut current = command;
    current.creation_key = "replacement-create".into();
    let mut metadata = scoped_metadata(replacement.clone(), 2, "replacement-create");
    metadata.object_key = Some("provider-policy".into());
    current.import = Some(metadata);
    let new_profile = creator.execute(&context(), current).await.unwrap();
    assert_ne!(old_profile.id, new_profile.id);
    let recovered = shipping_service::use_cases::ListShippingProfilesHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        shipping_postgres::SqlxShippingReaderFactory,
        listing_source_postgres::SqlxSourceLocationAccessFactory,
    )
    .execute(
        &context(),
        source,
        None,
        21,
        Some(shipping_service::imports::ProfileImportIdentity {
            object_key: "provider-policy".into(),
            integration: replacement.clone(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        vec![new_profile.id],
        recovered.into_iter().map(|p| p.id).collect::<Vec<_>>()
    );
    let applied = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                import: Some(scoped_metadata(
                    replacement.clone(),
                    3,
                    "replacement-assignment",
                )),
                product_key: key.clone(),
                expected_version: cleared.version,
                delivery: PatchField::Set(ProductListingDeliveryPatch {
                    integration: replacement,
                    selection: PatchField::Set(ProfileSelection::Explicit(new_profile.id)),
                    ..Default::default()
                }),
            },
        )
        .await
        .unwrap();
    assert_eq!(assigned.product_listing_id, applied.product_listing_id);
    let d = dependencies(&pool, &[applied.product_listing_id]).await;
    assert!(d.listings[0].integration_current);
    assert_eq!(new_profile.id, d.profiles[0].profile.id());
}
