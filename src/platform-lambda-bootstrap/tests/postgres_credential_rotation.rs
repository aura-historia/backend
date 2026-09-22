use platform_lambda_bootstrap::VersionedCompositionCache;
use platform_postgres::PostgresPoolConfig;

use std::path::PathBuf;
use test_api::{
    IntegrationTestService, Postgres, aura_integration_test, get_postgres_client,
    get_postgres_host_gateway_connection_string, get_postgres_tls_root_certificate_path,
};

const BUSINESS_SCHEMA: Postgres = Postgres::new("migrations");
const ROTATION_ROLE: &str = "c07_runtime_rotation";
const OLD_PASSWORD: &str = "c07-old-password";
const NEW_PASSWORD: &str = "c07-new-password";

#[aura_integration_test(services = [BUSINESS_SCHEMA])]
async fn should_keep_an_old_pool_lease_alive_through_a_real_postgres_password_rotation() {
    let admin = get_postgres_client().await;
    let (host, port, root_certificate) = runtime_connection();
    drop_rotation_role(&admin).await;
    create_rotation_role(&admin).await;

    let cache = VersionedCompositionCache::new();
    let old = cache
        .get_or_try_build("version-old", || {
            build_pool(host.clone(), port, root_certificate.clone(), OLD_PASSWORD)
        })
        .await;
    let old = match old {
        Ok(lease) => lease,
        Err(error) => panic!("failed to build old credential pool: {error}"),
    };
    let mut old_transaction = match old.value().begin().await {
        Ok(transaction) => transaction,
        Err(error) => panic!("failed to begin old credential transaction: {error}"),
    };
    let old_user: String = match sqlx::query_scalar("SELECT current_user")
        .fetch_one(&mut *old_transaction)
        .await
    {
        Ok(user) => user,
        Err(error) => panic!("failed to query through old credential transaction: {error}"),
    };

    rotate_role_password(&admin).await;
    let new = cache
        .get_or_try_build("version-new", || {
            build_pool(host.clone(), port, root_certificate.clone(), NEW_PASSWORD)
        })
        .await;
    let new = match new {
        Ok(lease) => lease,
        Err(error) => panic!("failed to build rotated credential pool: {error}"),
    };
    let new_user: String = match sqlx::query_scalar("SELECT current_user")
        .fetch_one(new.value())
        .await
    {
        Ok(user) => user,
        Err(error) => panic!("failed to query through rotated credential pool: {error}"),
    };
    let old_query: i32 = match sqlx::query_scalar("SELECT 1")
        .fetch_one(&mut *old_transaction)
        .await
    {
        Ok(value) => value,
        Err(error) => panic!("old credential transaction was interrupted by rotation: {error}"),
    };
    if let Err(error) = old_transaction.commit().await {
        panic!("failed to commit old credential transaction: {error}");
    }

    assert_eq!(old.version_id(), "version-old");
    assert_eq!(new.version_id(), "version-new");
    assert_eq!(old_user, ROTATION_ROLE);
    assert_eq!(new_user, ROTATION_ROLE);
    assert_eq!(old_query, 1);

    old.value().close().await;
    new.value().close().await;
    drop_rotation_role(&admin).await;
}

fn runtime_connection() -> (String, u16, PathBuf) {
    let connection = match url::Url::parse(&get_postgres_host_gateway_connection_string("postgres"))
    {
        Ok(connection) => connection,
        Err(error) => panic!("failed to parse test Postgres connection: {error}"),
    };
    // This integration test runs on the host, unlike the packaged Lambda fixture.
    // The fixture certificate deliberately includes `localhost` for verified TLS.
    let host = "localhost".to_owned();
    let port = match connection.port() {
        Some(port) => port,
        None => panic!("test Postgres connection has no port"),
    };

    (host, port, get_postgres_tls_root_certificate_path())
}

async fn build_pool(
    host: String,
    port: u16,
    root_certificate: PathBuf,
    password: &str,
) -> Result<sqlx::PgPool, &'static str> {
    let config = PostgresPoolConfig::lambda(
        host,
        port,
        "postgres".to_owned(),
        ROTATION_ROLE.to_owned(),
        password.to_owned(),
        1,
        root_certificate,
    )
    .map_err(|_| "invalid pool configuration")?;
    config
        .connect()
        .await
        .map_err(|_| "pool construction failed")
}

async fn create_rotation_role(pool: &sqlx::PgPool) {
    if let Err(error) =
        sqlx::query("CREATE ROLE c07_runtime_rotation LOGIN PASSWORD 'c07-old-password'")
            .execute(pool)
            .await
    {
        panic!("failed to create PostgreSQL rotation role: {error}");
    }
}

async fn rotate_role_password(pool: &sqlx::PgPool) {
    if let Err(error) = sqlx::query("ALTER ROLE c07_runtime_rotation PASSWORD 'c07-new-password'")
        .execute(pool)
        .await
    {
        panic!("failed to rotate PostgreSQL test role password: {error}");
    }
}

async fn drop_rotation_role(pool: &sqlx::PgPool) {
    if let Err(error) = sqlx::query("DROP ROLE IF EXISTS c07_runtime_rotation")
        .execute(pool)
        .await
    {
        panic!("failed to drop PostgreSQL rotation role: {error}");
    }
}
