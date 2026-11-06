use application::transaction::{Transaction, UnitOfWork};
use credential_core::metadata_document_client_id::MetadataDocumentClientId;
use credential_core::scope::Scope;
use oauth_core::client::OAuthClientName;
use oauth_core::connection::{OAuthConnection, OAuthConnectionApproval};
use oauth_core::refresh_token::{
    HashedRawRefreshToken, MetadataDocumentClientTokenLifetimes, PresentedRefreshToken,
    RefreshTokenFamily,
};
use oauth_core::resource::AuraResource;
use oauth_postgres::{
    SqlxOAuthConnectionGrantReader, SqlxOAuthConnectionRepositoryFactory,
    SqlxRefreshTokenFamilyRepositoryFactory,
};
use oauth_service::ports::{
    OAuthConnectionGrantReader, OAuthConnectionRepository, OAuthConnectionRepositoryError,
    OAuthConnectionRepositoryFactory, RefreshTokenFamilyRepository,
    RefreshTokenFamilyRepositoryFactory,
};
use platform_postgres::{SqlxTransaction, SqlxUnitOfWork};
use sqlx::PgPool;
use std::collections::HashSet;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use user_core::user_id::UserId;
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const CLIENT_ID: &str = "https://agent.example/oauth/client.json";

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_persist_one_connection_per_user_and_client_and_read_its_grants() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let connections = SqlxOAuthConnectionRepositoryFactory::new();
    let user_id = seed_user(&pool).await;
    let client_id = client_id();
    let mut connection = start_connection(user_id, &client_id, &[Scope::UsersRead]);

    let mut tx = begin(&unit_of_work).await;
    connections
        .in_transaction(&mut tx)
        .insert(&connection)
        .await
        .unwrap();
    let duplicate = connections
        .in_transaction(&mut tx)
        .insert(&start_connection(user_id, &client_id, &[]))
        .await;
    assert!(matches!(
        duplicate,
        Err(OAuthConnectionRepositoryError::Conflict { .. })
    ));
    drop(tx);

    let mut tx = begin(&unit_of_work).await;
    connections
        .in_transaction(&mut tx)
        .insert(&connection)
        .await
        .unwrap();
    connection.approve_again(OAuthConnectionApproval {
        user_id,
        client_id: &client_id,
        client_name: &OAuthClientName::from("Agent v2"),
        scopes: &HashSet::from([Scope::WatchlistRead]),
        approved_at: connection.created() + Duration::minutes(1),
    });
    connections
        .in_transaction(&mut tx)
        .update(&connection)
        .await
        .unwrap();
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    let found = connections
        .in_transaction(&mut tx)
        .find_by_user_and_client_for_update(user_id, &client_id)
        .await
        .unwrap()
        .expect("persisted connection");
    let by_id = connections
        .in_transaction(&mut tx)
        .find_by_id_for_update(connection.id())
        .await
        .unwrap();
    commit(tx).await;
    assert_eq!(connection, found);
    assert_eq!(Some(connection.clone()), by_id);
    assert_eq!(
        HashSet::from([Scope::UsersRead, Scope::WatchlistRead]),
        SqlxOAuthConnectionGrantReader::new(pool.clone())
            .granted_scopes(user_id, &client_id)
            .await
            .unwrap()
    );
    assert!(
        SqlxOAuthConnectionGrantReader::new(pool.clone())
            .granted_scopes(UserId::new(), &client_id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_rotate_a_family_and_recognize_every_replaced_token() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let families = SqlxRefreshTokenFamilyRepositoryFactory::new();
    let connection = seed_connection(&pool, &unit_of_work).await;
    let lifetimes = MetadataDocumentClientTokenLifetimes::default();
    let now = whole_microseconds(OffsetDateTime::now_utc());
    let (mut family, first) = RefreshTokenFamily::start(
        connection.id(),
        HashSet::from([Scope::UsersRead]),
        AuraResource::ProductionMcp,
        &lifetimes,
        now,
    );
    let first = HashedRawRefreshToken::from(first);

    let mut tx = begin(&unit_of_work).await;
    families
        .in_transaction(&mut tx)
        .insert(&family)
        .await
        .unwrap();
    commit(tx).await;

    let mut replaced = Vec::new();
    for minutes in [1, 2] {
        let mut tx = begin(&unit_of_work).await;
        let current = family.current().hashed_token.clone();
        let (found, presented) = families
            .in_transaction(&mut tx)
            .find_by_token_for_update(&current)
            .await
            .unwrap()
            .expect("current token");
        assert_eq!(family, found);
        assert_eq!(PresentedRefreshToken::Current, presented);
        let rotation = family.rotate(&lifetimes, now + Duration::minutes(minutes));
        families
            .in_transaction(&mut tx)
            .record_rotation(&family, &rotation.replaced)
            .await
            .unwrap();
        commit(tx).await;
        replaced.push((rotation.replaced, now + Duration::minutes(minutes)));
    }

    assert_eq!(first, replaced[0].0);
    let mut tx = begin(&unit_of_work).await;
    for (token, at) in &replaced {
        let (found, presented) = families
            .in_transaction(&mut tx)
            .find_by_token_for_update(token)
            .await
            .unwrap()
            .expect("replaced tokens stay recognizable");
        assert_eq!(family, found);
        assert_eq!(PresentedRefreshToken::Replaced { at: *at }, presented);
        assert_eq!(
            Some(connection.id()),
            families
                .in_transaction(&mut tx)
                .find_connection_id(token)
                .await
                .unwrap()
        );
    }
    let stale_rotation = families
        .in_transaction(&mut tx)
        .record_rotation(&family, &replaced[0].0)
        .await;
    assert!(matches!(
        stale_rotation,
        Err(OAuthConnectionRepositoryError::InvalidPersistedState { .. })
    ));
    drop(tx);

    let mut tx = begin(&unit_of_work).await;
    families
        .in_transaction(&mut tx)
        .delete(family.id())
        .await
        .unwrap();
    commit(tx).await;
    let mut tx = begin(&unit_of_work).await;
    assert!(
        families
            .in_transaction(&mut tx)
            .find_by_token_for_update(&replaced[1].0)
            .await
            .unwrap()
            .is_none()
    );
    commit(tx).await;
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_refresh_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, remaining, "ending a family ends every token it issued");
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_serialize_concurrent_refreshes_so_the_loser_sees_the_token_replaced() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let families = SqlxRefreshTokenFamilyRepositoryFactory::new();
    let connection = seed_connection(&pool, &unit_of_work).await;
    let lifetimes = MetadataDocumentClientTokenLifetimes::default();
    let now = whole_microseconds(OffsetDateTime::now_utc());
    let (mut family, token) = RefreshTokenFamily::start(
        connection.id(),
        HashSet::from([Scope::UsersRead]),
        AuraResource::ProductionMcp,
        &lifetimes,
        now,
    );
    let token = HashedRawRefreshToken::from(token);
    let mut tx = begin(&unit_of_work).await;
    families
        .in_transaction(&mut tx)
        .insert(&family)
        .await
        .unwrap();
    commit(tx).await;

    let mut winner = begin(&unit_of_work).await;
    let (_, presented) = families
        .in_transaction(&mut winner)
        .find_by_token_for_update(&token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(PresentedRefreshToken::Current, presented);

    let loser = tokio::spawn({
        let unit_of_work = unit_of_work.clone();
        let token = token.clone();
        async move {
            let mut tx = begin(&unit_of_work).await;
            let presented = SqlxRefreshTokenFamilyRepositoryFactory::new()
                .in_transaction(&mut tx)
                .find_by_token_for_update(&token)
                .await
                .unwrap()
                .map(|(_, presented)| presented);
            commit(tx).await;
            presented
        }
    });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(!loser.is_finished(), "the loser waits for the family lock");
    let rotated_at = now + Duration::seconds(1);
    let rotation = family.rotate(&lifetimes, rotated_at);
    families
        .in_transaction(&mut winner)
        .record_rotation(&family, &rotation.replaced)
        .await
        .unwrap();
    commit(winner).await;

    assert_eq!(
        Some(PresentedRefreshToken::Replaced { at: rotated_at }),
        loser.await.unwrap()
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], isolated = true)]
async fn should_end_every_credential_with_its_connection() {
    let pool = get_postgres_client().await;
    let unit_of_work = SqlxUnitOfWork::new(pool.clone());
    let connections = SqlxOAuthConnectionRepositoryFactory::new();
    let families = SqlxRefreshTokenFamilyRepositoryFactory::new();
    let connection = seed_connection(&pool, &unit_of_work).await;
    let (family, token) = RefreshTokenFamily::start(
        connection.id(),
        HashSet::from([Scope::UsersRead]),
        AuraResource::ProductionMcp,
        &MetadataDocumentClientTokenLifetimes::default(),
        OffsetDateTime::now_utc(),
    );
    let mut tx = begin(&unit_of_work).await;
    families
        .in_transaction(&mut tx)
        .insert(&family)
        .await
        .unwrap();
    commit(tx).await;
    sqlx::query(
        "INSERT INTO access_tokens (access_token_id, user_id, token_short, token_hash, name, \
             origin, oauth_metadata_document_client_id, oauth_connection_id, expires_at) \
         VALUES ($1, $2, 'connection-short', 'connection-hash', 'agent', \
             'OAUTH_METADATA_DOCUMENT', $3, $4, now() + interval '1 hour')",
    )
    .bind(Uuid::now_v7())
    .bind(connection.user_id().into_uuid())
    .bind(connection.client_id().as_str())
    .bind(connection.id().into_uuid())
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = begin(&unit_of_work).await;
    assert!(
        !connections
            .in_transaction(&mut tx)
            .delete(UserId::new(), connection.id())
            .await
            .unwrap(),
        "only the owner's connection is deleted"
    );
    assert!(
        connections
            .in_transaction(&mut tx)
            .delete(connection.user_id(), connection.id())
            .await
            .unwrap()
    );
    commit(tx).await;

    let mut tx = begin(&unit_of_work).await;
    assert!(
        families
            .in_transaction(&mut tx)
            .find_connection_id(&HashedRawRefreshToken::from(token))
            .await
            .unwrap()
            .is_none()
    );
    commit(tx).await;
    let access_tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM access_tokens")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(0, access_tokens);
}

