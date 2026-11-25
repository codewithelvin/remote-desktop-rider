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
