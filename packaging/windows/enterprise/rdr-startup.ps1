# Remote Desktop Rider: Group Policy computer startup script for company
# computers (Enterprise mode). See docs/ENTERPRISE.md.
#
# Runs at every start of the computer, as SYSTEM, before anyone signs in.
# It installs the app from the share when it is missing or older than the
# installer there, and joins the company when the computer has not joined
# yet. Otherwise it does nothing (a second or two). It never installs an
# older version over a newer one (the app also updates itself).
#
# In the policy (Computer Configuration > Policies > Windows Settings >
# Scripts > Startup > PowerShell Scripts), give it these parameters:
#
#   -Installer \\your-domain\NETLOGON\rdr\rdr-setup.exe -Token rdr1.... [-Unattended]
#
#   -Installer   the company installer on a share the computers can read
#   -Token       the join token from the admin's console (Copy join token)
#   -Unattended  the admin may connect without anyone accepting, once the
#                admin also switches it on for the computer (servers, kiosks)
#
# What it did each time: %windir%\Temp\rdr-startup.log (the last 200 lines).
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

# major.minor.patch only: the installer file says 0.4.3.0, Windows' list 0.4.3.
function Version3([string]$text) { $v = [version]$text; [version]::new($v.Major, $v.Minor, [math]::Max($v.Build, 0)) }
function Installed-Version {
    $v = (Get-ItemProperty $uninstallKey -ErrorAction SilentlyContinue).DisplayVersion
    if ($v -and (Test-Path $hostExe)) { Version3 $v } else { $null }
}
function Offered-Version { Version3 (Get-Item $Installer).VersionInfo.ProductVersion }
function Joined { [bool](Select-String -Path $hostConfig -Pattern '^\[company\]' -Quiet -ErrorAction SilentlyContinue) }
# The network can come up a little after the computer starts.
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
        # Up to date and joined: only look at the share for a newer version.
        if (-not (Test-Path $Installer)) { return }
        if ((Offered-Version) -le $installed) { return }
    } elseif (-not (Wait-Share)) {
        Log "cannot read $Installer (network not ready, or the share is not readable by computer accounts)"
        return
    }
    $offered = Offered-Version
    $arguments = @("/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART")
    if (-not $installed -or $offered -gt $installed) {
        # The installer also joins (/ENROLL); joining again is harmless.
        $arguments += "/ENROLL=$Token"
        if ($Unattended) { $arguments += "/UNATTENDED" }
        Log "installing $offered (installed: $(if ($installed) { $installed } else { 'none' }))"
        $p = Start-Process -FilePath $Installer -ArgumentList $arguments -Wait -PassThru
        if ($p.ExitCode) { Log "the installer failed: exit code $($p.ExitCode)"; return }
    }
    # Not joined yet (new computer, or the server could not be reached): join now.
    for ($try = 1; -not (Joined) -and $try -le 3; $try++) {
        if ($try -gt 1) { Start-Sleep 20 }
        $joinArgs = @("enroll", $Token)
        if ($Unattended) { $joinArgs += "--unattended" }
        $out = & $hostExe @joinArgs 2>&1 | Out-String
        Log "join, try ${try}: $($out.Trim())"
    }
    if (-not (Joined)) { return }
    # Remote access on (the installer could not do it before joining).
    if (-not (Get-Service RemoteDesktopRider -ErrorAction SilentlyContinue)) {
        $out = & $hostExe service install 2>&1 | Out-String
        Log "remote access: $($out.Trim())"
    }
    Log "ok: version $(Installed-Version), in the company"
} catch {
    Log "error: $_"
}
