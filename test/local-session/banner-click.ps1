param(
    [string]$HostExe = "",
    [string]$Work = (Join-Path $env:TEMP "rdr-banner-click"),
    [int]$X = -1, [int]$Y = -1
)
. "$PSScriptRoot\lib.ps1"

$s = $null
$failed = $true
try {
    $s = Start-Session -Work $Work -HostExe $HostExe
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
