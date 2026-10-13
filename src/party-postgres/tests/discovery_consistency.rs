use application::{
    pagination::Cursor,
    transaction::{Transaction, UnitOfWork},
};
use geo::{AddressText, CountryCode, GeographicDescription};
use geo_postgres::SqlxResolutionRepositoryFactory;
use geo_service::{geocoding::*, resolution::*};
use party_core::{party_id::PartyId, party_location_id::PartyLocationId};
use party_postgres::discovery::SqlxPartyLocationDiscoveryReaderFactory;
use party_service::discovery::*;
use platform_postgres::SqlxUnitOfWork;
use std::time::Duration;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn discovery_rechecks_a_revoked_candidate_without_losing_page_continuation() {
    let pool = get_postgres_client().await;
    let owner = PartyId::new();
    sqlx::query("INSERT INTO parties(party_id,party_slug_id,name) VALUES($1,'discovery-consistency-test','Discovery consistency test')")
        .bind(owner.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let derived_id = PartyLocationId::new();
    let next_id = PartyLocationId::new();
    let last_id = PartyLocationId::new();
    assert!(derived_id < next_id && next_id < last_id);
    for (id, geography) in [
        (derived_id, None),
        (next_id, Some(asserted_country())),
        (last_id, Some(asserted_country())),
    ] {
        sqlx::query("INSERT INTO party_locations(party_location_id,party_id,label,geography,lifecycle,created_actor,updated_actor) VALUES($1,$2,'Public site',$3,'ACTIVE','SERVICE:test','SERVICE:test')")
            .bind(id.into_uuid())
            .bind(owner.into_uuid())
            .bind(geography)
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE geo_resolution_provider_controls SET is_default=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO geo_resolution_provider_controls(profile,is_default,enabled,storage_permitted) VALUES('TEST_DISCOVERY_CONSISTENCY',true,true,true)")
        .execute(&pool)
        .await
        .unwrap();
    let input = ResolutionInput::new(
        GeocodingRequest::new(AddressText::new("Synthetic private location").unwrap()),
        ResolutionScope::new(ScopeKind::Party, Some(owner.into_uuid())).unwrap(),
        RetentionPurpose::PartyLocation,
    )
    .unwrap();
    let binding = ResolutionBinding {
        subject: ResolutionSubject::new(RetentionPurpose::PartyLocation, derived_id.into_uuid())
            .unwrap(),
        input_revision: 1,
        guard: 1,
    };
    let uow = SqlxUnitOfWork::new(pool.clone());
    let mut tx = uow.begin().await.unwrap();
    SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .bind(&input, binding)
        .await
        .unwrap();
    let view = SqlxResolutionRepositoryFactory
        .resolution_reader(&mut tx)
        .inspect(binding.subject)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = uow.begin().await.unwrap();
    let ClaimOutcome::Claimed(claim) = SqlxResolutionRepositoryFactory
        .resolutions(&mut tx)
        .claim(view.work, "TEST_DISCOVERY_CONSISTENCY")
        .await
        .unwrap()
    else {
        panic!("expected local resolution claim")
    };
    tx.commit().await.unwrap();
    let candidate = GeocodingCandidate::new(
        geo::DerivedGeography::new(
            GeographicDescription::new(None, Some(CountryCode::DEU), None, None).unwrap(),
            None,
        ),
        vec![],
        None,
        GeocodingProvenance {
            provider: "TEST",
            product: "TEST",
            data_source: "TEST",
            region: "TEST".into(),
            interpretation_profile: "TEST_DISCOVERY_CONSISTENCY",
            external_reference: "synthetic provider record".into(),
            attribution_url: "https://example.test/attribution",
        },
    );
    let evidence = evaluate(
        &input,
        GeocodingResult {
            outcome: GeocodingOutcome::Candidates(
                CandidateSet::new(vec![candidate], input.request().candidate_limit()).unwrap(),
            ),
            retention: ResultRetention::Storage,
        },
        "TEST_DISCOVERY_CONSISTENCY",
    )
    .unwrap();
    let mut tx = uow.begin().await.unwrap();
    assert!(
        SqlxResolutionRepositoryFactory
            .resolutions(&mut tx)
            .complete(&claim, ResolutionCompletion::Evidence(evidence))
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    let request = DiscoverPartyLocationsRequest {
        geography: geo::core::discovery::GeographicQuery::new(
            geo::reference::ReferenceRelease::CURRENT,
            std::collections::HashSet::from([CountryCode::DEU]),
            Default::default(),
            None,
            None,
            Default::default(),
            None,
        )
        .unwrap(),
        cursor: Cursor {
            size: 1,
            search_after: None,
        },
    };
    let mut revocation = pool.begin().await.unwrap();
    sqlx::query("UPDATE geo_resolution_provider_controls SET storage_permitted=false WHERE profile='TEST_DISCOVERY_CONSISTENCY'")
        .execute(&mut *revocation)
        .await
        .unwrap();
    let page_request = request.clone();
    let discovery_pool = pool.clone();
    let discovery = tokio::spawn(async move {
        DiscoverPartyLocationsHandler::new(
            SqlxUnitOfWork::new(discovery_pool),
            SqlxPartyLocationDiscoveryReaderFactory,
        )
        .execute(page_request)
        .await
    });
    // The first SQL page sees permitted evidence. Pause the later evidence read
    // specifically at its provider lock, then commit a legitimate revocation.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query LIKE '%geo_resolution_provider_controls%FOR SHARE%')")
                .fetch_one(&pool)
                .await
                .unwrap();
            if waiting {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("discovery must reach the evidence-read lock after selecting its page");
    revocation.commit().await.unwrap();
    let result = discovery.await.unwrap();
    let first = result.expect("a concurrent provider revocation is not corrupt persisted state");
    assert_eq!(1, first.items.len());
    assert_eq!(next_id, first.items[0].location_id);
    assert_eq!(
        Some(next_id),
        first.cursor.search_after.map(|c| c.location_id)
    );

    let handler = DiscoverPartyLocationsHandler::new(uow, SqlxPartyLocationDiscoveryReaderFactory);
    let second = handler
        .execute(DiscoverPartyLocationsRequest {
            cursor: first.cursor,
            ..request.clone()
        })
        .await
        .unwrap();
    assert_eq!(1, second.items.len());
    assert_eq!(last_id, second.items[0].location_id);
    assert!(second.cursor.search_after.is_none());

    // A stable malformed row still fails closed; retries must not discard it.
    sqlx::query(
        "UPDATE party_locations SET label='  Noncanonical label  ' WHERE party_location_id=$1",
    )
    .bind(next_id.into_uuid())
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        handler.execute(request).await,
        Err(DiscoverPartyLocationsError::InvalidState(_))
    ));
}

fn asserted_country() -> serde_json::Value {
    serde_json::json!({
        "reference_release": geo::reference::ReferenceRelease::CURRENT.as_str(),
        "address_text": null,
        "country": "DE",
        "subdivision": null,
        "postal_code": null
    })
}
