# Show applied / pending migration status for all local crawler databases.
# Requires: cargo install sqlx-cli --no-default-features --features rustls,postgres
$migrationsDir = "$PSScriptRoot\..\..\migrations"
$dbUrls = @(
  "postgres://postgres:postgres@localhost:5432/crawler_server",
  "postgres://postgres:postgres@localhost:5432/crawler_demo",
  "postgres://postgres:postgres@localhost:5432/crawler_demo_scraper",
  "postgres://postgres:postgres@localhost:5432/crawler_demo_spider"
)

foreach ($dbUrl in $dbUrls) {
  Write-Host "Migration status for $dbUrl"
  cargo sqlx migrate info --source $migrationsDir --database-url $dbUrl
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
