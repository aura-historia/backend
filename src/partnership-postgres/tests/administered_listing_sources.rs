use application::operation_context::{
    CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
};
use listing_source_core::ListingSourceId;
use partnership_core::partnership_id::PartnershipId;
use partnership_postgres::SqlxListingSourceAuthorization;
use partnership_service::use_cases::queries::list_administered_listing_sources::*;
use party_core::party_id::PartyId;
use sqlx::PgPool;
use std::collections::BTreeSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;
const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

fn context(user_id: UserId) -> OperationContext {
    OperationContext {
        principal: Principal::DelegatedUser {
            user_id,
            capabilities: BTreeSet::from([CredentialCapability::ListingSourcesRead]),
        },
        request_id: RequestId::new("sources"),
        correlation_id: CorrelationId::new("sources"),
    }
}
async fn member(pool: &PgPool, user: UserId) -> (PartyId, PartnershipId) {
    let party = PartyId::new();
    let partnership = PartnershipId::new();
    sqlx::query("INSERT INTO parties (party_id, party_slug_id, name) VALUES ($1, $2, 'Operator')")
        .bind(party.into_uuid())
        .bind(format!("party-{}", party.as_uuid()))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnerships (partnership_id, party_id) VALUES ($1, $2)")
        .bind(partnership.into_uuid())
        .bind(party.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO partnership_members (user_id, partnership_id) VALUES ($1, $2)")
        .bind(user.into_uuid())
        .bind(partnership.into_uuid())
        .execute(pool)
        .await
        .unwrap();
    (party, partnership)
}
async fn user(pool: &PgPool) -> UserId {
    let user = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user.into_uuid())
        .bind(format!("{user}@example.test"))
        .execute(pool)
        .await
        .unwrap();
    user
}
async fn grant(pool: &PgPool, partnership: PartnershipId, source: ListingSourceId) {
    sqlx::query("INSERT INTO partnership_listing_source_grants (partnership_id, listing_source_id) VALUES ($1, $2)").bind(partnership.into_uuid()).bind(source.into_uuid()).execute(pool).await.unwrap();
}
async fn source(pool: &PgPool, party: PartyId, name: &str) -> ListingSourceId {
    let source = ListingSourceId::new();
    sqlx::query("INSERT INTO listing_sources (listing_source_id, listing_source_slug_id, name, operator_party_id) VALUES ($1, $2, $3, $4)").bind(source.into_uuid()).bind(format!("source-{}", source.as_uuid())).bind(name).bind(party.into_uuid()).execute(pool).await.unwrap();
    source
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_page_distinct_sources_with_repeated_names_and_overlapping_memberships() {
    let pool = get_postgres_client().await;
    let user = user(&pool).await;
    let (party_a, partnership_a) = member(&pool, user).await;
    let (party_b, partnership_b) = member(&pool, user).await;
    let mut expected = vec![];
    for (party, partnership, name) in [
        (party_a, partnership_a, "A"),
        (party_b, partnership_b, "Z"),
        (party_a, partnership_a, "Äntiques"),
        (party_a, partnership_a, "Same"),
        (party_a, partnership_a, "Same"),
        (party_b, partnership_b, "Same"),
        (party_b, partnership_b, "Same"),
    ] {
        let id = source(&pool, party, name).await;
        grant(&pool, partnership, id).await;
        // Overlapping memberships/grants cannot duplicate an eligible source or disqualify it.
        let other = if partnership == partnership_a {
            partnership_b
        } else {
            partnership_a
        };
        grant(&pool, other, id).await;
        expected.push((name.to_owned(), id));
    }
    let mismatched = source(&pool, party_a, "Not eligible").await;
    grant(&pool, partnership_b, mismatched).await;
    expected.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_uuid().cmp(b.1.as_uuid())));
    let expected: Vec<_> = expected.into_iter().map(|(_, id)| id).collect();
    let handler = ListAdministeredListingSourcesHandler::new(SqlxListingSourceAuthorization::new(
        pool.clone(),
    ));
    for size in [1, 2, 3, 100] {
        let mut after = None;
        let mut found = vec![];
        loop {
            let page = handler
                .execute(
                    &context(user),
                    ListAdministeredListingSourcesRequest { size, after },
                )
                .await
                .unwrap();
            assert!(page.items.len() <= size as usize);
            found.extend(
                page.items
                    .into_iter()
                    .map(|source| source.listing_source_id),
            );
            assert!(found.len() <= expected.len());
            after = page.next;
            if after.is_none() {
                break;
            }
        }
        assert_eq!(expected, found, "page size {size}");
    }
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_recheck_current_source_grants_membership_and_delegation_on_continuation() {
    let pool = get_postgres_client().await;
    let user = user(&pool).await;
    let (party, partnership) = member(&pool, user).await;
    let first = source(&pool, party, "A").await;
    grant(&pool, partnership, first).await;
    let revoked = source(&pool, party, "B").await;
    grant(&pool, partnership, revoked).await;
    let last = source(&pool, party, "C").await;
    grant(&pool, partnership, last).await;
    let handler = ListAdministeredListingSourcesHandler::new(SqlxListingSourceAuthorization::new(
        pool.clone(),
    ));
    let page = handler
        .execute(
            &context(user),
            ListAdministeredListingSourcesRequest {
                size: 1,
                after: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first, page.items[0].listing_source_id);
    let after = page.next.unwrap();
    sqlx::query("DELETE FROM partnership_listing_source_grants WHERE listing_source_id = $1")
        .bind(revoked.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let request = ListAdministeredListingSourcesRequest {
        size: 1,
        after: Some(after.clone()),
    };
    let remaining = handler
        .execute(&context(user), request.clone())
        .await
        .unwrap();
    assert_eq!(
        vec![last],
        remaining
            .items
            .iter()
            .map(|source| source.listing_source_id)
            .collect::<Vec<_>>()
    );
    assert!(remaining.next.is_none());
    let mut unscoped = context(user);
    unscoped.principal = Principal::DelegatedUser {
        user_id: user,
        capabilities: BTreeSet::new(),
    };
    assert!(matches!(
        handler.execute(&unscoped, request.clone()).await,
        Err(ListAdministeredListingSourcesError::Forbidden)
    ));
    let foreign = context(UserId::new());
    assert!(matches!(
        handler.execute(&foreign, request.clone()).await,
        Err(ListAdministeredListingSourcesError::CursorScopeMismatch)
    ));
    sqlx::query("DELETE FROM partnership_members WHERE user_id = $1")
        .bind(user.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        handler
            .execute(&context(user), request)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}
