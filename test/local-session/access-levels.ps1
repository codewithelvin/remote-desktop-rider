# End-to-end check of access levels on this PC (see lib.ps1): the person at
# the controlled computer allows "View only", "Standard" or "Full access" and
# changes it during the session. The host must enforce it whatever the viewer
# is: clicks are tried with this build's viewer and with an installed (older)
# viewer that knows nothing about access levels. Moves the pointer.
#
#   cargo build --profile quick -p rider-host -p rider-client -p rider-server
#   powershell -ExecutionPolicy Bypass -File test\local-session\access-levels.ps1
#
# Exit code 0: every check passed.
param(
    [string]$Work = (Join-Path $env:TEMP "rdr-access-levels"),
    [string]$OldClient = "C:\Program Files\RemoteDesktopRider\rider-client.exe"
)
. "$PSScriptRoot\lib.ps1"

$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
# On the tab strip, away from the banner and the viewer's toolbar tab.
$hx = [int]($sw / 2 - 360); $hy = 150

function Run-Levels([string]$label, [string]$client) {
    Write-Host "== $label"
    $s = $null
    try {
        $s = Start-Session -Work $Work -ClientExe $client -Answer "y view"
        $title = [Rdr]::Title($s.Viewer)
        Click-Host $s $hx $hy
        Check "$label, view only: the click is dropped" ([Rdr]::Clicks -eq 0)
        if (-not $client) { Check "$label, view only: the viewer says so ('$title')" ($title -like "*View only*") }

        $s.Host.Send("access full")
        Wait-For $s.Host '"access":"full"' | Out-Null
        Start-Sleep 1
        Click-Host $s $hx $hy
        Check "$label, changed to full access: the click arrives" ([Rdr]::Clicks -eq 1)

        $s.Host.Send("access view")
        Wait-For $s.Host '"access":"view"' | Out-Null
        Start-Sleep 1
        Click-Host $s $hx $hy
        Check "$label, back to view only: the click is dropped" ([Rdr]::Clicks -eq 1)
        Check "$label, the session is still up" (-not $s.Client.P.HasExited -and -not $s.Host.Find('"support_ended"'))
    } finally {
        Stop-Session $s
    }
}

Run-Levels "this viewer" ""
if (Test-Path $OldClient) {
    Run-Levels "installed viewer $((Get-Item $OldClient).VersionInfo.ProductVersion)" $OldClient
} else {
    Write-Host "no installed viewer at ${OldClient}: skipped the older-viewer run"
}

# This viewer with an installed (older) host, which has no access levels:
# everything works as before.
$oldHost = Join-Path (Split-Path $OldClient) "rider-host.exe"
if (Test-Path $oldHost) {
    $label = "installed host $((Get-Item $oldHost).VersionInfo.ProductVersion) with this viewer"
    Write-Host "== $label"
    $s = $null
    try {
        $s = Start-Session -Work $Work -HostExe $oldHost -Answer "y"
        Click-Host $s $hx $hy
        Check "${label}: the click arrives" ([Rdr]::Clicks -eq 1)
        Check "${label}: no access level in the title" (-not ([Rdr]::Title($s.Viewer) -like "*[[]*"))
    } finally {
        Stop-Session $s
    }
}

$failed = @($results | Where-Object Result -eq "FAIL").Count
Write-Host ""
$results | Format-Table -AutoSize | Out-String | Write-Host
if ($failed) { Write-Host "FAIL: $failed check(s)"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
