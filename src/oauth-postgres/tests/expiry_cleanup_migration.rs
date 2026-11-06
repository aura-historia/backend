use sqlx::PgPool;
use test_api::{IntegrationTestService, Postgres, aura_integration_test, get_postgres_client};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");

/// Deleted access tokens, authorization codes, third-party exchange codes, provider receipts,
/// refresh tokens, refresh-token families and connections.
type CleanupCounts = (i64, i64, i64, i64, i64, i64, i64);

const USER_ID: Uuid = Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f75);
const CLIENT_ID: Uuid = Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f76);
const EXPIRED_ACCESS_TOKEN_ID: Uuid = Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f77);
const FUTURE_ACCESS_TOKEN_ID: Uuid = Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f78);
const NON_EXPIRING_ACCESS_TOKEN_ID: Uuid = Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f79);
const EXPIRED_AUTHORIZATION_CODE: &str = "01890a5d-ac96-774b-bf1d-d5586c639f80";
const FUTURE_AUTHORIZATION_CODE: &str = "01890a5d-ac96-774b-bf1d-d5586c639f81";
const EXPIRED_THIRD_PARTY_EXCHANGE_CODE: &str = "01890a5d-ac96-774b-bf1d-d5586c639f82";
const FUTURE_THIRD_PARTY_EXCHANGE_CODE: &str = "01890a5d-ac96-774b-bf1d-d5586c639f83";
const EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID: Uuid =
    Uuid::from_u128(0x01890a5dac96774bbf1dd5586c639f7a);

