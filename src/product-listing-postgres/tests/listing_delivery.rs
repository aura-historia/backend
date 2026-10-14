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
            source_id: source,
            profile_id: profile.id,
            expected_revision: ShippingDefaultRevision::INITIAL,
        },
    )
    .await
    .unwrap();
    let key = listing(&pool, source, "painting").await;
    let command = ChangeProductListingDeliveryCommand {
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
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations(listing_source_id,domain) VALUES($1,'synthetic.myshopify.com')").bind(source.into_uuid()).execute(&pool).await.unwrap();
    let integration = DeliveryIntegration::Provider {
        method: listing_source_core::ListingIngestionMethod::Shopify,
        domain: listing_source_core::Domain::try_from("synthetic.myshopify.com").unwrap(),
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
        method: listing_source_core::ListingIngestionMethod::Shopify,
        domain: listing_source_core::Domain::try_from("foreign.myshopify.com").unwrap(),
    };
    assert!(matches!(
        changer(&pool)
            .execute(
                &context(),
                ChangeProductListingDeliveryCommand {
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
    sqlx::query("UPDATE listing_source_shopify_ingestion_configurations SET domain='replacement.myshopify.com' WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&pool).await.unwrap();
    // Reliable removal is possible after the old integration identity becomes ineligible.
    let removal = changer(&pool)
        .execute(
            &context(),
            ChangeProductListingDeliveryCommand {
                product_key: key.clone(),
                expected_version: pending.version,
                delivery: PatchField::Clear,
            },
        )
        .await
        .unwrap();
    assert_eq!(ChangeOutcome::Unchanged, removal.outcome);
    let new_integration = DeliveryIntegration::Provider {
        method: listing_source_core::ListingIngestionMethod::Shopify,
        domain: listing_source_core::Domain::try_from("replacement.myshopify.com").unwrap(),
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
