use application::{
    operation_context::{OperationContext, Principal},
    patch_field::PatchField,
    transaction::{Transaction, UnitOfWork},
};
use listing_source_core::ListingSourceId;
use listing_source_postgres::SqlxSourceLocationAccessFactory;
use party_core::party_id::PartyId;
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::{
    delivery::DeliveryIntegration,
    product_listing::{NewProductListing, ProductListing, ProductListingPricing},
    product_listing_id::{ProductListingId, ProductListingKey},
    product_listing_slug_id::ProductListingSlugId,
};
use product_listing_postgres::{
    SqlxProductListingEventAppenderFactory, SqlxProductListingRepositoryFactory,
    delivery::SqlxProductListingDeliveryReferencesFactory,
};
use product_listing_service::{
    ports::{
        ProductListingEventAppender, ProductListingEventAppenderFactory, ProductListingRepository,
        ProductListingRepositoryFactory, stamp_product_listing_event,
    },
    use_cases::*,
};
use shipping_core::{
    fixtures::gallery_terms,
    inputs::ListingRateInputs,
    profile::*,
    selection::{ComponentOverride, DeliveryOverrides},
    terms::ShippingPolicy,
};
use shipping_postgres::SqlxShippingRepositoryFactory;
use shipping_service::ports::*;
use shipping_sync_json::{PublicReferences, decode_chunk, encode_chunk, manifest};
use shipping_sync_postgres::SqlxShippingSyncStoreFactory;
use shipping_sync_service::{canonical::CanonicalShippingSync, use_cases::*, *};
use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const SCHEMA: Postgres = Postgres::new("migrations");
fn context() -> OperationContext {
    OperationContext {
        principal: Principal::System,
        request_id: "synthetic-sync".into(),
        correlation_id: "synthetic-sync".into(),
    }
}
fn service(pool: &PgPool) -> impl ShippingSyncUseCase {
    ShippingSyncHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxShippingSyncStoreFactory,
        SqlxSourceLocationAccessFactory,
        CanonicalShippingSync::new(
            SqlxShippingSyncStoreFactory,
            SqlxShippingRepositoryFactory,
            SqlxProductListingRepositoryFactory,
            SqlxProductListingEventAppenderFactory,
            SqlxProductListingDeliveryReferencesFactory,
        ),
    )
}
async fn source(pool: &PgPool) -> ListingSourceId {
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
async fn listing(pool: &PgPool, source: ListingSourceId, code: &str) -> ProductListingId {
    let id = ProductListingId::new();
    let mut listing = ProductListing::create(NewProductListing {
        id,
        title_slug_id: ProductListingSlugId::raw(&format!("{code}-a1b2c3")).unwrap(),
        listing_source_id: source,
        source_listing_id: code.try_into().unwrap(),
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
        OffsetDateTime::now_utc(),
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
    id
}
fn key(v: &str) -> ExternalKey {
    ExternalKey::new(v.into()).unwrap()
}
fn window() -> ObservationWindow {
    let now = OffsetDateTime::now_utc();
    ObservationWindow {
        observed_at: now,
        fresh_until: now + Duration::hours(1),
        expires_at: now + Duration::hours(2),
    }
}
fn profile(amount: u64) -> Record {
    Record {
        objects: vec![],
        key: key("gallery"),
        window: window(),
        assertion: Assertion::Profile {
            content: Box::new(ShippingProfileContent {
                name: ShippingProfileName::new("Gallery terms".into()).unwrap(),
                attributed_party: None,
                terms: gallery_terms(amount),
            }),
            evidence: vec![],
        },
    }
}
fn default() -> Record {
    Record {
        objects: vec![],
        key: key("default"),
        window: window(),
        assertion: Assertion::Default {
            profile: Some(key("gallery")),
        },
    }
}
fn item(code: &str, cabinet: bool) -> Record {
    Record {
        objects: vec![],
        key: key(code),
        window: window(),
        assertion: Assertion::Item {
            source_listing_id: code.try_into().unwrap(),
            assertion: Box::new(ItemAssertion {
                selection: EvidenceSelection::InheritSourceDefault,
                rate_inputs: ListingRateInputs::default(),
                overrides: DeliveryOverrides {
                    shipping: if cabinet {
                        ComponentOverride::Replace(ShippingPolicy::Prohibited)
                    } else {
                        ComponentOverride::Inherit
                    },
                    collection: ComponentOverride::Inherit,
                },
            }),
        },
    }
}
async fn binding(s: &impl ShippingSyncUseCase, source: ListingSourceId) -> Binding {
    match s
        .execute(
            &context(),
            source,
            SyncCommand::Rotate {
                integration: DeliveryIntegration::Source,
                expected_revision: 0,
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Binding(b) => b,
        _ => panic!("binding"),
    }
}
async fn head(s: &impl ShippingSyncUseCase, b: &Binding, scope: &str) -> ScopeHead {
    match s
        .execute(
            &context(),
            b.source,
            SyncCommand::Head {
                epoch: b.epoch,
                scope: key(scope),
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Head(h) => h,
        _ => panic!("head"),
    }
}
async fn begin(s: &impl ShippingSyncUseCase, b: &Binding, scope: &str, mode: Mode) -> Receipt {
    begin_components(s, b, scope, mode, [true; 3]).await
}
async fn begin_components(
    s: &impl ShippingSyncUseCase,
    b: &Binding,
    scope: &str,
    mode: Mode,
    components: [bool; 3],
) -> Receipt {
    let h = head(s, b, scope).await;
    match s
        .execute(
            &context(),
            b.source,
            SyncCommand::Begin(Begin {
                epoch: b.epoch,
                key: key(&Uuid::now_v7().to_string()),
                scope: key(scope),
                mode,
                expected_generation: h.generation,
                expected_sequence: h.sequence,
                provider_version: key("website-v1"),
                components,
            }),
        )
        .await
        .unwrap()
    {
        SyncResult::Receipt(r) => *r,
        _ => panic!("receipt"),
    }
}
fn chunk(source: ListingSourceId, records: Vec<Record>) -> Chunk {
    decode_chunk::<PublicReferences>(encode_chunk::<PublicReferences>(&records).unwrap(), source)
        .unwrap()
}
async fn upload(
    s: &impl ShippingSyncUseCase,
    r: &Receipt,
    index: u32,
    c: Chunk,
) -> Result<SyncResult, SyncError> {
    s.execute(
        &context(),
        r.source,
        SyncCommand::Capture {
            id: r.id,
            index,
            chunk: c,
        },
    )
    .await
}
async fn finish(s: &impl ShippingSyncUseCase, r: &Receipt, chunks: &[Chunk]) -> Receipt {
    match s
        .execute(
            &context(),
            r.source,
            SyncCommand::Finalize {
                id: r.id,
                manifest: manifest(chunks),
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Receipt(r) => *r,
        _ => panic!("receipt"),
    }
}
async fn apply(
    s: &impl ShippingSyncUseCase,
    b: &Binding,
    scope: &str,
    records: impl FnOnce() -> Vec<Record>,
) -> Receipt {
    let r = begin(s, b, scope, Mode::Snapshot).await;
    let c = chunk(b.source, records());
    upload(s, &r, 0, c.clone()).await.unwrap();
    finish(s, &r, &[c]).await
}
async fn profiles(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM shipping_profiles")
        .fetch_one(pool)
        .await
        .unwrap()
}
#[aura_integration_test(services=[SCHEMA])]
async fn gallery_snapshot_applies_once_and_profile_refresh_is_immediate_on_authoritative_reads() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let painting = listing(&p, source, "painting").await;
    listing(&p, source, "cabinet").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let first = apply(&s, &b, "configuration", || {
        vec![
            profile(8000),
            default(),
            item("painting", false),
            item("cabinet", true),
        ]
    })
    .await;
    assert_eq!(ApplicationState::Applied, first.state);
    assert_eq!(1, profiles(&p).await);
    let before: (i64, Option<i64>) = sqlx::query_as(
        "SELECT version,price_amount FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(painting.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    let second = apply(&s, &b, "configuration", || {
        vec![
            profile(9500),
            default(),
            item("painting", false),
            item("cabinet", true),
        ]
    })
    .await;
    assert_eq!(ApplicationState::Applied, second.state);
    assert_eq!(1, profiles(&p).await);
    let after: (i64, Option<i64>) = sqlx::query_as(
        "SELECT version,price_amount FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(painting.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!(before, after);
    let amount:serde_json::Value=sqlx::query_scalar("SELECT terms #> '{shipping,coverage,zones,0,services,0,rate,price,amount,amount_minor}' FROM shipping_profiles").fetch_one(&p).await.unwrap();
    assert_eq!(serde_json::json!(9500), amount);
    let reader = shipping_postgres::assessment::SqlxAssessmentReaderFactory;
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = reader
        .in_transaction(&mut tx)
        .snapshot(&[painting], None)
        .await
        .unwrap();
    assert_eq!(
        gallery_terms(9500),
        deps.profiles[0].profile.content().terms
    );
    tx.commit().await.unwrap();
}

#[aura_integration_test(services=[SCHEMA])]
async fn immutable_out_of_order_chunks_incomplete_snapshots_and_retries() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || vec![profile(8000), default()]).await;
    let r = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let a = chunk(source, vec![profile(9500)]);
    let z = chunk(source, vec![default()]);
    upload(&s, &r, 1, z.clone()).await.unwrap();
    assert!(matches!(
        s.execute(
            &context(),
            source,
            SyncCommand::Finalize {
                id: r.id,
                manifest: manifest(&[a.clone(), z.clone()])
            }
        )
        .await,
        Err(SyncError::Incomplete)
    ));
    let current = s
        .execute(&context(), source, SyncCommand::Status { id: r.id })
        .await
        .unwrap();
    let SyncResult::Receipt(current) = current else {
        panic!("receipt")
    };
    assert!(!current.raw_complete);
    assert!(current.manifest.is_some());
    assert!(matches!(
        s.execute(
            &context(),
            source,
            SyncCommand::Finalize {
                id: r.id,
                manifest: manifest(std::slice::from_ref(&z))
            }
        )
        .await,
        Err(SyncError::Conflict)
    ));
    let active: String = sqlx::query_scalar("SELECT lifecycle FROM shipping_profiles")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!("ACTIVE", active);
    upload(&s, &r, 0, a.clone()).await.unwrap();
    upload(&s, &r, 0, a.clone()).await.unwrap();
    assert!(matches!(
        upload(&s, &r, 0, chunk(source, vec![profile(7000)])).await,
        Err(SyncError::Conflict)
    ));
    assert_eq!(
        ApplicationState::Applied,
        finish(&s, &r, &[a.clone(), z.clone()]).await.state
    );
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM shipping_changes")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(
        ApplicationState::Applied,
        finish(&s, &r, &[a, z]).await.state
    );
    assert_eq!(
        signals,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM shipping_changes")
            .fetch_one(&p)
            .await
            .unwrap()
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn missing_listing_retains_raw_custody_and_rolls_back_all_canonical_effects() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let r = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let c = chunk(source, vec![profile(8000), default(), item("later", false)]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    let waiting = finish(&s, &r, std::slice::from_ref(&c)).await;
    assert_eq!(ApplicationState::WaitingDependency, waiting.state);
    assert_eq!(Some("LISTING_MISSING"), waiting.failure.as_deref());
    assert!(waiting.manifest.is_some());
    assert_eq!(0, profiles(&p).await);
    listing(&p, source, "later").await;
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    assert_eq!(1, profiles(&p).await);
}

#[aura_integration_test(services=[SCHEMA])]
async fn live_delta_fences_full_fetch_and_failed_extraction_does_not_retract() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || vec![profile(8000), default()]).await;
    let old = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let delta = begin(&s, &b, "configuration", Mode::Delta).await;
    let d = chunk(source, vec![profile(9500)]);
    upload(&s, &delta, 0, d.clone()).await.unwrap();
    assert_eq!(
        ApplicationState::Applied,
        finish(&s, &delta, &[d]).await.state
    );
    let empty = chunk(source, vec![]);
    upload(&s, &old, 0, empty.clone()).await.unwrap();
    assert_eq!(
        ApplicationState::ReconcileRequired,
        finish(&s, &old, &[empty]).await.state
    );
    assert!(head(&s, &b, "configuration").await.dirty);
    assert_eq!(1, profiles(&p).await);
    let failed = apply(&s, &b, "configuration", || {
        vec![Record {
            objects: vec![],
            key: key("capability"),
            window: window(),
            assertion: Assertion::Unavailable {
                capability: Capability::Configuration,
                reason: UnavailableReason::PermissionDenied,
            },
        }]
    })
    .await;
    assert_eq!(ApplicationState::ReconcileRequired, failed.state);
    let lifecycle: String = sqlx::query_scalar("SELECT lifecycle FROM shipping_profiles")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!("ACTIVE", lifecycle);
}

#[aura_integration_test(services=[SCHEMA])]
async fn equal_refresh_evidence_correction_and_unavailable_delta_preserve_freshness() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || vec![profile(8000)]).await;
    let original: (OffsetDateTime, OffsetDateTime, OffsetDateTime) =
        sqlx::query_as("SELECT verified_at,fresh_until,expires_at FROM shipping_profiles")
            .fetch_one(&p)
            .await
            .unwrap();
    apply(&s, &b, "configuration", || vec![profile(8000)]).await;
    apply(&s, &b, "configuration", || {
        let mut r = profile(8000);
        if let Assertion::Profile { evidence, .. } = &mut r.assertion {
            evidence.push(
                ShippingEvidence::new("https://example.test/corrected".into(), "EXPLICIT".into())
                    .unwrap(),
            );
        }
        vec![r]
    })
    .await;
    let after: (OffsetDateTime, OffsetDateTime, OffsetDateTime) =
        sqlx::query_as("SELECT verified_at,fresh_until,expires_at FROM shipping_profiles")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(original, after);

    // A reliable restoration changes applicability and acquires a new assertion window.
    apply(&s, &b, "configuration", Vec::new).await;
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "configuration", || vec![profile(8000)])
            .await
            .state
    );
    let restored: (String, OffsetDateTime) =
        sqlx::query_as("SELECT lifecycle,verified_at FROM shipping_profiles")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!("ACTIVE", restored.0);
    assert!(restored.1 > original.0);
}

