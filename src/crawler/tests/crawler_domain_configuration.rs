use crawler::service::crawler_domain_configuration::{
    CrawlerDomainConfigurationError, CrawlerDomainConfigurationRepository,
    CrawlerDomainConfigurationRepositoryImpl,
};
use crawler::service::listing_source_registration::{
    ListingSourceRegistrationRepository, ListingSourceRegistrationRepositoryImpl,
    RegisteredListingSource,
};
use crawler::spider::candidate_service::{SpiderCandidateService, SpiderCandidateServiceImpl};
use listing_source_core::{Domain, ListingSourceId, ListingSourceName, ListingSourceSlugId};
use test_api::*;

const POSTGRES: Postgres = Postgres::new("src/crawler/migrations");

fn source(id: ListingSourceId, enabled: bool) -> RegisteredListingSource {
    RegisteredListingSource {
        listing_source_id: id,
        listing_source_name: ListingSourceName::try_from("Test source").unwrap(),
        listing_source_slug: ListingSourceSlugId::raw("test-source").unwrap(),
        crawl_enabled: enabled,
    }
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_onboard_enabled_source_then_preserve_domain_while_disabled_and_reenable_it() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool.clone());
    let candidates = SpiderCandidateServiceImpl::new(pool.clone());
    let listing_source_id = ListingSourceId::new();

    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();
    assert!(candidates.get_candidates(10, &[]).await.unwrap().is_empty());

    let configured = domains
        .register(listing_source_id, Domain::try_from("example.com").unwrap())
        .await
        .unwrap();
    let repeated = domains
        .register(listing_source_id, Domain::try_from("example.com").unwrap())
        .await
        .unwrap();
    assert_eq!(configured.domain_id, repeated.domain_id);
    assert_eq!(candidates.get_candidates(10, &[]).await.unwrap().len(), 1);

    registration
        .apply_snapshot(&[source(listing_source_id, false)])
        .await
        .unwrap();
    assert!(candidates.get_candidates(10, &[]).await.unwrap().is_empty());
    assert_eq!(
        domains
            .list_for_source(listing_source_id)
            .await
            .unwrap()
            .len(),
        1
    );

    registration
        .apply_snapshot(&[source(listing_source_id, true)])
        .await
        .unwrap();
    let candidates = candidates.get_candidates(10, &[]).await.unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].domain_id, configured.domain_id);
}

#[serial_test::serial]
#[aura_integration_test(services = [POSTGRES])]
async fn should_reject_domain_claimed_by_another_listing_source() {
    let pool = get_postgres_client().await;
    let registration = ListingSourceRegistrationRepositoryImpl::new(pool.clone());
    let domains = CrawlerDomainConfigurationRepositoryImpl::new(pool);
    let owner = ListingSourceId::new();
    let claimant = ListingSourceId::new();
    registration
        .apply_snapshot(&[source(owner, true), source(claimant, true)])
        .await
        .unwrap();
    domains
        .register(owner, Domain::try_from("owned.example.com").unwrap())
        .await
        .unwrap();

    let result = domains
        .register(claimant, Domain::try_from("owned.example.com").unwrap())
        .await;
    assert!(matches!(
        result,
        Err(CrawlerDomainConfigurationError::DomainOwnedByAnotherListingSource { .. })
    ));
}
