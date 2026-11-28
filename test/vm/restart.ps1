param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = "",
    [string]$Pin = "rider-test-pin",
    [int]$BackWithin = 600,
    [int]$HangAfter = 300,
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-restart")
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
$vbox = Join-Path $env:ProgramFiles "Oracle\VirtualBox\VBoxManage.exe"
. "$PSScriptRoot\vmlib.ps1"
Add-Type -AssemblyName System.Drawing
[Rdr]::DpiAware()

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
$nudge = 0
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function Boot-Time {
    Remove-Item (Join-Path $guest "boot.out") -ErrorAction SilentlyContinue
    Guest "boot-time.ps1" 600
    (Wait-File "boot.out" '^\d{4}-' 30).Trim()
}
function Cold-Start {
    $ErrorActionPreference = "Continue"
    Start-Sleep 20
    & $vbox controlvm rider-test-host poweroff 2>&1 | Out-Null
    Push-Location $PSScriptRoot
    try { & vagrant up 2>&1 | Out-Null } finally { Pop-Location }
}
function Pixel([int]$x, [int]$y) {
    $img = New-Object System.Drawing.Bitmap 1, 1
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen($x, $y, 0, 0, $img.Size)
    $img.GetPixel(0, 0)
}
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices;
public static class Burst {
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    static IntPtr At(int x, int y) { return new IntPtr((y << 16) | (x & 0xFFFF)); }
    public static void Tap(IntPtr h, int x, int y) {
        PostMessage(h, 0x0200, IntPtr.Zero, At(x, y));
        PostMessage(h, 0x0201, new IntPtr(1), At(x, y));
        PostMessage(h, 0x0202, IntPtr.Zero, At(x, y));
    }
}
"@
function Tap([IntPtr]$win, [int]$x, [int]$y) { [Burst]::Tap($win, $x, $y) }
function Strip([IntPtr]$win, [double]$scale) {
    $c = [Rdr]::Client($win)
    $img = New-Object System.Drawing.Bitmap $c[2], ([int](36 * $scale))
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen($c[0], $c[1], 0, 0, $img.Size)
    $img
}
function Labels([IntPtr]$win, [double]$scale) {
    $img = Strip $win $scale
    $img.Save((Join-Path $Work "toolbar.png"))
    $rows = [int](10 * $scale)..[int](26 * $scale)
    $gap = 20 * $scale
    $clusters = @(); $first = -1; $last = -1
    for ($x = 0; $x -lt $img.Width; $x++) {
        $ink = $false
        foreach ($y in $rows) { $p = $img.GetPixel($x, $y); if ($p.R -gt 170 -and $p.G -gt 170 -and $p.B -gt 170) { $ink = $true; break } }
        if (-not $ink) { continue }
        if ($first -ge 0 -and $x - $last -gt $gap) { $clusters += [int](($first + $last) / 2); $first = -1 }
        if ($first -lt 0) { $first = $x }
        $last = $x
    }
    if ($first -ge 0) { $clusters += [int](($first + $last) / 2) }
    , $clusters
}
function Is-Red([IntPtr]$win, [double]$scale, [int]$x) {
    $p = (Strip $win $scale).GetPixel($x, [int](6 * $scale))
    $p.R -gt 150 -and $p.G -lt 90 -and $p.B -lt 80
}
function Tap-Until([IntPtr]$win, [int]$x, [int]$y, [scriptblock]$done) {
    for ($i = 0; $i -lt 6; $i++) {
        $script:nudge = ($script:nudge + 1) % 4
        Tap $win ($x + $script:nudge) $y
        if (Wait-Until $done 2) { return $true }
    }
    $false
}
function Press-Restart([IntPtr]$win, [double]$scale) {
    $labels = Labels $win $scale
    if ($labels.Count -lt 3) { return $false }
    $x = $labels[$labels.Count - 2]; $y = [int](18 * $scale)
    if (-not (Tap-Until $win $x $y { Is-Red $win $scale $x })) { return $false }
    (Strip $win $scale).Save((Join-Path $Work "toolbar-asked.png"))
    Tap-Until $win $x $y { [Rdr]::Title($win) -match 'restarts in 15 s' }
}

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["APPDATA"] = $Work; $env1["RIDER_LANG"] = "en"
$viewer = $null; $pairJob = $null
try {
    $c = New-Object Rdr+Proc $Client, "set-server $Server $Fingerprint", $env1
    $c.P.WaitForExit(15000) | Out-Null
    Remove-Item (Join-Path $guest "pair.out") -ErrorAction SilentlyContinue
    $pairJob = Guest-Start "pair-auto.ps1"
    $code = ((Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120) | ConvertFrom-Json).code
    $pair = New-Object Rdr+Proc $Client, "pair $code --name vm", $env1
    Wait-File "pair.out" '"event":"request"' 30 | Out-Null
    Start-Sleep 1
    $pair.Send("y")
    $pair.P.WaitForExit(60000) | Out-Null
    Wait-File "pair.out" '^exit=' 30 | Out-Null
    Check "paired with the VM" ($pair.P.ExitCode -eq 0)
    Start-Sleep 3

    $config = Join-Path $Work "RemoteDesktopRider\client\client.toml"
    $toml = Get-Content $config -Raw
    if ($toml -notmatch 'pin_toolbar = false') { throw "unexpected viewer config: $toml" }
    Set-Content $config ($toml -replace 'pin_toolbar = false', 'pin_toolbar = true') -NoNewline
    $viewer = New-Object Rdr+Proc $Client, "connect vm --pin-stdin", $env1
    $viewer.Send($Pin)
    $end = (Get-Date).AddSeconds(40)
    do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end -and -not $viewer.P.HasExited)
    if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
    [Rdr]::Place($win, 100, 100, 1200, 900)
    Wait-Until { [Rdr]::Title($win) -match ' [1-9]\d* fps' } 20 | Out-Null
    Start-Sleep 2

    $scale = [Rdr]::GetDpiForSystem() / 96
    $boot = Boot-Time

    Remove-Item (Join-Path $guest "restart-panel.out") -ErrorAction SilentlyContinue
    Guest "run-as-user.ps1 restart-panel.ps1"
    Wait-File "restart-panel.out" '^panel' 60 | Out-Null
    Check "full access: Restart next to Disconnect asks once, then restarts" (Press-Restart $win $scale)
    Check "the viewer is told about the 15 s warning" ([bool](Wait-Until { [Rdr]::Title($win) -match 'restarts in 15 s' } 10))
    $clicked = Wait-File "restart-panel.out" '^(cancel-clicked|error)' 30
    Check "the host shows a Cancel button" ($clicked -eq "cancel-clicked")
    Check "Cancel on the host reaches the viewer" ([bool](Wait-Until { [Rdr]::Title($win) -match 'restart was cancelled' } 10))
    Start-Sleep 20
    Check "after Cancel the computer does not restart" ((Boot-Time) -eq $boot -and -not $viewer.P.HasExited)

    Check "Restart can be pressed again" (Press-Restart $win $scale)
    $asked = Get-Date
    Check "a second restart is announced" ([bool](Wait-Until { [Rdr]::Title($win) -match 'restarts in 15 s' } 10))
    Start-Sleep 15
    Check "Windows shuts down after the warning" ([bool](Wait-Until { -not (Agent-Alive) -and -not (Wait-Until { Agent-Alive } 15) } 120))
    $down = Get-Date
    Write-Host ("  shutting down {0:0} s after the click" -f ($down - $asked).TotalSeconds)
    $back = Wait-Until { $viewer.All() -match 'reconnected after' } $HangAfter
    if (-not $back -and -not (Agent-Alive)) {
        Write-Host "  the VM did not come back by itself in $HangAfter s: power-cycling it"
        Cold-Start
        $back = Wait-Until { $viewer.All() -match 'reconnected after' } ($BackWithin - $HangAfter)
    }
    if ($back) { Write-Host ("  viewer back {0:0} s after Windows went down" -f ((Get-Date) - $down).TotalSeconds) }
    Check "the viewer reconnects by itself within $BackWithin s" ([bool]$back)
    Remove-Item (Join-Path $guest "shutdown-events.out") -ErrorAction SilentlyContinue
    Guest "shutdown-events.ps1" 300
    Check "Windows logged the restart as started by the host" ([bool](Get-Content (Join-Path $guest "shutdown-events.out") | Where-Object { $_ -match ' 1074 .*rider-host\.exe' }))
    if (@($results | Where-Object Result -eq "FAIL").Count) {
        $viewer.All() | Set-Content (Join-Path $Work "viewer.log")
        Write-Host "  --- viewer said:"; ($viewer.All() -split "`n" | Select-Object -Last 15) | ForEach-Object { Write-Host "    $_" }
        Write-Host "  --- panel:"; Get-Content (Join-Path $guest "restart-panel.out") -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "    $_" }
    }
} finally {
    if ($viewer) { $viewer.Stop() }
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    Guest "stop-tests.ps1"
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