#[aura_integration_test(services=[SCHEMA])]
async fn partner_override_survives_refresh_and_epoch_revocation() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let id = listing(&p, source, "painting").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || {
        vec![profile(8000), default(), item("painting", false)]
    })
    .await;
    let key = ProductListingKey::new(source, "painting".try_into().unwrap());
    let version: i64 =
        sqlx::query_scalar("SELECT version FROM product_listings WHERE product_listing_id=$1")
            .bind(id.into_uuid())
            .fetch_one(&p)
            .await
            .unwrap();
    ChangeProductListingDeliveryHandler::new(
        SqlxUnitOfWork::new(p.clone()),
        SqlxProductListingRepositoryFactory,
        SqlxProductListingEventAppenderFactory,
        SqlxProductListingDeliveryReferencesFactory,
    )
    .execute(
        &context(),
        ChangeProductListingDeliveryCommand {
            product_key: key,
            expected_version: version.try_into().unwrap(),
            delivery: PatchField::Set(ProductListingDeliveryPatch {
                shipping: PatchField::Set(ComponentOverride::Replace(ShippingPolicy::Prohibited)),
                ..Default::default()
            }),
        },
    )
    .await
    .unwrap();
    apply(&s, &b, "configuration", || {
        vec![profile(9500), default(), item("painting", false)]
    })
    .await;
    let value: serde_json::Value = sqlx::query_scalar(
        "SELECT delivery_configuration FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(id.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!("PROHIBITED", value["shipping"]["value"]["kind"]);
    s.execute(&context(), source, SyncCommand::Revoke { epoch: b.epoch })
        .await
        .unwrap();
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.profiles.is_empty());
    assert!(deps.listings[0].delivery.selection().is_none());
    assert_eq!(
        &ComponentOverride::Replace(ShippingPolicy::Prohibited),
        &deps.listings[0].delivery.overrides().shipping
    );
    tx.commit().await.unwrap();
    assert!(matches!(
        s.execute(
            &context(),
            source,
            SyncCommand::Status { id: Uuid::now_v7() }
        )
        .await,
        Err(SyncError::NotFound)
    ));
}

