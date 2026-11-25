# Enterprise mode, installing by Group Policy (Milestone 7, phase 5), across
# this PC and the Windows test VM. The VM becomes a new company computer:
# no app, no host config. Its share \\localhost\rdr holds the company
# installer (target\installer) and packaging\windows\enterprise\rdr-startup.ps1,
# which runs as SYSTEM the way a GPO startup script does. The company server
# is the real installation in Docker (test/server/server.sh). Checks: the
# script installs, turns on remote access and joins (with -Unattended); the
# computer is online in the admin's list; the employee accepts a session;
# a second start does nothing, quickly. The VM's usual host (C:\rider build,
# its config) is put back afterwards.
#
#   packaging\windows\build.ps1 -Version 0.4.90   (any version; the latest installer is used)
#   bash test/server/server.sh build
#   cd test/vm && vagrant up
#   powershell -ExecutionPolicy Bypass -File test\vm\enterprise-install.ps1
#
# Exit code 0: every check passed.
param([string]$Work = (Join-Path $env:TEMP "rdr-vm-enterprise-install"))
. "$PSScriptRoot\entlib.ps1"

$setup = Get-ChildItem (Join-Path $repo "target\installer") -Filter "rdr-setup-*.exe" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $setup) { throw "no installer: run packaging\windows\build.ps1" }
$version = $setup.BaseName -replace '^rdr-setup-', ''
$gpo = Join-Path $guest "gpo"
New-Item -ItemType Directory -Force $gpo | Out-Null
Copy-Item $setup.FullName (Join-Path $gpo "rdr-setup.exe") -Force
Copy-Item (Join-Path $repo "packaging\windows\enterprise\rdr-startup.ps1") $gpo -Force

try {
    $signin = New-Company
    if ($signin.First.Code) { throw "admin sign-in failed: $($signin.First.Text)" }
    Guest "ent-gpo.ps1 fresh"
    Check "the VM is a new computer ($(GuestOut 'ent-gpo.out'))" ((GuestOut "ent-gpo.out") -eq "fresh: app=False config=False")

    Guest "ent-gpo.ps1 startup unattended" 300
    $o = GuestOut "ent-gpo.out"
    Write-Host ($o -join "`n")
    Check "the startup script installed $version" ($o -contains "version $version")
    Check "remote access is on (service running)" ($o -contains "service Running")
    Check "it joined, unattended-capable" ([bool]($o -match '^Company:\s+TestCo, unattended-capable'))
    Check "the startup log says ok" ([bool]($o -match 'ok: version'))

    $pc = Wait-Online
    Check "it is online in the admin's list ('$($pc.name)', user '$($pc.user)')" ($pc -and $pc.user -eq "vagrant")
    if (-not $pc) { throw "the new computer did not come online" }
    # Unattended-capable, but the admin has not switched it on: still asks.
    $r = Try-Connect $pc.fingerprint "accept"
    Check "the employee is asked and accepts; the session opens" ((GuestOut "ent-answer.out") -contains "asked False" -and $r.Connected)

    Guest "ent-gpo.ps1 startup unattended" 300
    $o = GuestOut "ent-gpo.out"
    $seconds = [double](($o | Where-Object { $_ -like "seconds *" }) -replace 'seconds ', '')
    $installs = @($o | Where-Object { $_ -match 'installing ' }).Count
    Check "the next start does nothing ($seconds s, $installs install(s) in the log)" ($installs -eq 1 -and $seconds -lt 15)
} finally {
    Guest "ent-gpo.ps1 restore" 300
    Write-Host "VM: $(GuestOut 'ent-gpo.out')"
    Server "down" | Out-Null
}
Finish 8
