param([Parameter(Mandatory)][string]$Mode, [string]$Flag = "")
$ErrorActionPreference = "Stop"
$out = "C:\rider-test\ent-gpo.out"
$pf = "C:\Program Files\RemoteDesktopRider"
$hostDir = "C:\ProgramData\RemoteDesktopRider\host"
$keep = "C:\ent-gpo-backup"
$share = "C:\rdr-share"
$uninstallKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{80CDF23F-6BC2-46BF-AEF6-6515FE750049}_is1"

function Remove-App {
    $uninst = Join-Path $pf "unins000.exe"
    if (Test-Path $uninst) {
        Start-Process $uninst -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART" -Wait
        for ($i = 0; $i -lt 60 -and (Test-Path $uninst); $i++) { Start-Sleep -Milliseconds 500 }
    } elseif (Test-Path "$pf\rider-host.exe") {
        & "$pf\rider-host.exe" service uninstall 2>&1 | Out-Null
    }
    Get-Process RemoteDesktopRider, rider-host, rider-client -ErrorAction SilentlyContinue | Stop-Process -Force
    Remove-Item -Recurse -Force $pf -ErrorAction SilentlyContinue
    Remove-Item $uninstallKey -Recurse -ErrorAction SilentlyContinue
}

switch ($Mode) {
    "fresh" {
        Remove-App
        if (-not (Test-Path $keep)) { Move-Item $hostDir $keep } else { Remove-Item -Recurse -Force $hostDir -ErrorAction SilentlyContinue }
        New-Item -ItemType Directory -Force $share | Out-Null
        Copy-Item C:\rider-test\gpo\* $share -Force
        if (-not (Get-SmbShare rdr -ErrorAction SilentlyContinue)) {
            New-SmbShare -Name rdr -Path $share -ReadAccess Everyone | Out-Null
        }
        Remove-Item "$env:windir\Temp\rdr-startup.log" -ErrorAction SilentlyContinue
        Set-Content $out "fresh: app=$(Test-Path $pf) config=$(Test-Path $hostDir)"
    }
    "startup" {
        $token = (Get-Content C:\rider-test\ent-token.txt -Raw).Trim()
        $a = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "\\localhost\rdr\rdr-startup.ps1",
            "-Installer", "\\localhost\rdr\rdr-setup.exe", "-Token", $token)
        if ($Flag -eq "unattended") { $a += "-Unattended" }
        $t = Measure-Command { & powershell.exe @a }
        if (Test-Path "$pf\rider-host.exe") { & "$pf\rider-host.exe" auto-update off 2>&1 | Out-Null }
        $info = if (Test-Path "$pf\rider-host.exe") { & "$pf\rider-host.exe" info 2>&1 | Out-String } else { "no rider-host" }
        $lines = @(
            "seconds $([math]::Round($t.TotalSeconds, 1))"
            "version $((Get-ItemProperty $uninstallKey -ErrorAction SilentlyContinue).DisplayVersion)"
            "service $((Get-Service RemoteDesktopRider -ErrorAction SilentlyContinue).Status)"
            "--- info"; $info.Trim()
            "--- startup log"; Get-Content "$env:windir\Temp\rdr-startup.log" -ErrorAction SilentlyContinue
            "--- enroll log"; Get-Content "$hostDir\enroll.log" -ErrorAction SilentlyContinue
        )
        Set-Content $out $lines -Encoding UTF8
    }
    "restore" {
        Remove-App
        if (Test-Path $keep) {
            Remove-Item -Recurse -Force $hostDir -ErrorAction SilentlyContinue
            Move-Item $keep $hostDir
        }
        Remove-SmbShare -Name rdr -Force -ErrorAction SilentlyContinue
        Remove-Item -Recurse -Force $share -ErrorAction SilentlyContinue
        & C:\rider-test\update-host.ps1 | Out-Null
        Set-Content $out "restored: service $((Get-Service RemoteDesktopRider).Status)"
    }
}
Get-Content $out
