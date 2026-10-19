# Snapshot the crawler_demo database and the review console during a dev run.
# Read-only. Requires the crawler-postgres-1 container and, for the review section, the dev binary.
#
# Usage: .\soak-status.ps1 [-Container crawler-postgres-1] [-Database crawler_demo] [-Console http://127.0.0.1:7878]
param(
    [string]$Container = 'crawler-postgres-1',
    [string]$Database = 'crawler_demo',
    [string]$Console = 'http://127.0.0.1:7878'
)

function Invoke-Sql {
    param([string]$Title, [string]$Sql)
    Write-Host ""
    Write-Host "## $Title"
    docker exec -i $Container psql -U postgres -d $Database -X -P pager=off -c $Sql
}

Write-Host ("snapshot at {0} UTC" -f (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd HH:mm:ss'))

Invoke-Sql 'Spider state per domain' @"
SELECT s.listing_source_name AS source,
       d.listing_source_domain AS domain,
       d.url_pattern_state AS pattern_state,
       left(coalesce(d.url_pattern, ''), 60) AS pattern,
       to_char(d.last_crawled, 'HH24:MI') AS crawled,
       d.last_crawl_outcome AS outcome,
       d.crawl_truncation_count AS trunc,
       d.crawl_failure_count AS fails,
       d.last_crawl_error_kind AS crawl_error,
       to_char(d.next_crawl_at, 'MM-DD HH24:MI') AS next_crawl,
       d.scrape_failure_streak AS streak,
       d.last_scrape_error_kind AS scrape_error,
       d.last_scrape_status_code AS code,
       to_char(d.next_scrape_at, 'HH24:MI') AS next_scrape
FROM listing_source_domains d
JOIN listing_sources s USING (listing_source_id)
WHERE s.crawl_enabled
ORDER BY s.listing_source_name;
"@

Invoke-Sql 'URLs per domain: class, disposition, scrape outcome' @"
SELECT d.listing_source_domain AS domain,
       count(*) AS urls,
       count(*) FILTER (WHERE u.url_class = 'product') AS product,
       count(*) FILTER (WHERE u.url_class = 'category') AS category,
       count(*) FILTER (WHERE u.url_class = 'other') AS other,
       count(*) FILTER (WHERE u.url LIKE '%?%') AS with_query,
       count(*) FILTER (WHERE u.crawler_disposition = 'DORMANT_SOLD') AS sold,
       count(*) FILTER (WHERE u.last_scraped IS NOT NULL) AS scraped,
       count(*) FILTER (WHERE u.last_captured_raw_input_sha256 IS NOT NULL) AS captured,
       count(*) FILTER (WHERE u.failure_count > 0) AS failing,
       count(*) FILTER (WHERE u.next_retry_at > now()) AS waiting
FROM listing_source_urls u
JOIN listing_source_domains d USING (domain_id)
GROUP BY d.listing_source_domain
ORDER BY urls DESC;
"@

Invoke-Sql 'URL error kinds (product URLs with a failure)' @"
SELECT d.listing_source_domain AS domain,
       u.last_error_kind AS error_kind,
       u.last_status_code AS code,
       count(*) AS urls,
       left(min(u.last_error_message), 90) AS sample_message
FROM listing_source_urls u
JOIN listing_source_domains d USING (domain_id)
WHERE u.url_class = 'product' AND u.last_error_kind IS NOT NULL
GROUP BY 1, 2, 3
ORDER BY urls DESC
LIMIT 40;
"@

Invoke-Sql 'LLM budget and schemas per source' @"
SELECT s.listing_source_name AS source,
       s.llm_calls_count AS llm_calls,
       CASE WHEN p.listing_source_id IS NULL THEN 0 ELSE jsonb_array_length(p.product_schema) END AS schemas,
       to_char(p.updated, 'HH24:MI') AS schema_updated,
       (SELECT count(*) FROM crawler_reviews r WHERE r.listing_source_id = s.listing_source_id AND r.status = 'PENDING_REVIEW') AS pending_reviews
FROM listing_sources s
LEFT JOIN listing_source_product_schemas p USING (listing_source_id)
WHERE s.crawl_enabled
ORDER BY llm_calls DESC, source;
"@

Invoke-Sql 'Reviews' @"
SELECT to_char(r.created, 'HH24:MI') AS created,
       s.listing_source_name AS source,
       r.artifact_type AS artifact,
       r.status,
       left(r.reason, 50) AS reason,
       r.candidate_version AS v,
       (SELECT count(*) FROM crawler_review_pages p WHERE p.review_id = r.review_id) AS pages
FROM crawler_reviews r
JOIN listing_sources s USING (listing_source_id)
ORDER BY r.created DESC
LIMIT 30;
"@

Write-Host ""
Write-Host "## Review console"
try {
    $reviews = Invoke-RestMethod -Uri "$Console/api/reviews" -TimeoutSec 10
    $pending = @($reviews | Where-Object { $_.status -eq 'PENDING_REVIEW' })
    Write-Host ("{0} reviews listed, {1} pending" -f @($reviews).Count, $pending.Count)
    foreach ($review in $pending) {
        Write-Host ("  {0}  {1}  {2}  {3}" -f $review.review_id, $review.listing_source_name, $review.artifact_type, $review.reason)
    }
} catch {
    Write-Host "console unreachable at $Console ($($_.Exception.Message))"
}

if (Test-Path 'scraped_raw_observations.json') {
    $size = (Get-Item 'scraped_raw_observations.json').Length
    Write-Host ("raw observations file: {0:N0} bytes" -f $size)
}
