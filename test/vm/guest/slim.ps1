$ErrorActionPreference = "SilentlyContinue"

$version = "3"
$marker = "C:\ProgramData\rider-slim.version"
if ((Get-Content $marker -ErrorAction SilentlyContinue) -eq $version) { "slim: already done"; exit 0 }

bcdedit /timeout 0 | Out-Null
bcdedit /set "{current}" quietboot yes | Out-Null
bcdedit /set "{current}" bootux disabled | Out-Null

$pol = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"
Set-ItemProperty $pol EnableFirstLogonAnimation 0 -Type DWord
Set-ItemProperty $pol DelayedDesktopSwitchTimeout 0 -Type DWord
Set-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Authentication\LogonUI\BootAnimation" DisableStartupSound 1 -Type DWord
New-Item "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize" -Force | Out-Null
Set-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize" StartupDelayInMSec 0 -Type DWord
Set-ItemProperty "HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Serialize" WaitForIdleState 0 -Type DWord

$apps = "*WebExperience*", "*Teams*", "*OneDrive*", "*BingNews*", "*BingWeather*", "*GetHelp*", "*Getstarted*",
    "*YourPhone*", "*ZuneMusic*", "*ZuneVideo*", "*WindowsFeedbackHub*", "*XboxGamingOverlay*", "*Xbox.TCUI*",
    "*XboxIdentityProvider*", "*XboxSpeechToTextOverlay*", "*GamingApp*", "*People*", "*MicrosoftOfficeHub*",
    "*Clipchamp*", "*PowerAutomateDesktop*", "*Todos*", "*windowscommunicationsapps*", "*Copilot*", "*DevHome*",
    "*OutlookForWindows*", "*549981C3F5F10*"
foreach ($a in $apps) {
    Get-AppxPackage -AllUsers $a | Remove-AppxPackage -AllUsers
    Get-AppxProvisionedPackage -Online | Where-Object DisplayName -like $a | Remove-AppxProvisionedPackage -Online | Out-Null
}
Get-Process OneDrive* | Stop-Process -Force
& "$env:SystemRoot\System32\OneDriveSetup.exe" /uninstall 2>$null

powercfg /hibernate off
powercfg /change standby-timeout-ac 0
powercfg /change monitor-timeout-ac 0
powercfg /change disk-timeout-ac 0
powercfg /setactive SCHEME_MIN

$services = "WSearch", "SysMain", "DiagTrack", "dmwappushservice", "WerSvc", "MapsBroker",
    "wuauserv", "UsoSvc", "edgeupdate", "edgeupdatem", "XblAuthManager", "XblGameSave",
    "lfsvc", "RetailDemo", "PcaSvc", "Spooler", "Fax", "WbioSrvc", "TabletInputService",
    "DPS", "WdiServiceHost", "WdiSystemHost", "TrkWks", "SSDPSRV", "upnphost", "PhoneSvc", "OneSyncSvc",
    "CDPSvc", "InstallService", "DoSvc", "wisvc", "WMPNetworkSvc", "icssvc", "SEMgrSvc", "WalletService",
    "TroubleshootingSvc", "diagsvc", "DusmSvc", "stisvc", "FrameServer", "SharedAccess"
foreach ($s in $services) {
    Stop-Service $s -Force
    Set-Service $s -StartupType Disabled
}

