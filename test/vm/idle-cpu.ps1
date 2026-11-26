param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = "",
    [double]$MaxCorePercent = 5
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
. "$PSScriptRoot\vmlib.ps1"

function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    throw "timed out waiting for '$pattern' in $file"
}
$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function CorePercent($p, [int]$seconds) {
    $p.Refresh(); $a = $p.TotalProcessorTime; $t = [Diagnostics.Stopwatch]::StartNew()
    Start-Sleep $seconds
    $p.Refresh()
    100 * ($p.TotalProcessorTime - $a).TotalMilliseconds / $t.ElapsedMilliseconds
}

Remove-Item (Join-Path $guest "screen.out"), (Join-Path $guest "support.out"), (Join-Path $guest "pattern.png") -ErrorAction SilentlyContinue
Set-Content (Join-Path $guest "support.args") "$Server $Fingerprint"
Guest "start-sharp.ps1" | Out-Null
$code = ((Wait-File "support.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"') | ConvertFrom-Json).code
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["RIDER_LANG"] = "en"
$viewer = New-Object Rdr+Proc $Client, "quick $code", $env1
try {
    Wait-File "support.out" '"support_request"' | Out-Null
    [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "y view`r`n")
    Wait-File "support.out" '"support_started"' | Out-Null
    $end = (Get-Date).AddSeconds(20)
    do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
    if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
    foreach ($size in @(@(1280, 720), @(1900, 1000))) {
        [Rdr]::PlaceClient($win, 40, 40, $size[0], $size[1]) | Out-Null
        Start-Sleep 8
        $cpu = CorePercent $viewer.P 15
        Check ("viewer {0}x{1}, still screen: {2:N1}% of one core (max {3}%)" -f $size[0], $size[1], $cpu, $MaxCorePercent) ($cpu -lt $MaxCorePercent)
    }
    Check "the session is still up" (-not $viewer.P.HasExited)
} finally {
    if (-not $viewer.P.HasExited) { $viewer.P.Kill() }
    Guest "stop-tests.ps1" | Out-Null
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
