# End-to-end check on this PC, no admin rights, nothing installed or changed:
# a remote click where the host's session banner is (top centre of the
# screen; the viewer cannot see it, it is excluded from capture) must reach
# the window underneath - a browser's tab strip, say - and must not end the
# session. See lib.ps1 for how the session is set up. Moves the pointer.
#
#   cargo build --profile quick -p rider-host -p rider-client -p rider-server
#   powershell -ExecutionPolicy Bypass -File test\local-session\banner-click.ps1
#   ... -HostExe "C:\Program Files\RemoteDesktopRider\rider-host.exe"   # an installed host
#
# Exit code 0: the click went through and the session stayed up.
param(
    [string]$HostExe = "",
    [string]$Work = (Join-Path $env:TEMP "rdr-banner-click"),
    # Host pixel to click (default: on the banner). For checking the harness.
    [int]$X = -1, [int]$Y = -1
)
. "$PSScriptRoot\lib.ps1"

$s = $null
$failed = $true
try {
    $s = Start-Session -Work $Work -HostExe $HostExe
    # On the banner (it is at least 520 px wide at the top centre), off the
    # middle so it misses the viewer's toolbar tab.
    $hx = [int]($sw / 2 + 150); $hy = 17
    if ($X -ge 0) { $hx = $X; $hy = $Y }
    Click-Host $s $hx $hy
    Start-Sleep 1
    $clicks = [Rdr]::Clicks
    $ended = $s.Host.Find('"support_ended"')
    $alive = -not $s.Client.P.HasExited
    Write-Host "tab strip clicks: $clicks (last at $([Rdr]::LastClick)); viewer running: $alive; host ended the session: $([bool]$ended)"
    if (-not $alive -or $env:RDR_VERBOSE) { Write-Host "client said:`n$($s.Client.All())`nhost said:`n$($s.Host.All())" }
    $failed = -not ($clicks -ge 1 -and $alive -and -not $ended)
} finally {
    Stop-Session $s
}
if ($failed) { Write-Host "FAIL: the remote click did not go through, or it ended the session"; exit 1 }
Write-Host "PASS: the remote click reached the window under the banner and the session stayed up"
exit 0
