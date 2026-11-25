param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$Token,
    [switch]$Unattended
)
$ErrorActionPreference = "Stop"
$log = Join-Path $env:windir "Temp\rdr-startup.log"
function Log([string]$text) {
    $line = "{0:yyyy-MM-dd HH:mm:ss}  {1}" -f (Get-Date), $text
    $old = @(Get-Content $log -ErrorAction SilentlyContinue | Select-Object -Last 199)
    Set-Content $log ($old + $line) -Encoding UTF8
}

$uninstallKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\{80CDF23F-6BC2-46BF-AEF6-6515FE750049}_is1"
$hostExe = Join-Path $env:ProgramFiles "RemoteDesktopRider\rider-host.exe"
$hostConfig = Join-Path $env:ProgramData "RemoteDesktopRider\host\host.toml"

function Version3([string]$text) { $v = [version]$text; [version]::new($v.Major, $v.Minor, [math]::Max($v.Build, 0)) }
function Installed-Version {
    $v = (Get-ItemProperty $uninstallKey -ErrorAction SilentlyContinue).DisplayVersion
    if ($v -and (Test-Path $hostExe)) { Version3 $v } else { $null }
}
function Offered-Version { Version3 (Get-Item $Installer).VersionInfo.ProductVersion }
function Joined { [bool](Select-String -Path $hostConfig -Pattern '^\[company\]' -Quiet -ErrorAction SilentlyContinue) }
function Wait-Share {
    for ($i = 0; $i -lt 12; $i++) {
        if (Test-Path $Installer) { return $true }
        Start-Sleep 5
    }
    $false
}

try {
    $installed = Installed-Version
    if ($installed -and (Joined)) {
        if (-not (Test-Path $Installer)) { return }
        if ((Offered-Version) -le $installed) { return }
    } elseif (-not (Wait-Share)) {
        Log "cannot read $Installer (network not ready, or the share is not readable by computer accounts)"
        return
    }
    $offered = Offered-Version
    $arguments = @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART")
    if (-not $installed -or $offered -gt $installed) {
        $arguments += "/ENROLL=$Token"
        if ($Unattended) { $arguments += "/UNATTENDED" }
        Log "installing $offered (installed: $(if ($installed) { $installed } else { 'none' }))"
        $p = Start-Process -FilePath $Installer -ArgumentList $arguments -Wait -PassThru
        if ($p.ExitCode) { Log "the installer failed: exit code $($p.ExitCode)"; return }
    }
    for ($try = 1; -not (Joined) -and $try -le 3; $try++) {
        if ($try -gt 1) { Start-Sleep 20 }
        $joinArgs = @("enroll", $Token)
        if ($Unattended) { $joinArgs += "--unattended" }
        $out = & $hostExe @joinArgs 2>&1 | Out-String
        Log "join, try ${try}: $($out.Trim())"
    }
    if (-not (Joined)) { return }
    if (-not (Get-Service RemoteDesktopRider -ErrorAction SilentlyContinue)) {
        $out = & $hostExe service install 2>&1 | Out-String
        Log "remote access: $($out.Trim())"
    }
    Log "ok: version $(Installed-Version), in the company"
} catch {
    Log "error: $_"
}
