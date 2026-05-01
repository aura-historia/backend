# Apply crawler migrations to all local crawler databases.
# Requires: cargo install sqlx-cli --no-default-features --features rustls,postgres

$migrationsDir = "$PSScriptRoot\..\..\migrations"
$dbUrls = @(
  "postgres://postgres:postgres@localhost:5432/crawler_server",
  "postgres://postgres:postgres@localhost:5432/crawler_demo",
  "postgres://postgres:postgres@localhost:5432/crawler_demo_scraper",
  "postgres://postgres:postgres@localhost:5432/crawler_demo_spider"
)

foreach ($dbUrl in $dbUrls) {
  Write-Host "Migrating $dbUrl"
  cargo sqlx migrate run --source $migrationsDir --database-url $dbUrl
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host "Migrations applied to all crawler databases."
