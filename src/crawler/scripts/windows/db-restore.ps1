# Restore all local crawler Docker Postgres databases from a backup directory.

param(
  [Parameter(Mandatory = $true)]
  [string]$BackupDir,
  [switch]$Yes
)

if (-not $Yes) {
  Write-Error "Usage: .\db-restore.ps1 -BackupDir <path> -Yes"
  Write-Error "Restores local crawler dev Postgres only."
  exit 1
}

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

foreach ($dbName in $dbNames) {
  $dumpPath = "$BackupDir\$dbName.dump"
  if (-not (Test-Path $dumpPath) -or (Get-Item $dumpPath).Length -eq 0) {
    Write-Error "Missing backup dump: $dumpPath"
    exit 1
  }
}

& "$PSScriptRoot\db-up.ps1"
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$containerId = docker compose -f $composeFile ps -q postgres
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
if (-not $containerId) {
  Write-Error "Local crawler Postgres container is not running. Run db-up first."
  exit 1
}

Write-Host "Restoring local crawler dev Postgres only."
foreach ($dbName in $dbNames) {
  $dumpFile = "/tmp/$dbName.dump"
  Write-Host "Restoring $dbName"
  docker cp "$BackupDir\$dbName.dump" "${containerId}:${dumpFile}"
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  docker compose -f $composeFile exec -T postgres pg_restore --username postgres --dbname $dbName --clean --if-exists $dumpFile
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
  docker compose -f $composeFile exec -T postgres rm -f $dumpFile
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

Write-Host "Crawler Postgres restore complete."
