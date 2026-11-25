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
#define TrayClass "RemoteDesktopRider.Tray"
#define WmQuitApp "$8003"

[Setup]
AppId={{80CDF23F-6BC2-46BF-AEF6-6515FE750049}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Elvin Huseynov
AppPublisherURL=https://github.com/codewithelvin/remote-desktop-rider
AppSupportURL=https://github.com/codewithelvin/remote-desktop-rider/issues
AppUpdatesURL=https://github.com/codewithelvin/remote-desktop-rider/releases
VersionInfoVersion={#AppVersion}
VersionInfoProductName={#AppName}
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
CloseApplications=no
SetupMutex=RemoteDesktopRiderSetup
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
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueName: "RemoteDesktopRider"; ValueType: none; Flags: uninsdeletevalue
Root: HKCU; Subkey: "Software\RemoteDesktopRider"; ValueType: none; Flags: uninsdeletekey dontcreatekey

[Run]
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FirewallRule}"""; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FirewallRule}"" dir=in action=allow protocol=UDP profile=any program=""{app}\rider-host.exe"""; Flags: runhidden
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall add rule name=""{#FirewallRule}"" dir=in action=allow protocol=UDP profile=any program=""{app}\rider-client.exe"""; Flags: runhidden
Filename: "{sys}\net.exe"; Parameters: "start {#ServiceName}"; Flags: runhidden; Check: ServiceWasRunning
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: postinstall nowait skipifsilent runasoriginaluser
Filename: "{app}\{#AppExe}"; Parameters: "{code:RelaunchArgs}"; Flags: nowait runasoriginaluser; Check: WizardSilent and AppWasRunning

[UninstallRun]
Filename: "{app}\rider-host.exe"; Parameters: "service uninstall"; Flags: runhidden; RunOnceId: "RemoveService"
Filename: "{sys}\netsh.exe"; Parameters: "advfirewall firewall delete rule name=""{#FirewallRule}"""; Flags: runhidden; RunOnceId: "RemoveFirewallRule"

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
var
  AppRunning, ServiceRunning: Boolean;

function AppWasRunning: Boolean;
begin
  Result := AppRunning;
end;

function RelaunchArgs(Param: String): String;
begin
  if ExpandConstant('{param:show|0}') = '1' then
    Result := ''
  else
    Result := '--tray';
end;

function ServiceWasRunning: Boolean;
begin
  Result := ServiceRunning;
end;

function HasSwitch(Name: String): Boolean;
var
  I: Integer;
begin
  Result := False;
  for I := 1 to ParamCount do
    if CompareText(ParamStr(I), '/' + Name) = 0 then
      Result := True;
end;

function ValidToken(T: String): Boolean;
var
  I: Integer;
begin
  Result := (Length(T) > 5) and (Length(T) < 4096) and (Copy(T, 1, 5) = 'rdr1.');
  if Result then
    for I := 6 to Length(T) do
      if Pos(T[I], '0123456789abcdefABCDEF') = 0 then
        Result := False;
end;

procedure Enroll;
var
  Token, Host, Args, Log: String;
  Code: Integer;
begin
  Token := Trim(ExpandConstant('{param:ENROLL|}'));
  if Token = '' then
    Exit;
  Host := ExpandConstant('{app}\rider-host.exe');
  Log := ExpandConstant('{commonappdata}\RemoteDesktopRider\host\enroll.log');
  Exec(Host, 'info', '', SW_HIDE, ewWaitUntilTerminated, Code);
  if not ValidToken(Token) then
  begin
    SaveStringToFile(Log, 'not a join token: /ENROLL=' + Copy(Token, 1, 12) + '...' + #13#10, False);
    Exit;
  end;
  Args := 'enroll ' + Token;
  if HasSwitch('UNATTENDED') then
    Args := Args + ' --unattended';
  Exec(ExpandConstant('{cmd}'), '/c ""' + Host + '" ' + Args + ' > "' + Log + '" 2>&1"', '', SW_HIDE,
    ewWaitUntilTerminated, Code);
  Exec(Host, 'service install', '', SW_HIDE, ewWaitUntilTerminated, Code);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    Enroll;
end;

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
  if Exec(ExpandConstant('{sys}\net.exe'), 'stop {#ServiceName}', '', SW_HIDE, ewWaitUntilTerminated, Code) and (Code = 0) then
    ServiceRunning := True;
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
