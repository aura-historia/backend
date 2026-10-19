# Approve, reject or mark a review as needing repair through the local review console.
# Only for observing runtime behaviour behind a blocking review during a dev run; record every use.
# The dev binary accepts same-origin mutations without a token, so the Origin header is required.
#
# Usage: .\review-action.ps1 -ReviewId cr_01... -Action approve|reject|needs-repair [-Notes "soak: observe runtime"]
#        .\review-action.ps1 -ReviewId cr_01... -Action matrix      # re-run the deterministic gate on live pages
param(
    [Parameter(Mandatory = $true)][string]$ReviewId,
    [Parameter(Mandatory = $true)][ValidateSet('approve', 'reject', 'needs-repair', 'matrix', 'show')][string]$Action,
    [string]$Notes = 'dev verification',
    [string]$Console = 'http://127.0.0.1:7878'
)

$headers = @{ Origin = $Console }

switch ($Action) {
    'show' {
        Invoke-RestMethod -Uri "$Console/api/reviews/$ReviewId" -Headers $headers | ConvertTo-Json -Depth 8
    }
    'matrix' {
        Invoke-RestMethod -Uri "$Console/api/reviews/$ReviewId/matrix?refresh=true" -Headers $headers -TimeoutSec 120 | ConvertTo-Json -Depth 8
    }
    default {
        $body = @{ notes = $Notes } | ConvertTo-Json
        $result = Invoke-RestMethod -Method Post -Uri "$Console/api/reviews/$ReviewId/$Action" -Headers $headers -ContentType 'application/json' -Body $body
        Write-Host ("{0} {1} at {2} UTC: {3}" -f $Action, $ReviewId, (Get-Date).ToUniversalTime().ToString('HH:mm:ss'), ($result | ConvertTo-Json -Compress))
    }
}
