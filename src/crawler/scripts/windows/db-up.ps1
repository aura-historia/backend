# Start local crawler Postgres and ensure all crawler databases exist.
# Requires: cargo install sqlx-cli --no-default-features --features rustls,postgres

$composeDir = "$PSScriptRoot\..\.."
docker compose -f "$composeDir\docker-compose.yml" up -d
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$dbUrls = @(
  "postgres://postgres:postgres@localhost:5432/crawler_server",
  "postgres://postgres:postgres@localhost:5432/crawler_demo",
  "postgres://postgres:postgres@localhost:5432/crawler_demo_scraper",
  "postgres://postgres:postgres@localhost:5432/crawler_demo_spider"
)

foreach ($dbUrl in $dbUrls) {
  cargo sqlx database create --database-url $dbUrl
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host "Local crawler Postgres is up and databases are ready."