Set-MpPreference -DisableRealtimeMonitoring $true -DisableScanningNetworkFiles $true `
    -DisableBehaviorMonitoring $true -DisableIOAVProtection $true -MAPSReporting 0 -SubmitSamplesConsent 2
Add-MpPreference -ExclusionPath "C:\rider", "C:\rider-test", "C:\Program Files\RemoteDesktopRider",
    "C:\ProgramData\RemoteDesktopRider"

$tasks = "\Microsoft\Windows\TaskScheduler\", "\Microsoft\Windows\Defrag\", "\Microsoft\Windows\Application Experience\",
    "\Microsoft\Windows\Customer Experience Improvement Program\", "\Microsoft\Windows\Windows Defender\",
    "\Microsoft\Windows\DiskDiagnostic\", "\Microsoft\Windows\Maintenance\", "\Microsoft\Windows\UpdateOrchestrator\",
    "\Microsoft\Windows\WindowsUpdate\", "\Microsoft\Windows\Feedback\Siuf\"
foreach ($t in $tasks) { Get-ScheduledTask -TaskPath $t | Disable-ScheduledTask | Out-Null }
New-Item "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\Maintenance" -Force | Out-Null
Set-ItemProperty "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Schedule\Maintenance" MaintenanceDisabled 1

$user = "HKCU:\Software\Microsoft\Windows\CurrentVersion"
New-Item "$user\Explorer\VisualEffects" -Force | Out-Null
Set-ItemProperty "$user\Explorer\VisualEffects" VisualFXSetting 2
Set-ItemProperty "$user\Themes\Personalize" EnableTransparency 0
New-Item "$user\BackgroundAccessApplications" -Force | Out-Null
Set-ItemProperty "$user\BackgroundAccessApplications" GlobalUserDisabled 1
Set-ItemProperty "$user\ContentDeliveryManager" SubscribedContentEnabled 0
Set-ItemProperty "$user\ContentDeliveryManager" SystemPaneSuggestionsEnabled 0
Set-ItemProperty "HKCU:\Control Panel\Desktop" MenuShowDelay "0"
Set-ItemProperty "HKCU:\Control Panel\Desktop\WindowMetrics" MinAnimate "0"

$desk = "HKCU:\Control Panel\Desktop"
Set-ItemProperty $desk UserPreferencesMask ([byte[]](0x90, 0x12, 0x03, 0x80, 0x10, 0x00, 0x00, 0x00)) -Type Binary
Set-ItemProperty $desk DragFullWindows "0"
Set-ItemProperty $desk FontSmoothing "2"
Set-ItemProperty $desk Wallpaper ""
Set-ItemProperty "HKCU:\Control Panel\Colors" Background "40 44 52"
$adv = "$user\Explorer\Advanced"
foreach ($v in @{ TaskbarAnimations = 0; ListviewAlphaSelect = 0; ListviewShadow = 0; IconsOnly = 1; EnableSnapAssistFlyout = 0
        EnableSnapBar = 0; TaskbarDa = 0; TaskbarMn = 0; ShowCopilotButton = 0; Start_TrackDocs = 0; Start_IrisRecommendations = 0
        ShowTaskViewButton = 0 }.GetEnumerator()) {
    Set-ItemProperty $adv $v.Key $v.Value -Type DWord
}
New-Item "HKCU:\Software\Microsoft\Windows\DWM" -Force | Out-Null
Set-ItemProperty "HKCU:\Software\Microsoft\Windows\DWM" EnableAeroPeek 0 -Type DWord
Set-ItemProperty "HKCU:\Software\Microsoft\Windows\DWM" AlwaysHibernateThumbnails 0 -Type DWord
New-Item "$user\Search" -Force | Out-Null
Set-ItemProperty "$user\Search" SearchboxTaskbarMode 0 -Type DWord
New-Item "HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot" -Force | Out-Null
Set-ItemProperty "HKCU:\Software\Policies\Microsoft\Windows\WindowsCopilot" TurnOffWindowsCopilot 1 -Type DWord
Add-Type -Namespace Slim -Name Spi -MemberDefinition @"
[DllImport("user32.dll")] public static extern bool SystemParametersInfo(uint a, uint b, IntPtr c, uint d);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool SystemParametersInfo(uint a, uint b, string c, uint d);
"@
$send = 3
$anim = [System.Runtime.InteropServices.Marshal]::AllocHGlobal(8)
[System.Runtime.InteropServices.Marshal]::WriteInt32($anim, 0, 8); [System.Runtime.InteropServices.Marshal]::WriteInt32($anim, 4, 0)
[Slim.Spi]::SystemParametersInfo(0x0049, 8, $anim, $send) | Out-Null
foreach ($off in 0x1043, 0x1013, 0x1015, 0x1019, 0x101B, 0x1025, 0x1003, 0x1005, 0x1049, 0x1023) {
    [Slim.Spi]::SystemParametersInfo($off, 0, [IntPtr]::Zero, $send) | Out-Null
}
[Slim.Spi]::SystemParametersInfo(0x0025, 0, [IntPtr]::Zero, $send) | Out-Null
[Slim.Spi]::SystemParametersInfo(0x0014, 0, "", $send) | Out-Null

foreach ($name in "MicrosoftEdgeAutoLaunch*", "OneDrive*", "Teams*") {
    Get-Item "$user\Run" | Select-Object -ExpandProperty Property | Where-Object { $_ -like $name } |
        ForEach-Object { Remove-ItemProperty "$user\Run" $_ }
}
New-Item "HKLM:\SOFTWARE\Policies\Microsoft\Edge" -Force | Out-Null
Set-ItemProperty "HKLM:\SOFTWARE\Policies\Microsoft\Edge" StartupBoostEnabled 0
Set-ItemProperty "HKLM:\SOFTWARE\Policies\Microsoft\Edge" BackgroundModeEnabled 0
New-Item "HKLM:\SOFTWARE\Policies\Microsoft\Dsh" -Force | Out-Null
Set-ItemProperty "HKLM:\SOFTWARE\Policies\Microsoft\Dsh" AllowNewsAndInterests 0

Set-Content $marker $version
"slim: done (version $version); takes full effect at the next start"