#[aura_integration_test(services=[SCHEMA])]
async fn missing_profile_can_be_repaired_without_acknowledging_the_pending_membership() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    listing(&p, source, "painting").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let r = begin(&s, &b, "items", Mode::Snapshot).await;
    let mut i = item("painting", false);
    if let Assertion::Item { assertion, .. } = &mut i.assertion {
        assertion.selection = EvidenceSelection::Profile(key("gallery"));
    }
    let c = chunk(source, vec![i]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    assert_eq!(
        Some("PROFILE_MISSING"),
        finish(&s, &r, std::slice::from_ref(&c))
            .await
            .failure
            .as_deref()
    );
    apply(&s, &b, "configuration", || vec![profile(8000)]).await;
    assert_eq!(
        ApplicationState::ReconcileRequired,
        finish(&s, &r, &[c]).await.state
    );
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "items", || {
            let mut i = item("painting", false);
            if let Assertion::Item { assertion, .. } = &mut i.assertion {
                assertion.selection = EvidenceSelection::Profile(key("gallery"));
            }
            vec![i]
        })
        .await
        .state
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn snapshot_components_and_tombstones_never_clear_unowned_configuration() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    listing(&p, source, "cabinet").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || {
        vec![profile(8000), default(), item("cabinet", true)]
    })
    .await;
    let h = head(&s, &b, "configuration").await;
    let r = match s
        .execute(
            &context(),
            source,
            SyncCommand::Begin(Begin {
                epoch: b.epoch,
                key: key("membership-only-snapshot"),
                scope: key("configuration"),
                mode: Mode::Snapshot,
                expected_generation: h.generation,
                expected_sequence: h.sequence,
                provider_version: key("website-v1"),
                components: [false, false, true],
            }),
        )
        .await
        .unwrap()
    {
        SyncResult::Receipt(r) => *r,
        _ => panic!("receipt"),
    };
    let c = chunk(source, vec![]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    let state:(String,Option<Uuid>)=sqlx::query_as("SELECT p.lifecycle,d.shipping_profile_id FROM shipping_profiles p JOIN shipping_source_defaults d USING(listing_source_id)").fetch_one(&p).await.unwrap();
    assert_eq!("ACTIVE", state.0);
    assert!(state.1.is_some());
    let value: serde_json::Value =
        sqlx::query_scalar("SELECT delivery_configuration FROM product_listings")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(serde_json::Value::Null, value["selection"]);
    assert_eq!("INHERIT", value["shipping"]["kind"]);
}

