# Access levels across two machines: this PC's viewer controls the Windows
# test VM through the real rendezvous server, in a one-time support session.
# The VM runs the build under test (target/release, installed as the service
# copies it); the "tab strip" window there logs the clicks that arrive.
#
#   cargo build --release --workspace
#   cd test/vm && vagrant up         # then, once per build:
#   vagrant winrm -c '& C:\rider\rider-host.exe service install'
#   powershell -ExecutionPolicy Bypass -File test\vm\access-levels.ps1 -Server rdm.example.com -Fingerprint <hex>
#
# -Client picks the viewer (default: this build's); try an installed older one too.
param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = ""
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
$vm = $PSScriptRoot

. "$PSScriptRoot\vmlib.ps1"   # Guest: run a guest/ script in the VM
function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    throw "timed out waiting for '$pattern' in $file"
}
function Clicks() { @(Get-Content (Join-Path $guest "clicks.out") -ErrorAction SilentlyContinue | Where-Object { $_ }).Count }
# The VM reads this file as it changes: retry while it holds it.
function Command([string]$line) {
    for ($i = 0; $i -lt 50; $i++) {
        try { [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "$line`r`n"); return } catch { Start-Sleep -Milliseconds 100 }
    }
    throw "cannot write support.cmd"
}

$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}

$viewer = $null
try {
    Remove-Item (Join-Path $guest "screen.out"), (Join-Path $guest "support.out") -ErrorAction SilentlyContinue
    Set-Content (Join-Path $guest "support.args") "$Server $Fingerprint"
    Guest "start-tests.ps1"
    $size = (Wait-File "screen.out" '^\d+ \d+$') -split ' '
    $vw = [int]$size[0]; $vh = [int]$size[1]
    $code = ((Wait-File "support.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"') | ConvertFrom-Json).code
    Write-Host "VM screen ${vw}x${vh}; support code $code; viewer $Client"

    $env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $env1["RIDER_LANG"] = "en"   # the checks read English text
    $viewer = New-Object Rdr+Proc $Client, "quick $code", $env1
    Wait-File "support.out" '"support_request"' | Out-Null
    Command "y view"
    Wait-File "support.out" '"support_started"' | Out-Null
    $end = (Get-Date).AddSeconds(20)
    do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
    if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
    [Rdr]::Place($win, 100, 100, 1280, 800)
    # First frames (the title counts them), then the toolbar's intro is over.
    Wait-Until { [Rdr]::Title($win) -match ' [1-9]\d* fps' } 20 | Out-Null
    Start-Sleep 3

    # On the tab strip, away from the banner and the viewer's toolbar tab.
    $hx = [int]($vw / 2 - 360); $hy = 150
    $s = @{ Viewer = $win }
    $title = [Rdr]::Title($win)
    # A dropped click can only be seen by waiting a moment.
    [Rdr]::ClickHostPixel($win, $hx, $hy, $vw, $vh) | Out-Null; Start-Sleep 2
    Check "view only: the click is dropped" ((Clicks) -eq 0)
    Write-Host "  viewer title: $title"
    Check "view only: the viewer says so" ($title -like "*View only*" -or $Client -notlike "*target*")

    Command "access full"
    Wait-File "support.out" '"access":"full"' | Out-Null
    Start-Sleep 1
    [Rdr]::ClickHostPixel($win, $hx, $hy, $vw, $vh) | Out-Null
    Check "full access: the click arrives" ((Wait-Until { (Clicks) -ge 1 } 5) -and (Clicks) -eq 1)
    # On the (hidden) banner: goes through to the tab strip, session stays.
    [Rdr]::ClickHostPixel($win, [int]($vw / 2 + 150), 17, $vw, $vh) | Out-Null
    Check "full access: a click on the banner spot reaches the window under it" ((Wait-Until { (Clicks) -ge 2 } 5) -and (Clicks) -eq 2)

    Command "access view"
    Wait-File "support.out" '"access":"view"' | Out-Null
    Start-Sleep 1
    [Rdr]::ClickHostPixel($win, $hx, $hy, $vw, $vh) | Out-Null; Start-Sleep 2
    Check "back to view only: the click is dropped" ((Clicks) -eq 2)
    $ended = Get-Content (Join-Path $guest "support.out") | Where-Object { $_ -match '"support_ended"' }
    Check "the session is still up" (-not $viewer.P.HasExited -and -not $ended)
} finally {
    if ($viewer) { $viewer.Stop() }
    Guest "stop-tests.ps1"
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
