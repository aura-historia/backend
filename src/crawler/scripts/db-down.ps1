# Stop the crawler Postgres container (data volume is preserved).
# Safe to run from any directory.
$composeDir = "$PSScriptRoot\.."
docker compose -f "$composeDir\docker-compose.yml" down