#[aura_integration_test(services=[SCHEMA])]
async fn epoch_rotation_revocation_source_deletion_and_expiry_fence_old_work() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let r = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let c = chunk(source, vec![profile(8000)]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    let next = match s
        .execute(
            &context(),
            source,
            SyncCommand::Rotate {
                integration: DeliveryIntegration::Source,
                expected_revision: 1,
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Binding(b) => b,
        _ => panic!("binding"),
    };
    assert_ne!(b.epoch, next.epoch);
    assert!(matches!(
        s.execute(
            &context(),
            source,
            SyncCommand::Finalize {
                id: r.id,
                manifest: manifest(&[c])
            }
        )
        .await,
        Err(SyncError::Revoked)
    ));
    assert_eq!(0, profiles(&p).await);
    let fresh = begin(&s, &next, "configuration", Mode::Snapshot).await;
    let c = chunk(source, vec![profile(9500)]);
    upload(&s, &fresh, 0, c.clone()).await.unwrap();
    sqlx::query("UPDATE shipping_sync_sessions SET expires_at=clock_timestamp()-interval '1 second' WHERE session_id=$1").bind(fresh.id).execute(&p).await.unwrap();
    assert_eq!(
        ApplicationState::Expired,
        finish(&s, &fresh, &[c]).await.state
    );
    assert_eq!(0, profiles(&p).await);
    sqlx::query("DELETE FROM listing_sources WHERE listing_source_id=$1")
        .bind(source.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    assert!(
        s.execute(&context(), source, SyncCommand::Status { id: fresh.id })
            .await
            .is_err()
    );
    let retained: i64 = sqlx::query_scalar("SELECT count(*) FROM shipping_sync_sessions")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(2, retained);
}

#[aura_integration_test(services=[SCHEMA])]
async fn concurrent_finalizers_apply_one_generation_and_leave_no_partial_state() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let first = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let second = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let a = chunk(source, vec![profile(8000)]);
    let z = chunk(source, vec![profile(9500)]);
    upload(&s, &first, 0, a.clone()).await.unwrap();
    upload(&s, &second, 0, z.clone()).await.unwrap();
    let left = [a];
    let right = [z];
    let (x, y) = tokio::join!(finish(&s, &first, &left), finish(&s, &second, &right));
    let states = [x.state, y.state];
    assert_eq!(
        1,
        states
            .iter()
            .filter(|s| **s == ApplicationState::Applied)
            .count()
    );
    assert_eq!(
        1,
        states
            .iter()
            .filter(|s| **s == ApplicationState::ReconcileRequired)
            .count()
    );
    assert_eq!(1, profiles(&p).await);
}

#[aura_integration_test(services=[SCHEMA])]
async fn bounded_retention_erases_content_and_preserves_replay_fences() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let r = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let c = chunk(source, vec![profile(8000)]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    finish(&s, &r, std::slice::from_ref(&c)).await;
    sqlx::query("UPDATE shipping_sync_sessions SET started_at=clock_timestamp()-interval '31 days' WHERE session_id=$1").bind(r.id).execute(&p).await.unwrap();
    let cleaned = s
        .execute(&context(), source, SyncCommand::Cleanup { limit: 1 })
        .await
        .unwrap();
    assert!(matches!(cleaned, SyncResult::Cleaned { sessions: 1 }));
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT payload IS NULL AND erased_at IS NOT NULL FROM shipping_sync_chunks"
        )
        .fetch_one(&p)
        .await
        .unwrap()
    );
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    assert_eq!(1, profiles(&p).await);
}

