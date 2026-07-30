# Destroy the crawler Postgres container AND its data volume, then start fresh.
# Migrations can then be re-applied with .\db-migrate.ps1.
$composeDir = "$PSScriptRoot\..\.."
$composeFile = "$composeDir\docker-compose.yml"

if (-not (Test-Path $composeFile)) {
  Write-Error "Crawler Docker Compose file not found: $composeFile"
  exit 1
}

$services = docker compose -f $composeFile config --services
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if ($services -notcontains "postgres") {
  Write-Error "Crawler Docker Compose file has no postgres service: $composeFile"
  exit 1
}

Write-Host "Resetting local crawler dev Postgres only."
docker compose -f $composeFile down -v
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
docker compose -f $composeFile up -d
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

& "$PSScriptRoot\db-up.ps1"
