# Print the Rust snippets to paste into src/crawler/src/bin/dev.rs for a dev run.
# Input CSV has no header: name,slug,domain (domain is the apex without www.).
# StartIndex must be above every index already used in crawler_demo, because the demo
# ListingSource ID is derived from it (01890a5d-ac96-774b-af1d-d5586c639f{index:02x}).
#
# Usage: .\gen-demo-shops.ps1 -Csv shops.csv -StartIndex 48
param(
    [Parameter(Mandatory = $true)][string]$Csv,
    [Parameter(Mandatory = $true)][int]$StartIndex
)

if ($StartIndex -lt 1 -or $StartIndex -gt 255) { throw "StartIndex must be 1..255 (two hex digits)" }

$rows = Import-Csv -Path $Csv -Header name, slug, domain | Where-Object { $_.domain }
if (-not $rows) { throw "no rows in $Csv" }
if ($StartIndex + $rows.Count - 1 -gt 255) { throw "index range exceeds 255" }

foreach ($row in $rows) {
    if ($row.slug -cnotmatch '^[a-z0-9]+(-[a-z0-9]+)*$') { throw "slug '$($row.slug)' must be lowercase kebab-case" }
    if ($row.domain -cmatch '^www\.|/|[A-Z]') { throw "domain '$($row.domain)' must be a lowercase apex without www. or path" }
}

Write-Output "// --- demo_listing_sources(): replace the tuple array, and format the ID with {index:02x} ---"
Write-Output "    ["
$index = $StartIndex
foreach ($row in $rows) {
    Write-Output ('        ({0}, "{1}", "{2}"),' -f $index, $row.name.Replace('"', '\"'), $row.slug)
    $index++
}
Write-Output "    ]"
Write-Output ""
Write-Output '// --- ID format line: uuid::Uuid::parse_str(&format!("01890a5d-ac96-774b-af1d-d5586c639f{index:02x}")) ---'
Write-Output ""
Write-Output "// --- domain list for the zip([...]) in main(), same order ---"
Write-Output "    ["
foreach ($row in $rows) { Write-Output ('        "{0}",' -f $row.domain) }
Write-Output "    ]"
Write-Output ""
Write-Output ("// indices {0}..{1}; next round starts at {2}" -f $StartIndex, ($index - 1), $index)
