# Show applied / pending migration status for the local docker-compose Postgres.
# Requires: cargo install sqlx-cli --no-default-features --features rustls,postgres
$migrationsDir = "$PSScriptRoot\..\migrations"
cargo sqlx migrate info --source $migrationsDir
