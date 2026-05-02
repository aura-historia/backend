# Stop the crawler Postgres container (data volume is preserved).
$composeDir = "$PSScriptRoot\..\.."
docker compose -f "$composeDir\docker-compose.yml" down
