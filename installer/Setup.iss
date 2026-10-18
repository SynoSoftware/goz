#if Ver < EncodeVer(7, 1, 0)
  #error Inno Setup 7.1 or later is required.
#endif
#ifndef AppVersion
  #error Run installer\Build.ps1, which supplies the version from Cargo.toml.
#endif

; The service name, gozler_core::proto::SERVICE_NAME. It stays "gozler" across
; versions, and it is not "goz", the upstream service installed beside it.
#define ServiceName "gozler"

[Setup]
AppId=gozler
AppName=gozler
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppPublisherURL={#AppUrl}
AppUpdatesURL={#AppUrl}/releases/latest
VersionInfoVersion={#AppVersion}
DefaultDirName={autopf}\gozler
; Installing the service and the machine PATH entry needs administrator rights.
PrivilegesRequired=admin
; The x86 setup engine is smaller; the installed binaries stay x64.
SetupArchitecture=x86
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
DisableDirPage=yes
DisableProgramGroupPage=yes
DisableWelcomePage=yes
WizardStyle=modern dynamic
LicenseFile=..\LICENSE
OutputDir=..\target\installer
OutputBaseFilename=gozler-{#AppVersion}-x64-Setup
Compression=lzma2/max
SolidCompression=yes
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
StartService=Starting the gozler service...
ServiceFailed=The gozler service could not be installed (exit code %1). Search does not work until it runs. Run "gozlerd install" in an elevated terminal to retry.

[Files]
Source: "..\target\release\gozlerd.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\gozler.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[UninstallRun]
Filename: "{app}\gozlerd.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveService"

[UninstallDelete]
; The service log that init_service_tracing in service.rs writes.
Type: filesandordirs; Name: "{win}\Logs\gozler"

[Code]
const
  EnvironmentKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

function PathHasApp(const Path: String): Boolean;
begin
  Result := Pos(';' + Uppercase(ExpandConstant('{app}')) + ';',
    ';' + Uppercase(Path) + ';') > 0;
end;

procedure AddAppToPath;
var
  Path: String;
begin
  if not RegQueryStringValue(HKLM, EnvironmentKey, 'Path', Path) then
    Path := '';
  if PathHasApp(Path) then exit;
  if (Path <> '') and (Path[Length(Path)] <> ';') then
    Path := Path + ';';
  RegWriteExpandStringValue(HKLM, EnvironmentKey, 'Path', Path + ExpandConstant('{app}'));
end;

procedure RemoveAppFromPath;
var
  Path: String;
begin
  if not RegQueryStringValue(HKLM, EnvironmentKey, 'Path', Path) then exit;
  if not PathHasApp(Path) then exit;
  Path := ';' + Path + ';';
  StringChangeEx(Path, ';' + ExpandConstant('{app}') + ';', ';', True);
  RegWriteExpandStringValue(HKLM, EnvironmentKey, 'Path', Copy(Path, 2, Length(Path) - 2));
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  // The running daemon locks gozlerd.exe, and "gozlerd install" does not
  // restart a service that already runs, so stop it before the files change.
  // net.exe waits for the stop; its error when nothing runs is expected.
  Exec(ExpandConstant('{sys}\net.exe'), 'stop {#ServiceName}', '', SW_HIDE,
    ewWaitUntilTerminated, Code);
  Log(Format('net stop {#ServiceName}: code=%d', [Code]));
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code: Integer;
begin
  if CurStep <> ssPostInstall then exit;
  AddAppToPath;
  WizardForm.StatusLabel.Caption := CustomMessage('StartService');
  // "install" registers the service, or points an existing one at this
  // gozlerd.exe, and starts it.
  if not Exec(ExpandConstant('{app}\gozlerd.exe'), 'install', '', SW_HIDE,
    ewWaitUntilTerminated, Code) or (Code <> 0) then
    SuppressibleMsgBox(FmtMessage(CustomMessage('ServiceFailed'), [IntToStr(Code)]),
      mbError, MB_OK, IDOK);
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveAppFromPath;
end;