#[aura_integration_test(services = [BUSINESS_SCHEMA], exclusive = true)]
async fn should_physically_remove_only_expired_oauth_credentials_in_a_bounded_batch() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let pg_ttl_installed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname = 'pg_ttl_index')",
        )
        .fetch_one(&pool)
        .await?;
        assert!(!pg_ttl_installed);

        let indexes: Vec<String> = sqlx::query_scalar(
            "SELECT indexrelname FROM pg_stat_user_indexes \
             WHERE indexrelname = ANY($1) ORDER BY indexrelname",
        )
        .bind(vec![
            "access_tokens_expires_at_idx",
            "oauth_authorization_codes_expires_at_idx",
            "oauth_third_party_exchange_codes_expires_at_idx",
        ])
        .fetch_all(&pool)
        .await?;
        assert_eq!(
            indexes,
            vec![
                "access_tokens_expires_at_idx",
                "oauth_authorization_codes_expires_at_idx",
                "oauth_third_party_exchange_codes_expires_at_idx",
            ]
        );

        let expired_at = OffsetDateTime::now_utc() - Duration::hours(1);
        let future_at = OffsetDateTime::now_utc() + Duration::hours(1);
        seed_user_and_client(&pool).await?;
        seed_access_token(
            &pool,
            EXPIRED_ACCESS_TOKEN_ID,
            "dummy-access-token-expired",
            Some(expired_at),
        )
        .await?;
        seed_access_token(
            &pool,
            FUTURE_ACCESS_TOKEN_ID,
            "dummy-access-token-future",
            Some(future_at),
        )
        .await?;
        seed_access_token(
            &pool,
            NON_EXPIRING_ACCESS_TOKEN_ID,
            "dummy-access-token-non-expiring",
            None,
        )
        .await?;
        seed_authorization_code(
            &pool,
            EXPIRED_AUTHORIZATION_CODE,
            "dummy-code-expired",
            expired_at,
        )
        .await?;
        seed_authorization_code(
            &pool,
            FUTURE_AUTHORIZATION_CODE,
            "dummy-code-future",
            future_at,
        )
        .await?;
        seed_third_party_exchange_code(
            &pool,
            EXPIRED_THIRD_PARTY_EXCHANGE_CODE,
            EXPIRED_ACCESS_TOKEN_ID,
            "dummy-third-party-code-expired",
            expired_at,
        )
        .await?;
        seed_third_party_exchange_code(
            &pool,
            FUTURE_THIRD_PARTY_EXCHANGE_CODE,
            FUTURE_ACCESS_TOKEN_ID,
            "dummy-third-party-code-future",
            future_at,
        )
        .await?;

        let deleted: CleanupCounts =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(1_000_i32)
                .fetch_one(&pool)
                .await?;
        assert_eq!(deleted, (1, 1, 1, 0, 0, 0, 0));

        assert!(!access_token_exists(&pool, EXPIRED_ACCESS_TOKEN_ID).await?);
        assert!(!authorization_code_exists(&pool, EXPIRED_AUTHORIZATION_CODE).await?);
        assert!(!third_party_exchange_code_exists(&pool, EXPIRED_THIRD_PARTY_EXCHANGE_CODE).await?);
        assert!(access_token_exists(&pool, FUTURE_ACCESS_TOKEN_ID).await?);
        assert!(authorization_code_exists(&pool, FUTURE_AUTHORIZATION_CODE).await?);
        assert!(third_party_exchange_code_exists(&pool, FUTURE_THIRD_PARTY_EXCHANGE_CODE).await?);
        assert!(access_token_exists(&pool, NON_EXPIRING_ACCESS_TOKEN_ID).await?);

        let repeated: CleanupCounts =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(1_000_i32)
                .fetch_one(&pool)
                .await?;
        assert_eq!(repeated, (0, 0, 0, 0, 0, 0, 0));

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth expiry cleanup migration integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], exclusive = true)]
async fn should_reject_null_cleanup_batch_size() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let null_batch_result: Result<CleanupCounts, sqlx::Error> =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(Option::<i32>::None)
                .fetch_one(&pool)
                .await;

        assert!(null_batch_result.is_err());
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth expiry cleanup NULL batch-size integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], exclusive = true)]
async fn should_not_cascade_an_expired_access_token_with_more_expired_exchange_codes_than_batch_size()
 {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let expired_at = OffsetDateTime::now_utc() - Duration::hours(1);
        seed_user_and_client(&pool).await?;
        seed_access_token(
            &pool,
            EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID,
            "dummy-access-token-many-expired-codes",
            Some(expired_at),
        )
        .await?;
        for third_party_exchange_code in [
            "many-expired-third-party-code-one",
            "many-expired-third-party-code-two",
            "many-expired-third-party-code-three",
        ] {
            seed_third_party_exchange_code(
                &pool,
                third_party_exchange_code,
                EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID,
                third_party_exchange_code,
                expired_at,
            )
            .await?;
        }

        let first_deleted: CleanupCounts =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(1_i32)
                .fetch_one(&pool)
                .await?;
        assert_eq!(first_deleted, (0, 0, 1, 0, 0, 0, 0));
        assert!(access_token_exists(&pool, EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID).await?);
        assert_eq!(
            third_party_exchange_code_count_for_access_token(
                &pool,
                EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID,
            )
            .await?,
            2
        );

        let second_deleted: CleanupCounts =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(1_i32)
                .fetch_one(&pool)
                .await?;
        assert_eq!(second_deleted, (0, 0, 1, 0, 0, 0, 0));
        assert!(access_token_exists(&pool, EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID).await?);

        let third_deleted: CleanupCounts =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(1_i32)
                .fetch_one(&pool)
                .await?;
        assert_eq!(third_deleted, (1, 0, 1, 0, 0, 0, 0));
        assert!(!access_token_exists(&pool, EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID).await?);
        assert_eq!(
            third_party_exchange_code_count_for_access_token(
                &pool,
                EXPIRED_ACCESS_TOKEN_WITH_MANY_CODES_ID,
            )
            .await?,
            0
        );

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth expiry cleanup bounded dependent integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], exclusive = true)]
async fn should_share_expired_rows_between_concurrent_cleanup_transactions() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let expired_at = OffsetDateTime::now_utc() - Duration::hours(1);
        seed_user_and_client(&pool).await?;
        seed_authorization_code(
            &pool,
            "01890a5d-ac96-774b-bf1d-d5586c639f84",
            "concurrent-code-one",
            expired_at,
        )
        .await?;
        seed_authorization_code(
            &pool,
            "01890a5d-ac96-774b-bf1d-d5586c639f85",
            "concurrent-code-two",
            expired_at,
        )
        .await?;

        let mut first = pool.begin().await?;
        let first_deleted: CleanupCounts = sqlx::query_as(
            "SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)",
        )
        .bind(1_i32)
        .fetch_one(&mut *first)
        .await?;
        assert_eq!(first_deleted, (0, 1, 0, 0, 0, 0, 0));

        let mut second = pool.begin().await?;
        let second_deleted: CleanupCounts = sqlx::query_as(
            "SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)",
        )
        .bind(1_i32)
        .fetch_one(&mut *second)
        .await?;
        assert_eq!(second_deleted, (0, 1, 0, 0, 0, 0, 0));

        first.commit().await?;
        second.commit().await?;
        let remaining: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM oauth_authorization_codes WHERE expires_at < statement_timestamp()",
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(remaining, 0);

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth concurrent expiry cleanup integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], exclusive = true)]
async fn should_restore_expired_rows_when_cleanup_transaction_rolls_back() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let expired_at = OffsetDateTime::now_utc() - Duration::hours(1);
        let authorization_code = "01890a5d-ac96-774b-bf1d-d5586c639f86";
        seed_user_and_client(&pool).await?;
        seed_authorization_code(&pool, authorization_code, "rollback-code", expired_at).await?;

        let mut transaction = pool.begin().await?;
        let deleted: CleanupCounts =
            sqlx::query_as("SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)")
                .bind(1_i32)
                .fetch_one(&mut *transaction)
                .await?;
        assert_eq!(deleted, (0, 1, 0, 0, 0, 0, 0));

        transaction.rollback().await?;
        assert!(authorization_code_exists(&pool, authorization_code).await?);

        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth expiry cleanup rollback integration test failed: {result:?}"
    );
}

