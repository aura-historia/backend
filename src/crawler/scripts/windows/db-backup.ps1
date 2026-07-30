# Back up all local crawler Docker Postgres databases.

param(
  [string]$BackupDir
)

$composeDir = "$PSScriptRoot\..\.."
$composeFile = "$composeDir\docker-compose.yml"
$dbNames = @(
  "crawler_server",
  "crawler_demo",
  "crawler_demo_scraper",
  "crawler_demo_spider"
)

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

docker compose -f $composeFile up -d
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

if (-not $BackupDir) {
  $timestamp = (Get-Date).ToUniversalTime().ToString("yyyyMMddTHHmmssZ")
  $BackupDir = "$composeDir\backups\$timestamp"
}

New-Item -ItemType Directory -Force -Path $BackupDir | Out-Null

$containerId = docker compose -f $composeFile ps -q postgres
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (-not $containerId) {
  Write-Error "Local crawler Postgres container is not running. Run db-up first."
  exit 1
}

foreach ($dbName in $dbNames) {
  $dumpFile = "/tmp/$dbName.dump"
  Write-Host "Backing up $dbName"
  docker compose -f $composeFile exec -T postgres pg_dump --username postgres --dbname $dbName --format custom --file $dumpFile
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  docker cp "${containerId}:${dumpFile}" "$BackupDir\$dbName.dump"
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  docker compose -f $composeFile exec -T postgres rm -f $dumpFile
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host "Crawler Postgres backup written to $BackupDir"