fn client_id() -> MetadataDocumentClientId {
    MetadataDocumentClientId::try_from(CLIENT_ID).unwrap()
}

fn start_connection(
    user_id: UserId,
    client_id: &MetadataDocumentClientId,
    scopes: &[Scope],
) -> OAuthConnection {
    OAuthConnection::start(OAuthConnectionApproval {
        user_id,
        client_id,
        client_name: &OAuthClientName::from("Agent"),
        scopes: &scopes.iter().copied().collect(),
        approved_at: whole_microseconds(OffsetDateTime::now_utc()),
    })
}

async fn seed_connection(pool: &PgPool, unit_of_work: &SqlxUnitOfWork) -> OAuthConnection {
    let connection = start_connection(seed_user(pool).await, &client_id(), &[Scope::UsersRead]);
    let mut tx = begin(unit_of_work).await;
    SqlxOAuthConnectionRepositoryFactory::new()
        .in_transaction(&mut tx)
        .insert(&connection)
        .await
        .unwrap();
    commit(tx).await;
    connection
}

async fn seed_user(pool: &PgPool) -> UserId {
    let user_id = UserId::new();
    sqlx::query("INSERT INTO users (user_id, email, tier, role) VALUES ($1, $2, 'FREE', 'USER')")
        .bind(user_id.into_uuid())
        .bind(format!("oauth-connection-{}@example.test", Uuid::now_v7()))
        .execute(pool)
        .await
        .unwrap();
    user_id
}

/// PostgreSQL keeps microseconds.
fn whole_microseconds(at: OffsetDateTime) -> OffsetDateTime {
    at.replace_nanosecond(at.nanosecond() / 1_000 * 1_000)
        .unwrap()
}

async fn begin(unit_of_work: &SqlxUnitOfWork) -> SqlxTransaction {
    unit_of_work.begin().await.unwrap()
}

async fn commit(tx: SqlxTransaction) {
    tx.commit().await.unwrap();
}