#[aura_integration_test(services = [BUSINESS_SCHEMA], exclusive = true)]
async fn should_remove_expired_refresh_token_families_in_bounded_steps_then_empty_connections() {
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = get_postgres_client().await;
        let now = OffsetDateTime::now_utc();
        seed_user_and_client(&pool).await?;
        let expired_connection = seed_connection(&pool, "expired", now).await?;
        let live_connection = seed_connection(&pool, "live", now).await?;
        let pending_connection = seed_connection(&pool, "pending", now).await?;
        let empty_connection = seed_connection(&pool, "empty", now).await?;
        seed_family(
            &pool,
            expired_connection,
            "expired",
            3,
            now - Duration::hours(1),
        )
        .await?;
        seed_family(&pool, live_connection, "live", 2, now + Duration::days(1)).await?;
        // An approved code not yet exchanged keeps its connection.
        sqlx::query(
            "INSERT INTO oauth_authorization_codes ( \
                 authorization_code, metadata_document_client_id, user_id, redirect_uri, \
                 code_challenge, code_challenge_method, expires_at \
             ) VALUES ('01890a5d-ac96-774b-bf1d-d5586c639f90', \
                 'https://pending.example/client.json', $1, 'http://127.0.0.1/callback', \
                 'pending-challenge', 'S256', $2)",
        )
        .bind(USER_ID)
        .bind(now + Duration::minutes(5))
        .execute(&pool)
        .await?;

        let cleanup = |pool: PgPool| async move {
            sqlx::query_as::<_, CleanupCounts>(
                "SELECT * FROM cleanup_expired_credentials_and_provider_receipts($1)",
            )
            .bind(2_i32)
            .fetch_one(&pool)
            .await
        };
        // Two of the expired family's three tokens, and the connection without credentials.
        assert_eq!((0, 0, 0, 0, 2, 0, 1), cleanup(pool.clone()).await?);
        assert!(connection_exists(&pool, expired_connection).await?);
        assert!(!connection_exists(&pool, empty_connection).await?);
        // The last token, then its family, then the connection it emptied.
        assert_eq!((0, 0, 0, 0, 1, 1, 1), cleanup(pool.clone()).await?);
        assert!(!connection_exists(&pool, expired_connection).await?);
        assert_eq!((0, 0, 0, 0, 0, 0, 0), cleanup(pool.clone()).await?);
        assert!(connection_exists(&pool, live_connection).await?);
        assert!(connection_exists(&pool, pending_connection).await?);
        let live_tokens: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM oauth_refresh_tokens")
            .fetch_one(&pool)
            .await?;
        assert_eq!(2, live_tokens);
        Ok(())
    }
    .await;

    assert!(
        result.is_ok(),
        "OAuth connection cleanup integration test failed: {result:?}"
    );
}

async fn seed_connection(
    pool: &PgPool,
    host: &str,
    now: OffsetDateTime,
) -> Result<Uuid, sqlx::Error> {
    let connection_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO oauth_connections \
             (connection_id, user_id, client_id, client_name, scopes, created, last_used) \
         VALUES ($1, $2, $3, $4, ARRAY['users:read'], $5, $5)",
    )
    .bind(connection_id)
    .bind(USER_ID)
    .bind(format!("https://{host}.example/client.json"))
    .bind(host)
    .bind(now - Duration::days(1))
    .execute(pool)
    .await?;
    Ok(connection_id)
}