#[derive(Clone)]
struct Forward<T>(T);
struct AmbiguousTx {
    inner: platform_postgres::SqlxTransaction,
    commits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
struct AmbiguousUow {
    inner: SqlxUnitOfWork,
    commits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[async_trait::async_trait]
impl UnitOfWork for AmbiguousUow {
    type Tx = AmbiguousTx;
    async fn begin(&self) -> Result<Self::Tx, application::transaction::TransactionError> {
        Ok(AmbiguousTx {
            inner: self.inner.begin().await?,
            commits: self.commits.clone(),
        })
    }
}
#[async_trait::async_trait]
impl Transaction for AmbiguousTx {
    async fn commit(self) -> Result<(), application::transaction::TransactionError> {
        self.inner.commit().await?;
        use std::sync::atomic::Ordering;
        let remaining = self.commits.load(Ordering::SeqCst);
        if remaining > 0 && self.commits.fetch_sub(1, Ordering::SeqCst) == 1 {
            return Err(application::transaction::TransactionError::CommitFailed(
                application::error::static_error("synthetic lost commit acknowledgment"),
            ));
        }
        Ok(())
    }
}
impl<T: shipping_sync_service::ports::SyncStoreFactory<platform_postgres::SqlxTransaction>>
    shipping_sync_service::ports::SyncStoreFactory<AmbiguousTx> for Forward<T>
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut AmbiguousTx,
    ) -> impl shipping_sync_service::ports::SyncStore + 'a {
        self.0.in_transaction(&mut tx.inner)
    }
}
impl<
    T: listing_source_service::ports::location_default::SourceLocationAccessFactory<
            platform_postgres::SqlxTransaction,
        >,
