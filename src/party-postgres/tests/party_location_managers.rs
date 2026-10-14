use application::{
    operation_context::{
        CorrelationId, CredentialCapability, OperationContext, Principal, RequestId,
    },
    pagination::Cursor,
};
use partnership_core::partnership_id::PartnershipId;
use party_core::party_id::PartyId;
use party_postgres::{SqlxPartyLocationAccessFactory, SqlxPartyLocationManagerReaderFactory};
use party_service::use_cases::{
    party_locations::{
        GrantPartyLocationManagementCommand, GrantPartyLocationManagementHandler,
        GrantPartyLocationManagementUseCase, PartyLocationError,
    },
    queries::list_party_location_managers::*,
};
use platform_postgres::SqlxUnitOfWork;
use sqlx::PgPool;
use test_api::{IntegrationTestService, aura_integration_test, get_postgres_client};
use user_core::user_id::UserId;
use user_service::use_cases::queries::check_user_admin::{
    CheckUserAdminError, CheckUserAdminRequest, CheckUserAdminResult, CheckUserAdminUseCase,
};
use uuid::Uuid;

const BUSINESS_SCHEMA: test_api::Postgres = test_api::Postgres::new("migrations");

// The location policy must still revalidate the persisted administrator under the owner lock.
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

fn context(principal: Principal) -> OperationContext {
    OperationContext {
        command_key: None,
        principal,
        request_id: RequestId::new("location-managers-test"),
        correlation_id: CorrelationId::new("location-managers-test"),
    }
}

fn request(party_id: PartyId) -> ListPartyLocationManagersRequest {
    ListPartyLocationManagersRequest {
        party_id,
        user_id: None,
        access: None,
        cursor: Cursor::default(),
    }
}

async fn party(pool: &PgPool) -> PartyId {
    let id = PartyId::new();
    sqlx::query(
        "INSERT INTO parties(party_id,party_slug_id,name) VALUES ($1,$2,'Location manager test')",
    )
    .bind(id.into_uuid())
    .bind(format!("location-manager-{}", id.as_uuid().simple()))
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn user(pool: &PgPool, sequence: u128, role: &str) -> UserId {
    let id = UserId::try_from(Uuid::from_u128(
        0x0000_0000_0000_7000_8000_0000_0000_0000 | sequence,
    ))
    .unwrap();
    sqlx::query("INSERT INTO users(user_id,email,first_name,last_name,tier,role) VALUES ($1,$2,'Ada','Lovelace','FREE',$3)")
        .bind(id.into_uuid()).bind(format!("manager-{sequence}@example.test")).bind(role)
        .execute(pool).await.unwrap();
    id
}

async fn partnership(
    pool: &PgPool,
    party_id: PartyId,
    state: &str,
    members: &[UserId],
) -> PartnershipId {
    let id = PartnershipId::new();
    sqlx::query(
        "INSERT INTO partnerships(partnership_id,party_id,business_state) VALUES ($1,$2,$3)",
    )
    .bind(id.into_uuid())
    .bind(party_id.into_uuid())
    .bind(state)
    .execute(pool)
    .await
    .unwrap();
    for member in members {
        sqlx::query("INSERT INTO partnership_members(user_id,partnership_id) VALUES ($1,$2)")
            .bind(member.into_uuid())
            .bind(id.into_uuid())
            .execute(pool)
            .await
            .unwrap();
    }
    id
}

async fn grant(pool: &PgPool, party_id: PartyId, user_id: UserId, granted: bool) {
    GrantPartyLocationManagementHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationAccessFactory,
        Admin,
    )
    .execute(
        &context(Principal::System),
        GrantPartyLocationManagementCommand {
            party_id,
            user_id,
            granted,
        },
    )
    .await
    .unwrap();
}

