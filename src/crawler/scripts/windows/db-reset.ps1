# Destroy the crawler Postgres container AND its data volume, then start fresh.
# Migrations can then be re-applied with .\db-migrate.ps1.
$composeDir = "$PSScriptRoot\..\.."
docker compose -f "$composeDir\docker-compose.yml" down -v
docker compose -f "$composeDir\docker-compose.yml" up -d
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& "$PSScriptRoot\db-up.ps1"
