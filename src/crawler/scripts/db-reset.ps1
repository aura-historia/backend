# Destroy the crawler Postgres container AND its data volume, then start fresh.
# This replaces the old "delete container, rebuild, re-run schema.sql" workflow.
# Migrations are re-applied automatically next time `cargo run -p crawler --bin demo` runs.
$composeDir = "$PSScriptRoot\.."
docker compose -f "$composeDir\docker-compose.yml" down -v
docker compose -f "$composeDir\docker-compose.yml" up -d