> listing_source_service::ports::location_default::SourceLocationAccessFactory<AmbiguousTx>
    for Forward<T>
{
    fn in_transaction<'a>(
        &'a self,
        tx: &'a mut AmbiguousTx,
    ) -> impl listing_source_service::ports::location_default::SourceLocationAccess + 'a {
        self.0.in_transaction(&mut tx.inner)
    }
}
#[async_trait::async_trait]
impl<T: shipping_sync_service::ports::SyncApplication<platform_postgres::SqlxTransaction>>
    shipping_sync_service::ports::SyncApplication<AmbiguousTx> for Forward<T>
{
    async fn apply(
        &self,
        tx: &mut AmbiguousTx,
        binding: &Binding,
        receipt: &Receipt,
        records: &[Record],
    ) -> Result<bool, SyncError> {
        self.0.apply(&mut tx.inner, binding, receipt, records).await
    }
}
#[aura_integration_test(services=[SCHEMA])]
async fn lost_canonical_commit_acknowledgment_replays_the_durable_receipt() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let commits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let s = ShippingSyncHandler::new(
        AmbiguousUow {
            inner: SqlxUnitOfWork::new(p.clone()),
            commits: commits.clone(),
        },
        Forward(SqlxShippingSyncStoreFactory),
        Forward(SqlxSourceLocationAccessFactory),
        Forward(CanonicalShippingSync::new(
            SqlxShippingSyncStoreFactory,
            SqlxShippingRepositoryFactory,
            SqlxProductListingRepositoryFactory,
            SqlxProductListingEventAppenderFactory,
            SqlxProductListingDeliveryReferencesFactory,
        )),
    );
    let b = binding(&s, source).await;
    let r = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let c = chunk(source, vec![profile(8000)]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    commits.store(2, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        s.execute(
            &context(),
            source,
            SyncCommand::Finalize {
                id: r.id,
                manifest: manifest(std::slice::from_ref(&c))
            }
        )
        .await,
        Err(SyncError::Transaction(
            application::transaction::TransactionError::CommitFailed(_)
        ))
    ));
    assert_eq!(1, profiles(&p).await);
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    let signals: i64 = sqlx::query_scalar("SELECT count(*) FROM shipping_changes")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(1, signals);
    assert_eq!(
        1,
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM shipping_sync_changes")
            .fetch_one(&p)
            .await
            .unwrap()
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn configured_provider_epochs_are_bound_and_replacement_keeps_listing_identity() {
    use listing_source_core::{Domain, ListingIngestionMethod};
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let id = listing(&p, source, "painting").await;
    let s = service(&p);
    sqlx::query("INSERT INTO listing_source_ingestion_methods(listing_source_id,ingestion_method) VALUES($1,'SHOPIFY')").bind(source.into_uuid()).execute(&p).await.unwrap();
    // Use actual configuration constraints and synthetic shop identity only.
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations(listing_source_id,domain,currency) VALUES($1,'synthetic.myshopify.com','EUR')").bind(source.into_uuid()).execute(&p).await.unwrap();
    let integration = DeliveryIntegration::Provider {
        method: ListingIngestionMethod::Shopify,
        domain: Domain::try_from("synthetic.myshopify.com").unwrap(),
    };
    let b = match s
        .execute(
            &context(),
            source,
            SyncCommand::Rotate {
                integration: integration.clone(),
                expected_revision: 0,
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Binding(b) => b,
        _ => panic!("binding"),
    };
    let apply_records = || {
        let mut i = item("painting", false);
        if let Assertion::Item { assertion, .. } = &mut i.assertion {
            assertion.selection = EvidenceSelection::Profile(key("gallery"));
        }
        vec![profile(8000), i]
    };
    apply(&s, &b, "configuration", apply_records).await;
    let b = match s
        .execute(
            &context(),
            source,
            SyncCommand::Rotate {
                integration,
                expected_revision: 1,
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Binding(b) => b,
        _ => panic!("binding"),
    };
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "configuration", apply_records).await.state
    );
    let current: Uuid = sqlx::query_scalar("SELECT product_listing_id FROM product_listings")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(id.into_uuid(), current);
    sqlx::query("UPDATE listing_source_shopify_ingestion_configurations SET domain='replacement.myshopify.com' WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&p).await.unwrap();
    assert!(matches!(
        s.execute(
            &context(),
            source,
            SyncCommand::Head {
                epoch: b.epoch,
                scope: key("configuration")
            }
        )
        .await,
        Err(SyncError::Revoked)
    ));
    let replacement = DeliveryIntegration::Provider {
        method: ListingIngestionMethod::Shopify,
        domain: Domain::try_from("replacement.myshopify.com").unwrap(),
    };
    let b = match s
        .execute(
            &context(),
            source,
            SyncCommand::Rotate {
                integration: replacement,
                expected_revision: 0,
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Binding(b) => b,
        _ => panic!("binding"),
    };
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "configuration", apply_records).await.state
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn configuration_changes_fence_membership_in_other_scopes_and_schedule_reconciliation() {
    use listing_source_core::{Domain, ListingIngestionMethod};
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let id = listing(&p, source, "painting").await;
    let s = service(&p);
    sqlx::query("INSERT INTO listing_source_ingestion_methods(listing_source_id,ingestion_method) VALUES($1,'SHOPIFY')").bind(source.into_uuid()).execute(&p).await.unwrap();
    sqlx::query("INSERT INTO listing_source_shopify_ingestion_configurations(listing_source_id,domain,currency) VALUES($1,'scoped.myshopify.com','EUR')").bind(source.into_uuid()).execute(&p).await.unwrap();
    let b = match s
        .execute(
            &context(),
            source,
            SyncCommand::Rotate {
                integration: DeliveryIntegration::Provider {
                    method: ListingIngestionMethod::Shopify,
                    domain: Domain::try_from("scoped.myshopify.com").unwrap(),
                },
                expected_revision: 0,
            },
        )
        .await
        .unwrap()
    {
        SyncResult::Binding(b) => b,
        _ => panic!("binding"),
    };
    apply(&s, &b, "configuration", || vec![profile(8000)]).await;
    let h = head(&s, &b, "items").await;
    let r = match s
        .execute(
            &context(),
            source,
            SyncCommand::Begin(Begin {
                epoch: b.epoch,
                key: key("item-fetch"),
                scope: key("items"),
                mode: Mode::Snapshot,
                expected_generation: h.generation,
                expected_sequence: h.sequence,
                provider_version: key("2026-10"),
                components: [false, false, true],
            }),
        )
        .await
        .unwrap()
    {
        SyncResult::Receipt(r) => *r,
        _ => panic!("receipt"),
    };
    let mut i = item("painting", false);
    if let Assertion::Item { assertion, .. } = &mut i.assertion {
        assertion.selection = EvidenceSelection::Profile(key("gallery"));
    }
    let c = chunk(source, vec![i]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    // Equal configuration and failed acquisitions preserve already reliable assertions.
    apply(&s, &b, "configuration", || vec![profile(8000)]).await;
    assert!(!head(&s, &b, "items").await.dirty);
    let failed = begin(&s, &b, "items", Mode::Delta).await;
    let mut candidate = item("painting", false);
    if let Assertion::Item { assertion, .. } = &mut candidate.assertion {
        assertion.selection = EvidenceSelection::Unavailable;
    }
    let failed_chunk = chunk(source, vec![candidate]);
    upload(&s, &failed, 0, failed_chunk.clone()).await.unwrap();
    let failed_receipt = finish(&s, &failed, &[failed_chunk]).await;
    assert_eq!(ApplicationState::Applied, failed_receipt.state);
    assert!(!head(&s, &b, "items").await.dirty);
    use shipping_sync_service::ports::{SyncStore, SyncStoreFactory};
    let mut capture_tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let claim = SqlxShippingSyncStoreFactory
        .in_transaction(&mut capture_tx)
        .claim(&b, ObjectKind::Item, &key("painting"))
        .await
        .unwrap()
        .unwrap();
    let AppliedValue::Item { delivery, .. } = claim.value else {
        panic!("item")
    };
    assert!(matches!(
        delivery.selection(),
        Some(shipping_core::selection::ProfileSelection::Explicit(_))
    ));
    capture_tx.commit().await.unwrap();
    apply(&s, &b, "configuration", || vec![profile(9500)]).await;
    assert!(head(&s, &b, "items").await.dirty);
    let pending = s
        .execute(
            &context(),
            source,
            SyncCommand::Pending {
                epoch: b.epoch,
                after: None,
                limit: 32,
            },
        )
        .await
        .unwrap();
    let SyncResult::Pending(ids) = pending else {
        panic!("pending")
    };
    assert!(ids.contains(&failed_receipt.id));
    let status = s
        .execute(&context(), source, SyncCommand::Status { id: r.id })
        .await
        .unwrap();
    let SyncResult::Receipt(status) = status else {
        panic!("receipt")
    };
    assert!(status.reconciliation_required);
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.listings[0].delivery.selection().is_none());
    assert!(deps.profiles.is_empty());
    tx.commit().await.unwrap();

    // A full fetch for another component does not certify the retained membership.
    let partial = begin_components(&s, &b, "items", Mode::Snapshot, [true, false, false]).await;
    let c = chunk(source, vec![]);
    upload(&s, &partial, 0, c.clone()).await.unwrap();
    assert_eq!(
        ApplicationState::Applied,
        finish(&s, &partial, &[c]).await.state
    );
    assert!(head(&s, &b, "items").await.dirty);
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.listings[0].delivery.selection().is_none());
    tx.commit().await.unwrap();

    let complete = begin_components(&s, &b, "items", Mode::Snapshot, [false, false, true]).await;
    let mut i = item("painting", false);
    if let Assertion::Item { assertion, .. } = &mut i.assertion {
        assertion.selection = EvidenceSelection::Profile(key("gallery"));
    }
    let c = chunk(source, vec![i]);
    upload(&s, &complete, 0, c.clone()).await.unwrap();
    assert_eq!(
        ApplicationState::Applied,
        finish(&s, &complete, &[c]).await.state
    );
    assert!(!head(&s, &b, "items").await.dirty);
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.listings[0].delivery.selection().is_some());
    tx.commit().await.unwrap();
}

#[aura_integration_test(services=[SCHEMA])]
async fn component_age_caps_cache_and_expired_override_does_not_fall_back_to_an_offer() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let id = listing(&p, source, "cabinet").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || {
        vec![profile(8000), default(), item("cabinet", true)]
    })
    .await;
    sqlx::query("UPDATE product_listings SET delivery_shipping_observed_at=clock_timestamp()-interval '2 minutes',delivery_shipping_fresh_until=clock_timestamp()-interval '30 seconds',delivery_shipping_expires_at=clock_timestamp()+interval '1 minute' WHERE product_listing_id=$1").bind(id.into_uuid()).execute(&p).await.unwrap();
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    let evidence = deps.listings[0].evidence.unwrap();
    assert!(evidence.fresh_until.unwrap() < deps.evaluated_at);
    assert!(deps.listings[0].cache_until.unwrap() < deps.evaluated_at + Duration::minutes(2));
    tx.commit().await.unwrap();
    sqlx::query("UPDATE product_listings SET delivery_shipping_expires_at=clock_timestamp()-interval '1 second' WHERE product_listing_id=$1").bind(id.into_uuid()).execute(&p).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert_eq!(
        &ComponentOverride::Clear,
        &deps.listings[0].delivery.overrides().shipping
    );
    assert!(deps.listings[0].evidence.unwrap().expires_at.is_none());
    tx.commit().await.unwrap();
}

