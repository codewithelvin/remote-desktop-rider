param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Pin = "rider-test-pin",
    [string]$Offsets = ""
)
$guest = Join-Path $PSScriptRoot "guest"
. "$PSScriptRoot\vmlib.ps1"
$outFile = Join-Path $guest "tunnel-panel.out"
function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    return $null
}
function Seen([string]$prefix) {
    Get-Content $outFile -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ -like "$prefix*" } | Select-Object -Last 1
}
$results = @()
function Check([string]$what, $ok) {
    $ok = [bool]$ok
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}

$token = "rdr-panel-" + [guid]::NewGuid().ToString("N").Substring(0, 12)
$pairJob = $null
try {
    Guest "stop-tests.ps1" | Out-Null
    Guest "update-host.ps1"
    Remove-Item (Join-Path $guest "pair.out"), $outFile, (Join-Path $guest "tunnel-panel.args"), (Join-Path $guest "tunnel-click") -ErrorAction SilentlyContinue
    Guest "run-as-user.ps1 tunnel-panel.ps1"
    $pairJob = Guest-Start "pair-auto.ps1"
    $line = Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120
    if (-not $line) { throw "no pairing code from the VM" }
    $code = ($line | ConvertFrom-Json).code
    Set-Content (Join-Path $guest "tunnel-panel.args") "$Server $Fingerprint $code $Pin $token" -NoNewline
    if ($Offsets) { Set-Content (Join-Path $guest "tunnel-click") $Offsets -NoNewline }
    if (-not (Wait-File "tunnel-panel.out" '^done' 600)) { throw "the VM's script did not finish" }
    Get-Content $outFile -Encoding UTF8 | ForEach-Object { Write-Host "  vm: $_" }

    Check "the viewer paired and connected" ((Seen "paired") -eq "paired 0")
    Check "a tunnel request brings the session panel up" ((Seen "panel-up") -eq "panel-up True")
    Check "the banner shows for each new address" ((Seen "banner-allow") -eq "banner-allow True" -and (Seen "banner-deny") -eq "banner-deny True")
    Check "clicking Allow on the banner lets the connection through" ((Seen "allow-got") -eq "allow-got $token")
    $deny = (Seen "deny-got") -replace '^deny-got ', ''
    $reply, $seconds = $deny -split '\|'
    Check "clicking Deny on the banner refuses it at once ('$deny')" ($reply -in @("", "refused") -and [double]$seconds -lt 15)
    Check "no errors in the VM" ($null -eq (Seen "error"))
} finally {
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    Guest "stop-tests.ps1" | Out-Null
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed -or $results.Count -ne 6) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
