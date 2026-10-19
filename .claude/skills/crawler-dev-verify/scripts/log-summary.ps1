# Summarize a dev run's JSON log: records by level and message, pass durations,
# spider outcomes per domain, LLM retries and heartbeat share.
# Lines that are not JSON (cargo output) are ignored.
#
# Usage: .\log-summary.ps1 -Log target/soak/2026-10-18/round-1.jsonl [-Top 25] [-Since 14:05]
param(
    [Parameter(Mandatory = $true)][string]$Log,
    [int]$Top = 25,
    [string]$Since
)

if (-not (Test-Path $Log)) { throw "log not found: $Log" }

$records = New-Object System.Collections.Generic.List[object]
$skipped = 0
# Open with shared read/write access: the dev binary keeps the file open while the round runs.
$stream = [System.IO.File]::Open((Resolve-Path $Log), [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
$reader = New-Object System.IO.StreamReader($stream)
try {
    while ($null -ne ($line = $reader.ReadLine())) {
        $trimmed = $line.Trim()
        if (-not $trimmed.StartsWith('{')) { $skipped++; continue }
        try { $records.Add(($trimmed | ConvertFrom-Json)) } catch { $skipped++ }
    }
} finally { $reader.Dispose() }
if ($Since) {
    $records = [System.Collections.Generic.List[object]]@($records | Where-Object { $_.timestamp -and $_.timestamp.Substring(11, 5) -ge $Since })
}
$total = $records.Count
if ($total -eq 0) { Write-Host "no JSON records in $Log ($skipped non-JSON lines)"; return }

function Field { param($record, [string]$name)
    if ($null -ne $record.fields -and $null -ne $record.fields.$name) { return $record.fields.$name }
    if ($null -ne $record.span -and $null -ne $record.span.$name) { return $record.span.$name }
    foreach ($span in @($record.spans)) { if ($null -ne $span.$name) { return $span.$name } }
    return $null
}
function Msg { param($record) if ($record.fields) { return $record.fields.message } return $record.message }
function Stamp { param($record) if ($record.timestamp) { return $record.timestamp.Substring(11, 8) } return '' }

$first = Stamp $records[0]; $last = Stamp $records[$total - 1]
Write-Host ("{0} records ({1} non-JSON lines skipped), {2} -> {3} UTC" -f $total, $skipped, $first, $last)

Write-Host ""; Write-Host "## Records by level"
$records | Group-Object level | Sort-Object Count -Descending | ForEach-Object { Write-Host ("  {0,-6} {1,6}" -f $_.Name, $_.Count) }

Write-Host ""; Write-Host "## Top messages"
$byMessage = $records | Group-Object { Msg $_ } | Sort-Object Count -Descending
$byMessage | Select-Object -First $Top | ForEach-Object {
    $share = [math]::Round(100.0 * $_.Count / $total, 1)
    Write-Host ("  {0,6}  {1,5}%  {2}" -f $_.Count, $share, $_.Name)
}
$heartbeat = ($byMessage | Where-Object { $_.Name -match 'raw_capture|heartbeat' } | Measure-Object Count -Sum).Sum
if ($heartbeat) { Write-Host ("  heartbeat-like records: {0} ({1}% of all)" -f $heartbeat, [math]::Round(100.0 * $heartbeat / $total, 1)) }

Write-Host ""; Write-Host "## Scheduler passes"
foreach ($record in $records | Where-Object { (Msg $_) -match 'scheduler pass complete' }) {
    $f = $record.fields
    $deferred = if ($null -ne $f.deferred) { "$($f.deferred)" } else { "$($f.deferred_urls) urls/$($f.deferred_domains) domains" }
    Write-Host ("  {0}  {1,-30} total={2} ok={3} failed={4} skipped={5} deferred={6} duration={7}s" -f (Stamp $record), (Msg $record), $f.total, $f.succeeded, $f.failed, $f.skipped, $deferred, [math]::Round(($f.duration_ms / 1000.0), 0))
}

Write-Host ""; Write-Host "## Spider outcomes"
foreach ($record in $records | Where-Object { (Msg $_) -match '^(Crawl completed successfully|Spider run failed|Found no product URL pattern)$' }) {
    $domain = Field $record 'domain'
    if (-not $domain) { $domain = Field $record 'listing_source_domain' }
    if (-not $domain) { $domain = Field $record 'crawl_root_url' }
    $f = $record.fields
    $detail = @()
    foreach ($name in 'error_kind', 'stage', 'sample_size', 'pages_crawled', 'product_urls', 'robots_blocked', 'robots_denied', 'initial_anti_bot', 'cooldown_seconds') {
        if ($null -ne $f.$name -and "$($f.$name)" -ne '' -and "$($f.$name)" -ne '0') { $detail += "$name=$($f.$name)" }
    }
    if ($f.error) { $detail += ('error=' + ("$($f.error)" -replace '\s+', ' ').Substring(0, [math]::Min(100, "$($f.error)".Length))) }
    Write-Host ("  {0}  {1,-32} {2,-28} {3}" -f (Stamp $record), $domain, (Msg $record), ($detail -join ' '))
}

Write-Host ""; Write-Host "## LLM retries and validation feedback"
$llm = @($records | Where-Object { (Msg $_) -match '^Retrying' })
if ($llm.Count -eq 0) { Write-Host "  none" }
$llm | Group-Object { "{0} | {1} | {2}" -f (Msg $_), (Field $_ 'operation'), ((Field $_ 'error_kind'), (Field $_ 'feedback_code') -ne $null -join '') } |
    Sort-Object Count -Descending | ForEach-Object { Write-Host ("  {0,4}  {1}" -f $_.Count, $_.Name) }

Write-Host ""; Write-Host "## Warnings and errors by message"
$records | Where-Object { $_.level -in 'WARN', 'ERROR' } | Group-Object { Msg $_ } | Sort-Object Count -Descending |
    Select-Object -First $Top | ForEach-Object { Write-Host ("  {0,5}  {1}" -f $_.Count, $_.Name) }

Write-Host ""; Write-Host "## Scraper failures by error text (first 100 chars)"
$records | Where-Object { (Msg $_) -eq 'Scraper run failed' } |
    Group-Object { $e = "$($_.fields.error)" -replace '\s+', ' '; $e -replace 'https?://\S+', '<url>' | ForEach-Object { $_.Substring(0, [math]::Min(100, $_.Length)) } } |
    Sort-Object Count -Descending | Select-Object -First $Top | ForEach-Object { Write-Host ("  {0,5}  {1}" -f $_.Count, $_.Name) }

Write-Host ""; Write-Host "## Performance summaries"
foreach ($record in $records | Where-Object { (Msg $_) -eq 'Performance summary' } | Select-Object -Last 10) {
    $f = $record.fields
    Write-Host ("  {0}  {1,-20} items={2} avg={3}s" -f (Stamp $record), $f.label, $f.items_processed, $f.avg_seconds)
}