#[aura_integration_test(services=[SCHEMA])]
async fn a_known_empty_default_can_later_accept_its_sources_reliable_assignment() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || {
        vec![Record {
            key: key("default"),
            window: window(),
            objects: vec![],
            assertion: Assertion::Default { profile: None },
        }]
    })
    .await;
    apply(&s, &b, "configuration", || vec![profile(8000), default()]).await;
    let assigned:bool=sqlx::query_scalar("SELECT shipping_profile_id IS NOT NULL FROM shipping_source_defaults WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&p).await.unwrap();
    assert!(assigned);
}

#[aura_integration_test(services=[SCHEMA])]
async fn default_assignment_expiry_is_independent_and_caps_implicit_inheritance_cache() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let id = listing(&p, source, "painting").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || vec![profile(8000), default()]).await;
    sqlx::query("UPDATE shipping_source_defaults SET sync_fresh_until=clock_timestamp()+interval '30 seconds',sync_expires_at=clock_timestamp()+interval '1 minute' WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&p).await.unwrap();
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.defaults[0].selection.profile_id().is_some());
    assert!(deps.defaults[0].cache_until.unwrap() < deps.evaluated_at + Duration::minutes(2));
    tx.commit().await.unwrap();
    sqlx::query("UPDATE shipping_source_defaults SET sync_observed_at=clock_timestamp()-interval '2 minutes',sync_fresh_until=clock_timestamp()-interval '30 seconds',sync_expires_at=clock_timestamp()-interval '1 second' WHERE listing_source_id=$1").bind(source.into_uuid()).execute(&p).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.defaults[0].selection.profile_id().is_none());
    assert!(deps.profiles.is_empty());
    tx.commit().await.unwrap();
    assert!(sqlx::query_scalar::<_,bool>("SELECT shipping_profile_id IS NOT NULL FROM shipping_source_defaults WHERE listing_source_id=$1").bind(source.into_uuid()).fetch_one(&p).await.unwrap());
}

