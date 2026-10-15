use application::transaction::{Transaction, UnitOfWork};
use platform_postgres::SqlxUnitOfWork;
use product_listing_core::product_listing_id::ProductListingId;
use shipping_core::fixtures::gallery_terms;
use shipping_postgres::assessment::SqlxAssessmentReaderFactory;
use shipping_service::assessment::*;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use time::OffsetDateTime;

mod support;
use support::*;
#[aura_integration_test(services=[SCHEMA])]
async fn shared_current_dependencies_and_verification_are_independent_of_unrelated_writes() {
    let p = get_postgres_client().await;
    let (_, profile, ids) = seed(&p).await;
    let before = read(&p, &ids).await;
    assert_eq!(1, before.profiles.len());
    assert_eq!(1, before.defaults.len());
    assert_eq!(2, before.listings.len());
    let verified = before.profiles[0].evidence.verified_at;
    sqlx::query("UPDATE shipping_profiles SET management_revision=2,name='New label',updated=now() WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&p).await.unwrap();
    assert_eq!(
        verified,
        read(&p, &ids).await.profiles[0].evidence.verified_at
    );
    sqlx::query("UPDATE shipping_profiles SET management_revision=3,terms_revision=2,terms=$2 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(shipping_json::encode::<References>(&gallery_terms(9500)).unwrap()).execute(&p).await.unwrap();
    let after = read(&p, &ids).await;
    assert_eq!(
        gallery_terms(9500),
        after.profiles[0].profile.content().terms
    );
    assert!(after.profiles[0].evidence.verified_at > verified);
    let old: OffsetDateTime = sqlx::query_scalar(
        "SELECT delivery_verified_at FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(ids[1].into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    sqlx::query("UPDATE product_listings SET title_text='Unrelated edit',title_language='en',updated=now(),version=version+1 WHERE product_listing_id=$1").bind(ids[1].into_uuid()).execute(&p).await.unwrap();
    let now: OffsetDateTime = sqlx::query_scalar(
        "SELECT delivery_verified_at FROM product_listings WHERE product_listing_id=$1",
    )
    .bind(ids[1].into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!(old, now);
}
#[aura_integration_test(services=[SCHEMA])]
async fn reads_share_repeatable_snapshot_and_new_transactions_see_retirement() {
    let p = get_postgres_client().await;
    let (_, profile, ids) = seed(&p).await;
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let deps = SqlxAssessmentReaderFactory
        .in_transaction(&mut tx)
        .snapshot(&ids, None)
        .await
        .unwrap();
    sqlx::query("UPDATE shipping_profiles SET lifecycle='RETIRED',management_revision=2,terms_revision=2 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).execute(&p).await.unwrap();
    let lifecycle: String =
        sqlx::query_scalar("SELECT lifecycle FROM shipping_profiles WHERE shipping_profile_id=$1")
            .bind(profile.into_uuid())
            .fetch_one(tx.connection())
            .await
            .unwrap();
    assert_eq!("ACTIVE", lifecycle);
    assert_eq!("ACTIVE", deps.profiles[0].profile.lifecycle().as_str());
    tx.commit().await.unwrap();
    assert_eq!(
        "RETIRED",
        read(&p, &ids).await.profiles[0]
            .profile
            .lifecycle()
            .as_str()
    );
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    let absent = ProductListingId::new();
    assert!(
        SqlxAssessmentReaderFactory
            .in_transaction(&mut tx)
            .snapshot(&[absent], None)
            .await
            .unwrap()
            .listings
            .is_empty()
    );
}
#[aura_integration_test(services=[SCHEMA])]
async fn invalid_delivery_codes_fail_closed_and_withdrawn_is_unavailable() {
    let p = get_postgres_client().await;
    let (_, _, ids) = seed(&p).await;
    sqlx::query("UPDATE product_listings SET delivery_configuration=jsonb_set(delivery_configuration,'{schema}','\"FUTURE\"') WHERE product_listing_id=$1").bind(ids[1].into_uuid()).execute(&p).await.unwrap();
    let mut tx = SqlxUnitOfWork::new(p.clone()).begin().await.unwrap();
    assert!(matches!(
        SqlxAssessmentReaderFactory
            .in_transaction(&mut tx)
            .snapshot(&ids, None)
            .await,
        Err(AssessmentError::InvalidState(_))
    ));
    drop(tx);
    sqlx::query("UPDATE product_listings SET lifecycle='WITHDRAWN' WHERE product_listing_id=$1")
        .bind(ids[0].into_uuid())
        .execute(&p)
        .await
        .unwrap();
    let mut tx = SqlxUnitOfWork::new(p).begin().await.unwrap();
    assert!(
        SqlxAssessmentReaderFactory
            .in_transaction(&mut tx)
            .snapshot(&ids[..1], None)
            .await
            .unwrap()
            .listings
            .is_empty()
    );
}

#[aura_integration_test(services=[SCHEMA])]
async fn evidence_requires_a_current_listing_assertion_and_ordered_deadlines() {
    let p = get_postgres_client().await;
    let (_, _, ids) = seed(&p).await;
    for query in [
        "UPDATE product_listings SET delivery_verified_at=now() WHERE product_listing_id=$1",
        "UPDATE product_listings SET delivery_fresh_until=now() WHERE product_listing_id=$1",
        "UPDATE product_listings SET delivery_expires_at=now() WHERE product_listing_id=$1",
    ] {
        let error = sqlx::query(query)
            .bind(ids[0].into_uuid())
            .execute(&p)
            .await
            .unwrap_err();
        assert_eq!(
            Some("listing_delivery_evidence_window"),
            error.as_database_error().unwrap().constraint()
        );
    }
    for query in [
        "UPDATE product_listings SET delivery_verified_at=NULL WHERE product_listing_id=$1",
        "UPDATE product_listings SET delivery_fresh_until=delivery_verified_at-interval '1 second' WHERE product_listing_id=$1",
        "UPDATE product_listings SET delivery_expires_at=delivery_verified_at-interval '1 second' WHERE product_listing_id=$1",
        "UPDATE product_listings SET delivery_fresh_until=delivery_verified_at+interval '1 hour',delivery_expires_at=delivery_verified_at WHERE product_listing_id=$1",
    ] {
        let error = sqlx::query(query)
            .bind(ids[1].into_uuid())
            .execute(&p)
            .await
            .unwrap_err();
        assert_eq!(
            Some("listing_delivery_evidence_window"),
            error.as_database_error().unwrap().constraint()
        );
    }
}

#[aura_integration_test(services=[SCHEMA])]
async fn dispatch_uses_only_current_permitted_derived_country_without_projection_or_provider_io() {
    use geo_service::{geocoding::GeocodingRequest, resolution::*};
    use shipping_core::{coverage::*, terms::*};
    let p = get_postgres_client().await;
    let (source, profile, ids) = seed(&p).await;
    let owner: uuid::Uuid = sqlx::query_scalar(
        "SELECT operator_party_id FROM listing_sources WHERE listing_source_id=$1",
    )
    .bind(source.into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    let site = party_core::party_location_id::PartyLocationId::new();
    sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,lifecycle,created_actor,updated_actor)VALUES($1,$2,'Dispatch',$3,'ACTIVE','test','test')").bind(site.into_uuid()).bind(owner).bind(serde_json::json!({"reference_release":"iso-codes-4.20.1","address_text":"Synthetic unresolved dispatch site","country":null,"subdivision":null,"postal_code":null})).execute(&p).await.unwrap();
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&p)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default,enabled,storage_permitted,daily_limit,scope_daily_limit,concurrency_limit,scope_concurrency_limit)VALUES('SHIPPING_ORIGIN_TEST',true,true,true,100,20,2,1)").execute(&p).await.unwrap();
    let input = ResolutionInput::new(
        GeocodingRequest::new(geo::AddressText::new("Synthetic unresolved dispatch site").unwrap()),
        ResolutionScope::new(ScopeKind::Party, Some(owner)).unwrap(),
        RetentionPurpose::PartyLocation,
    )
    .unwrap();
    let binding = ResolutionBinding {
        subject: ResolutionSubject::new(RetentionPurpose::PartyLocation, site.into_uuid()).unwrap(),
        input_revision: 1,
        guard: 1,
    };
    let uow = SqlxUnitOfWork::new(p.clone());
    let mut tx = uow.begin().await.unwrap();
    geo_postgres::SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .bind(&input, binding)
        .await
        .unwrap();
    let work = geo_postgres::SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(binding.subject)
        .await
        .unwrap()
        .unwrap()
        .work;
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = geo_postgres::SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(work, "SHIPPING_ORIGIN_TEST")
        .await
        .unwrap()
    else {
        panic!("claim required")
    };
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let mut evidence = ResolutionEvidence {
        status: ResolutionStatus::Accepted,
        accepted_index: Some(0),
        candidates: vec![ResolutionCandidate {
            geography: geo::DerivedGeography::new(
                geo::GeographicDescription::new(None, Some(geo::CountryCode::DEU), None, None)
                    .unwrap(),
                None,
            )
            .unwrap(),
            acceptable: true,
            provider: "TEST".into(),
            product: "TEST".into(),
            data_source: "TEST".into(),
            region: "TEST".into(),
            profile: "SHIPPING_ORIGIN_TEST".into(),
            external_reference: "synthetic-site".into(),
            attribution_url: "https://example.test/attribution".into(),
        }],
    };
    evidence.candidates.push(evidence.candidates[0].clone());
    evidence.accepted_index = Some(1);
    assert!(
        geo_postgres::SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(&claim, ResolutionCompletion::Evidence(evidence))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let base = gallery_terms(8000);
    let coverage = match base.shipping().unwrap() {
        ShippingPolicy::Coverage(c) => c,
        _ => unreachable!(),
    };
    let zone = ShippingZone::new(
        vec![DeliveryArea::Domestic],
        Default::default(),
        coverage.zones()[0].services().to_vec(),
    )
    .unwrap();
    let terms = DeliveryTerms::new(
        Some(ShippingPolicy::coverage(
            ShippingCoverage::new(
                coverage.release(),
                coverage.completeness(),
                Default::default(),
                vec![],
                vec![zone],
                Some(DeliverySite::PartyLocation(site)),
            )
            .unwrap(),
        )),
        None,
    );
    sqlx::query("UPDATE shipping_profiles SET terms=$2,management_revision=2,terms_revision=2 WHERE shipping_profile_id=$1").bind(profile.into_uuid()).bind(shipping_json::encode::<References>(&terms).unwrap()).execute(&p).await.unwrap();
    let deps = read(&p, &ids).await;
    assert_eq!(1, deps.locations.len());
    assert_eq!(
        Some(geo::CountryCode::DEU),
        deps.locations[0]
            .location
            .content()
            .geography
            .as_ref()
            .unwrap()
            .country()
    );
    assert_eq!(
        Some(work.generation),
        deps.locations[0].resolution_generation
    );
    async fn project(pool: &sqlx::PgPool) {
        use shipping_service::projection::{
            ProjectListingShippingHandler, ProjectListingShippingUseCase,
        };
        for _ in 0..50 {
            ProjectListingShippingHandler::new(
                SqlxUnitOfWork::new(pool.clone()),
                shipping_postgres::projection::SqlxShippingProjectionStoreFactory,
            )
            .execute(None, 100)
            .await
            .unwrap();
            let pending: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM listing_shipping_work WHERE pending)",
            )
            .fetch_one(pool)
            .await
            .unwrap();
            if !pending {
                return;
            }
        }
        panic!("shipping did not converge");
    }
    project(&p).await;
    let summary: serde_json::Value = sqlx::query_scalar(
        "SELECT summary FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert_eq!("DE", summary["country_offers"][0]["country"]);
    let refs:i64=sqlx::query_scalar("SELECT count(*) FROM product_listing_shipping_dependencies WHERE product_listing_id=$1 AND dependency_kind='RESOLUTION'").bind(ids[0].into_uuid()).fetch_one(&p).await.unwrap();
    assert_eq!(1, refs);
    sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=false WHERE profile='SHIPPING_ORIGIN_TEST'").execute(&p).await.unwrap();
    let deps = read(&p, &ids).await;
    assert!(deps.locations[0].location.content().geography.is_none());
    assert!(deps.locations[0].resolution_generation.is_none());
    project(&p).await;
    let summary: serde_json::Value = sqlx::query_scalar(
        "SELECT summary FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert!(summary["country_offers"].as_array().unwrap().is_empty());
    assert_eq!(true, summary["conditional"]);
    let basis: serde_json::Value = sqlx::query_scalar(
        "SELECT dependency FROM product_listing_shipping WHERE product_listing_id=$1",
    )
    .bind(ids[0].into_uuid())
    .fetch_one(&p)
    .await
    .unwrap();
    assert!(
        !basis
            .to_string()
            .contains("Synthetic unresolved dispatch site")
    );
}