fn handler(pool: &PgPool) -> impl ListPartyLocationManagersUseCase {
    ListPartyLocationManagersHandler::new(
        SqlxUnitOfWork::new(pool.clone()),
        SqlxPartyLocationManagerReaderFactory,
        SqlxPartyLocationAccessFactory,
        Admin,
    )
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn lists_current_independent_reasons_filters_pages_and_grant_changes() {
    let pool = get_postgres_client().await;
    let party_id = party(&pool).await;
    let other_party = party(&pool).await;
    let dissolved_party = party(&pool).await;
    let explicit = user(&pool, 1, "USER").await;
    let member = user(&pool, 2, "USER").await;
    let both = user(&pool, 3, "USER").await;
    let suspended = user(&pool, 4, "USER").await;
    let deleted = user(&pool, 5, "USER").await;
    let unrelated = user(&pool, 6, "USER").await;
    let active_id = partnership(
        &pool,
        party_id,
        "ACTIVE",
        &[member, both, suspended, deleted],
    )
    .await;
    partnership(&pool, other_party, "ACTIVE", &[unrelated]).await;
    partnership(&pool, dissolved_party, "DISSOLVED", &[unrelated]).await;
    for id in [explicit, both, suspended, deleted] {
        grant(&pool, party_id, id, true).await;
    }
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(suspended.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE user_id=$1")
        .bind(deleted.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let list = handler(&pool);
    let ctx = context(Principal::System);
    let page = list.execute(&ctx, request(party_id)).await.unwrap();
    assert_eq!(
        vec![explicit, member, both],
        page.items.iter().map(|row| row.user_id).collect::<Vec<_>>()
    );
    assert_eq!(
        vec![PartyLocationManagerAccess::Explicit],
        page.items[0].access
    );
    let membership = PartyLocationManagerAccess::Partnership {
        partnership_ids: vec![active_id],
    };
    assert_eq!(vec![membership.clone()], page.items[1].access);
    assert_eq!(
        vec![PartyLocationManagerAccess::Explicit, membership.clone()],
        page.items[2].access
    );
    assert_eq!("manager-1@example.test", page.items[0].email.to_string());
    assert_eq!(Some("Ada"), page.items[0].first_name.as_deref());
    assert_eq!(Some("Lovelace"), page.items[0].last_name.as_deref());
    assert!(page.cursor.search_after.is_none());

    for (access, ids) in [
        (
            PartyLocationManagerAccessKind::Explicit,
            vec![explicit, both],
        ),
        (
            PartyLocationManagerAccessKind::Partnership,
            vec![member, both],
        ),
    ] {
        let page = list
            .execute(
                &ctx,
                ListPartyLocationManagersRequest {
                    access: Some(access),
                    ..request(party_id)
                },
            )
            .await
            .unwrap();
        assert_eq!(
            ids,
            page.items.iter().map(|row| row.user_id).collect::<Vec<_>>()
        );
        assert_eq!(2, page.items[1].access.len());
    }
    for id in [
        explicit,
        member,
        both,
        unrelated,
        suspended,
        deleted,
        UserId::new(),
    ] {
        let page = list
            .execute(
                &ctx,
                ListPartyLocationManagersRequest {
                    user_id: Some(id),
                    ..request(party_id)
                },
            )
            .await
            .unwrap();
        assert_eq!(
            usize::from([explicit, member, both].contains(&id)),
            page.items.len()
        );
        assert!(page.cursor.search_after.is_none());
    }
    let filtered = list
        .execute(
            &ctx,
            ListPartyLocationManagersRequest {
                user_id: Some(explicit),
                access: Some(PartyLocationManagerAccessKind::Partnership),
                ..request(party_id)
            },
        )
        .await
        .unwrap();
    assert!(filtered.items.is_empty());
    let first = list
        .execute(
            &ctx,
            ListPartyLocationManagersRequest {
                cursor: Cursor {
                    size: 2,
                    search_after: None,
                },
                ..request(party_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(Some(member), first.cursor.search_after);
    let second = list
        .execute(
            &ctx,
            ListPartyLocationManagersRequest {
                cursor: first.cursor,
                ..request(party_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(
        vec![both],
        second
            .items
            .iter()
            .map(|row| row.user_id)
            .collect::<Vec<_>>()
    );
    assert!(second.cursor.search_after.is_none());
    for (size, clamped) in [(0, 1), (3, 3), (200, 100)] {
        let page = list
            .execute(
                &ctx,
                ListPartyLocationManagersRequest {
                    cursor: Cursor {
                        size,
                        search_after: None,
                    },
                    ..request(party_id)
                },
            )
            .await
            .unwrap();
        assert_eq!(clamped, page.cursor.size);
        assert_eq!(
            if size == 0 { Some(explicit) } else { None },
            page.cursor.search_after
        );
    }
    // A removed cursor row must not prevent the remaining UUID range from being read.
    grant(&pool, party_id, explicit, false).await;
    let page = list
        .execute(
            &ctx,
            ListPartyLocationManagersRequest {
                cursor: Cursor {
                    size: 21,
                    search_after: Some(explicit),
                },
                ..request(party_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(2, page.items.len());
    grant(&pool, party_id, both, false).await;
    let page = list
        .execute(
            &ctx,
            ListPartyLocationManagersRequest {
                user_id: Some(both),
                ..request(party_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(vec![membership], page.items[0].access);
    grant(&pool, party_id, both, true).await;
    let page = list
        .execute(
            &ctx,
            ListPartyLocationManagersRequest {
                user_id: Some(both),
                ..request(party_id)
            },
        )
        .await
        .unwrap();
    assert_eq!(2, page.items[0].access.len());
    assert!(
        list.execute(&ctx, request(dissolved_party))
            .await
            .unwrap()
            .items
            .is_empty()
    );
    sqlx::query("UPDATE partnerships SET business_state='DISSOLVED' WHERE partnership_id=$1")
        .bind(active_id.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    let page = list.execute(&ctx, request(party_id)).await.unwrap();
    assert_eq!(1, page.items.len());
    assert_eq!(both, page.items[0].user_id);
    assert_eq!(
        vec![PartyLocationManagerAccess::Explicit],
        page.items[0].access
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn requires_current_admin_and_read_scope_and_reports_unknown_party() {
    let pool = get_postgres_client().await;
    let party_id = party(&pool).await;
    let admin = user(&pool, 10, "ADMIN").await;
    let non_admin = user(&pool, 11, "USER").await;
    grant(&pool, party_id, non_admin, true).await;
    let list = handler(&pool);
    for principal in [
        Principal::User(admin),
        Principal::DelegatedUser {
            user_id: admin,
            capabilities: [CredentialCapability::PartiesRead].into(),
        },
    ] {
        let ctx = context(principal);
        assert!(list.execute(&ctx, request(party_id)).await.is_ok());
        assert!(matches!(
            list.execute(&ctx, request(PartyId::new())).await,
            Err(PartyLocationError::NotFound)
        ));
    }
    for principal in [
        Principal::User(non_admin),
        Principal::DelegatedUser {
            user_id: admin,
            capabilities: [CredentialCapability::PartiesWrite].into(),
        },
        Principal::User(UserId::new()),
    ] {
        assert!(matches!(
            list.execute(&context(principal), request(party_id)).await,
            Err(PartyLocationError::Forbidden)
        ));
    }
    assert!(matches!(
        list.execute(&context(Principal::Anonymous), request(party_id))
            .await,
        Err(PartyLocationError::AuthenticationRequired)
    ));
    sqlx::query("UPDATE users SET suspended=true WHERE user_id=$1")
        .bind(admin.into_uuid())
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        list.execute(&context(Principal::User(admin)), request(party_id))
            .await,
        Err(PartyLocationError::Forbidden)
    ));
}