#[aura_integration_test(services=[SCHEMA])]
async fn tombstones_on_withdrawn_items_complete_without_restoring_or_reviving_old_shipping() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let id = listing(&p, source, "cabinet").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    apply(&s, &b, "configuration", || {
        vec![profile(8000), default(), item("cabinet", true)]
    })
    .await;
    sqlx::query("UPDATE product_listings SET lifecycle='WITHDRAWN' WHERE product_listing_id=$1")
        .bind(id.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let r = begin(&s, &b, "configuration", Mode::Delta).await;
    let c = chunk(
        source,
        vec![Record {
            key: key("cabinet"),
            window: window(),
            objects: vec![],
            assertion: Assertion::Delete {
                kind: ObjectKind::Item,
            },
        }],
    );
    upload(&s, &r, 0, c.clone()).await.unwrap();
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    let lifecycle: String =
        sqlx::query_scalar("SELECT lifecycle FROM product_listings WHERE product_listing_id=$1")
            .bind(id.into_uuid())
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!("WITHDRAWN", lifecycle);
    // Independent catalog restoration must not restore a tombstoned shipping assertion.
    sqlx::query("UPDATE product_listings SET lifecycle='ACTIVE' WHERE product_listing_id=$1")
        .bind(id.into_uuid())
        .execute(&p)
        .await
        .unwrap();
    use shipping_service::assessment::{AssessmentReader, AssessmentReaderFactory};
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert!(deps.listings[0].delivery.selection().is_none());
    assert_eq!(
        &ComponentOverride::Clear,
        &deps.listings[0].delivery.overrides().shipping
    );
    tx.commit().await.unwrap();

    let r = begin(&s, &b, "configuration", Mode::Delta).await;
    let mut unavailable = item("cabinet", true);
    if let Assertion::Item { assertion, .. } = &mut unavailable.assertion {
        assertion.selection = EvidenceSelection::Unavailable;
    }
    let c = chunk(source, vec![unavailable]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    assert_eq!(ApplicationState::Applied, finish(&s, &r, &[c]).await.state);
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT tombstoned FROM shipping_sync_claims WHERE epoch=$1 AND object_kind='ITEM' AND external_key='cabinet'"
    ).bind(b.epoch).fetch_one(&p).await.unwrap());

    // A subsequent reliable acquisition may reclaim only its retained ownership.
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "configuration", || {
            vec![profile(8000), default(), item("cabinet", true)]
        })
        .await
        .state
    );
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = shipping_postgres::assessment::SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&[id], None)
        .await
        .unwrap();
    assert_eq!(
        &ComponentOverride::Replace(ShippingPolicy::Prohibited),
        &deps.listings[0].delivery.overrides().shipping
    );
    tx.commit().await.unwrap();
}

#[aura_integration_test(services=[SCHEMA])]
async fn first_ownership_of_implicit_values_has_expiry_and_equal_refresh_preserves_it() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    listing(&p, source, "painting").await;
    let s = service(&p);
    let b = binding(&s, source).await;
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "items", || vec![item("painting", false)])
            .await
            .state
    );
    let read_windows = || {
        sqlx::query_as::<_, (Option<OffsetDateTime>, Option<OffsetDateTime>, Option<OffsetDateTime>, Option<OffsetDateTime>)>(
        "SELECT delivery_selection_expires_at,delivery_inputs_expires_at,delivery_shipping_expires_at,delivery_collection_expires_at FROM product_listings WHERE listing_source_id=$1"
    ).bind(source.into_uuid())
    };
    let before = read_windows().fetch_one(&p).await.unwrap();
    assert!(before.0.is_some() && before.1.is_some() && before.2.is_some() && before.3.is_some());
    assert_eq!(
        ApplicationState::Applied,
        apply(&s, &b, "items", || vec![item("painting", false)])
            .await
            .state
    );
    assert_eq!(before, read_windows().fetch_one(&p).await.unwrap());
}

#[aura_integration_test(services=[SCHEMA])]
async fn matching_chunk_custody_replays_after_observation_expiry_without_reacquisition() {
    let p = get_postgres_client().await;
    let source = source(&p).await;
    let s = service(&p);
    let b = binding(&s, source).await;
    let r = begin(&s, &b, "configuration", Mode::Snapshot).await;
    let mut record = profile(8000);
    record.window.fresh_until = record.window.observed_at;
    record.window.expires_at = record.window.observed_at + Duration::seconds(1);
    let c = chunk(source, vec![record]);
    upload(&s, &r, 0, c.clone()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let SyncResult::Receipt(replayed) = upload(&s, &r, 0, c.clone()).await.unwrap() else {
        panic!("receipt")
    };
    assert_eq!(1, replayed.captured_chunks);
    assert_eq!(vec![(0, c.digest)], replayed.chunk_digests);
    let applied = finish(&s, &r, &[c]).await;
    assert_eq!(ApplicationState::WaitingDependency, applied.state);
    assert_eq!(
        Some("EVIDENCE_EXPIRED_OR_PREDATES_FETCH"),
        applied.failure.as_deref()
    );
    assert_eq!(0, profiles(&p).await);
}
