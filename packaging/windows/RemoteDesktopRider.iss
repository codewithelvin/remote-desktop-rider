; Remote Desktop Rider installer (Inno Setup 6). Build with build.ps1, which
; passes AppVersion, BinDir, IconFile and OutDir.
;
; All three programs go to "Program Files\RemoteDesktopRider": rider-host's
; service expects its binary there (only administrators can change it, so it
; is safe to run as SYSTEM), and the firewall rules are per program path.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef BinDir
  #define BinDir "..\..\target\release"
#endif
#ifndef OutDir
  #define OutDir "..\..\target\installer"
#endif

#define AppName "Remote Desktop Rider"
#define AppExe "RemoteDesktopRider.exe"
#define ServiceName "RemoteDesktopRider"
#define FirewallRule "Remote Desktop Rider (app)"
; Window class of the app's tray window and its "quit" message (WM_APP + 3);
; keep in sync with crates/rider-app/src/tray.rs.
#define TrayClass "RemoteDesktopRider.Tray"
#define WmQuitApp "$8003"

[Setup]
AppId={{80CDF23F-6BC2-46BF-AEF6-6515FE750049}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppName}
AppPublisherURL=https://github.com/codewithelvin/remote-desktop-rider
AppSupportURL=https://github.com/codewithelvin/remote-desktop-rider/issues
AppUpdatesURL=https://github.com/codewithelvin/remote-desktop-rider/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
; Fixed: the service only runs from this folder.
DefaultDirName={autopf}\RemoteDesktopRider
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableReadyPage=yes
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutDir}
OutputBaseFilename=rdr-setup-{#AppVersion}
#ifdef IconFile
SetupIconFile={#IconFile}
#endif
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
Compression=lzma2/ultra64
SolidCompression=yes
; Running copies are closed in [Code] (the tray app ignores WM_CLOSE).
CloseApplications=no
SetupMutex=RemoteDesktopRiderSetup
; Only the startup entry of the installing user is touched.
UsedUserAreasWarning=no

[Tasks]
Name: desktopicon; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#BinDir}\{#AppExe}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\rider-client.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\rider-host.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; Created by the app itself; removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "RemoteDesktopRider"; ValueType: none; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\RemoteDesktopRider"; ValueType: none; Flags: uninsdeletekey dontcreatekey

[Run]
; Inbound UDP for direct connections, so Windows never asks (pairing runs
; rider-host before remote access is turned on; the viewer receives hole-punch
; packets). Scoped to the two programs.
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FirewallRule}"""; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FirewallRule}"" dir=in action=allow protocol=UDP profile=any program=""{app}\rider-host.exe"""; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FirewallRule}"" dir=in action=allow protocol=UDP profile=any program=""{app}\rider-client.exe"""; Flags: runhidden
Filename: "{sys}\net.exe"; Parameters: "start {#ServiceName}"; Flags: runhidden; Check: ServiceWasRunning
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent runasoriginaluser
; Silent upgrades: bring back the tray app if it was running.
Filename: "{app}\{#AppExe}"; Parameters: "--tray"; Flags: nowait runasoriginaluser; Check: WizardSilent and AppWasRunning

[UninstallRun]
Filename: "{app}\rider-host.exe"; Parameters: "service uninstall"; Flags: runhidden; RunOnceId: "RemoveService"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FirewallRule}"""; Flags: runhidden; RunOnceId: "RemoveFirewallRule"

[UninstallDelete]
; The folder may predate this installer (an earlier `rider-host service install`).
Type: dirifempty; Name: "{app}"

[Code]
var
  AppRunning, ServiceRunning: Boolean;

function AppWasRunning: Boolean;
begin
  Result := AppRunning;
end;

function ServiceWasRunning: Boolean;
begin
  Result := ServiceRunning;
end;

{ Closes every running part so its files can be replaced or removed. }
procedure StopAll;
var
  Wnd: HWND;
  I, Code: Integer;
begin
  Wnd := FindWindowByClassName('{#TrayClass}');
  if Wnd <> 0 then
  begin
    AppRunning := True;
    PostMessage(Wnd, {#WmQuitApp}, 0, 0);
    for I := 1 to 30 do
    begin
      if FindWindowByClassName('{#TrayClass}') = 0 then Break;
      Sleep(100);
    end;
  end;
  { net stop waits for the service (and its session agent) to exit; it
    returns 0 only if the service was running. }
  if Exec(ExpandConstant('{sys}\net.exe'), 'stop {#ServiceName}', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0) then
    ServiceRunning := True;
  { Viewer windows, pairing, an administrator window: nothing to save. }
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM rider-client.exe /IM rider-host.exe /IM {#AppExe}', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopAll;
  Result := '';
end;

procedure CurUninstallStepChanged(Step: TUninstallStep);
begin
  if Step = usUninstall then
    StopAll;
end;