/// A family whose current token expires at `expires_at`, after `tokens - 1` rotations.
async fn seed_family(
    pool: &PgPool,
    connection_id: Uuid,
    name: &str,
    tokens: usize,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    let family_id = Uuid::now_v7();
    let created = expires_at - Duration::days(30);
    sqlx::query(
        "INSERT INTO oauth_refresh_token_families \
             (family_id, connection_id, scopes, resource, created, absolute_expires_at, \
              expires_at) \
         VALUES ($1, $2, ARRAY['users:read'], 'https://mcp.aura-historia.com/v1', $3, $4, $4)",
    )
    .bind(family_id)
    .bind(connection_id)
    .bind(created)
    .bind(expires_at)
    .execute(pool)
    .await?;
    for index in 0..tokens {
        let issued = created + Duration::minutes(i64::try_from(index).unwrap_or_default());
        let replaced_at = (index + 1 < tokens).then(|| issued + Duration::minutes(1));
        sqlx::query(
            "INSERT INTO oauth_refresh_tokens \
                 (token_short, token_hash, family_id, issued, replaced_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(format!("{name}-{index}-short"))
        .bind(format!("{name}-{index}-hash"))
        .bind(family_id)
        .bind(issued)
        .bind(replaced_at)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn connection_exists(pool: &PgPool, connection_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM oauth_connections WHERE connection_id = $1)")
        .bind(connection_id)
        .fetch_one(pool)
        .await
}

async fn seed_user_and_client(pool: &PgPool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO users (user_id, email, tier, role) \
         VALUES ($1, 'dummy-oauth-ttl@example.test', 'FREE', 'USER')",
    )
    .bind(USER_ID)
    .execute(pool)
    .await?;

    sqlx::query(
        "INSERT INTO oauth_clients ( \
             client_id, client_secret_short_token, client_secret_long_token_hash, name, \
             redirect_uris, tos_uri, policy_uri, client_uri, logo_uri \
         ) VALUES ( \
             $1, 'dummy-client-secret-short', 'dummy-client-secret-hash', 'dummy-client', \
             ARRAY['https://example.test/oauth/callback'], 'https://example.test/tos', \
             'https://example.test/policy', 'https://example.test', \
             'https://example.test/logo.svg' \
         )",
    )
    .bind(CLIENT_ID)
    .execute(pool)
    .await?;

    Ok(())
}

async fn seed_access_token(
    pool: &PgPool,
    access_token_id: Uuid,
    token_suffix: &str,
    expires_at: Option<OffsetDateTime>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO access_tokens ( \
             access_token_id, user_id, token_short, token_hash, name, origin, oauth_client_id, expires_at \
         ) VALUES ($1, $2, $3, $4, $5, 'OAUTH', $6, $7)",
    )
    .bind(access_token_id)
    .bind(USER_ID)
    .bind(format!("{token_suffix}-short"))
    .bind(format!("{token_suffix}-hash"))
    .bind(token_suffix)
    .bind(CLIENT_ID)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn seed_authorization_code(
    pool: &PgPool,
    authorization_code: &str,
    code_challenge: &str,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_authorization_codes ( \
             authorization_code, client_id, user_id, redirect_uri, code_challenge, \
             code_challenge_method, expires_at \
         ) VALUES ($1, $2, $3, 'https://example.test/oauth/callback', $4, 'S256', $5)",
    )
    .bind(authorization_code)
    .bind(CLIENT_ID)
    .bind(USER_ID)
    .bind(code_challenge)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn seed_third_party_exchange_code(
    pool: &PgPool,
    third_party_exchange_code: &str,
    access_token_id: Uuid,
    access_token: &str,
    expires_at: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_third_party_exchange_codes ( \
             third_party_exchange_code, access_token_id, access_token, expires_at \
         ) VALUES ($1, $2, $3, $4)",
    )
    .bind(third_party_exchange_code)
    .bind(access_token_id)
    .bind(access_token)
    .bind(expires_at)
    .execute(pool)
    .await?;

    Ok(())
}

async fn access_token_exists(pool: &PgPool, access_token_id: Uuid) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM access_tokens WHERE access_token_id = $1)")
        .bind(access_token_id)
        .fetch_one(pool)
        .await
}

async fn authorization_code_exists(
    pool: &PgPool,
    authorization_code: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS( \
         SELECT 1 FROM oauth_authorization_codes WHERE authorization_code = $1)",
    )
    .bind(authorization_code)
    .fetch_one(pool)
    .await
}

async fn third_party_exchange_code_exists(
    pool: &PgPool,
    third_party_exchange_code: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS( \
         SELECT 1 FROM oauth_third_party_exchange_codes WHERE third_party_exchange_code = $1)",
    )
    .bind(third_party_exchange_code)
    .fetch_one(pool)
    .await
}

async fn third_party_exchange_code_count_for_access_token(
    pool: &PgPool,
    access_token_id: Uuid,
) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM oauth_third_party_exchange_codes WHERE access_token_id = $1",
    )
    .bind(access_token_id)
    .fetch_one(pool)
    .await
}
