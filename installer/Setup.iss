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
CleanupCaption=Clean up
CleanupDescription=Setup found programs that gozler replaces.
CleanupText=Choose what Setup removes after gozler is running: the program, its service and its PATH entry. Other files in its folder stay, and anything you leave stays beside gozler.
CleanGoz=goz in %1
CleanOldGozler=An earlier gozler in %1
CleaningUp=Cleaning up...
GozFailed=goz could not be removed (exit code %1), so it stays installed beside gozler. To remove it, run "%2" uninstall in an elevated terminal.

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

type
  // A goz or an earlier gozler that people run: one that a service runs or
  // that a PATH finds. Copies elsewhere, such as backups, are never found.
  TFound = record
    Folder: String;
    IsGoz: Boolean;
    // The program its service runs, or '' when only a PATH finds it.
    Daemon: String;
    // Its checkbox on CleanupPage.
    Item: Integer;
  end;

var
  // gozler's folder: the directory page is off, so gozler goes where the
  // last setup put it, or to the default folder.
  InstallFolder: String;
  Found: array of TFound;
  // Created only when something is found.
  CleanupPage: TInputOptionWizardPage;

function NormalFolder(Folder: String): String;
begin
  Folder := Trim(Folder);
  StringChangeEx(Folder, '"', '', True);
  Result := RemoveBackslashUnlessRoot(Folder);
end;

function SameFolder(const A, B: String): Boolean;
begin
  Result := CompareText(NormalFolder(A), NormalFolder(B)) = 0;
end;

// Puts the install folder on the machine PATH just before the first folder
// there that holds another gozler.exe, or at the end when none does, so the
// gozler this setup installs is the one a terminal runs even when an older
// copy is kept. The machine PATH comes before the user PATH, so a copy on
// the user PATH never wins.
procedure AddAppToPath;
var
  Original, Path, Kept, Entry, App: String;
  Semicolon: Integer;
  Placed: Boolean;
begin
  App := ExpandConstant('{app}');
  if not RegQueryStringValue(HKLM, EnvironmentKey, 'Path', Original) then
    Original := '';
  Path := Original + ';';
  Kept := '';
  Placed := False;
  while Path <> '' do
  begin
    Semicolon := Pos(';', Path);
    Entry := Copy(Path, 1, Semicolon - 1);
    Delete(Path, 1, Semicolon);
    if (Entry = '') or SameFolder(Entry, App) then
      continue;
    if not Placed and (Pos('%', Entry) = 0) and
      FileExists(NormalFolder(Entry) + '\gozler.exe') then
    begin
      Kept := Kept + App + ';';
      Placed := True;
    end;
    Kept := Kept + Entry + ';';
  end;
  if not Placed then
    Kept := Kept + App + ';';
  Kept := Copy(Kept, 1, Length(Kept) - 1);
  if Kept <> Original then
    RegWriteExpandStringValue(HKLM, EnvironmentKey, 'Path', Kept);
end;

// Removes every entry that names Folder from the PATH in RootKey: the machine
// PATH under HKLM or the user PATH under HKCU. An entry may quote the folder
// or end with a backslash; every other entry stays as written.
procedure RemoveFolderFromPath(const RootKey: Integer; const SubKey, Folder: String);
var
  Path, Kept, Entry: String;
  Semicolon: Integer;
  Changed: Boolean;
begin
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Path) then exit;
  Kept := '';
  Changed := False;
  Path := Path + ';';
  while Path <> '' do
  begin
    Semicolon := Pos(';', Path);
    Entry := Copy(Path, 1, Semicolon - 1);
    Delete(Path, 1, Semicolon);
    if (Entry <> '') and SameFolder(Entry, Folder) then
      Changed := True
    else
      Kept := Kept + Entry + ';';
  end;
  if Changed then
    RegWriteExpandStringValue(RootKey, SubKey, 'Path', Copy(Kept, 1, Length(Kept) - 1));
end;

// The program the service Name runs, from an ImagePath such as
// "C:\Program Files\goz\gozd.exe" run --service, or '' when there is none.
function ServiceProgram(const Name: String): String;
var
  ImagePath: String;
  Quote: Integer;
begin
  Result := '';
  if not RegQueryStringValue(HKLM, 'SYSTEM\CurrentControlSet\Services\' + Name,
    'ImagePath', ImagePath) then exit;
  if Copy(ImagePath, 1, 1) = '"' then
  begin
    Delete(ImagePath, 1, 1);
    Quote := Pos('"', ImagePath);
    if Quote > 0 then
      Result := Copy(ImagePath, 1, Quote - 1);
  end
  else if Pos(' ', ImagePath) > 0 then
    Result := Copy(ImagePath, 1, Pos(' ', ImagePath) - 1)
  else
    Result := ImagePath;
  if not FileExists(Result) then
    Result := '';
end;

// Records a goz or an earlier gozler in Folder, unless it is this install, a
// Cargo build output such as target\release, which holds programs someone
// compiled rather than installed, or a folder already recorded. Daemon is
// the program its service runs, or ''.
procedure AddFound(const Folder: String; const IsGoz: Boolean; const Daemon: String);
var
  I: Integer;
begin
  if (Folder = '') or SameFolder(Folder, InstallFolder) or
    DirExists(Folder + '\.fingerprint') then exit;
  for I := 0 to GetArrayLength(Found) - 1 do
    if SameFolder(Found[I].Folder, Folder) then exit;
  I := GetArrayLength(Found);
  SetArrayLength(Found, I + 1);
  Found[I].Folder := NormalFolder(Folder);
  Found[I].IsGoz := IsGoz;
  Found[I].Daemon := Daemon;
  Found[I].Item := -1;
end;

// Records the goz or gozler that a folder on the PATH in RootKey holds. Only
// the folder itself is looked at, never its subfolders. An entry built from
// an environment variable names a system folder, not a hand install.
procedure FindOnPath(const RootKey: Integer; const SubKey: String);
var
  Path, Entry: String;
  Semicolon: Integer;
begin
  if not RegQueryStringValue(RootKey, SubKey, 'Path', Path) then exit;
  Path := Path + ';';
  while Path <> '' do
  begin
    Semicolon := Pos(';', Path);
    Entry := NormalFolder(Copy(Path, 1, Semicolon - 1));
    Delete(Path, 1, Semicolon);
    if (Entry <> '') and (Pos('%', Entry) = 0) then
    begin
      if FileExists(Entry + '\gozler.exe') then
        AddFound(Entry, False, '')
      else if FileExists(Entry + '\goz.exe') then
        AddFound(Entry, True, '');
    end;
  end;
end;

procedure InitializeWizard;
var
  Daemon: String;
  I: Integer;
begin
  InstallFolder := WizardForm.PrevAppDir;
  if InstallFolder = '' then
    InstallFolder := ExpandConstant('{autopf}\gozler');
  // Services first, so a folder that both a service and a PATH name keeps its
  // service.
  Daemon := ServiceProgram('goz');
  if Daemon <> '' then
    AddFound(ExtractFileDir(Daemon), True, Daemon);
  Daemon := ServiceProgram('{#ServiceName}');
  if Daemon <> '' then
    AddFound(ExtractFileDir(Daemon), False, Daemon);
  FindOnPath(HKLM, EnvironmentKey);
  FindOnPath(HKCU, 'Environment');
  if GetArrayLength(Found) = 0 then exit;

  CleanupPage := CreateInputOptionPage(wpLicense, CustomMessage('CleanupCaption'),
    CustomMessage('CleanupDescription'), CustomMessage('CleanupText'), False, False);
  for I := 0 to GetArrayLength(Found) - 1 do
    if Found[I].IsGoz then
    begin
      Found[I].Item := CleanupPage.Add(
        FmtMessage(CustomMessage('CleanGoz'), [Found[I].Folder]));
      // goz stays unless the person ticks it, because gozler runs beside it
      // and people keep goz until gozler works for them. An upgrade
      // preselects what the last install chose; a first install, which has
      // no recorded choice, keeps goz.
      CleanupPage.Values[Found[I].Item] := GetPreviousData('Goz', '1') = '0';
    end
    else
    begin
      Found[I].Item := CleanupPage.Add(
        FmtMessage(CustomMessage('CleanOldGozler'), [Found[I].Folder]));
      CleanupPage.Values[Found[I].Item] := True;
    end;
end;

// Whether the person chose to remove Found[I]. A silent install removes
// nothing, so a script never removes a program by accident.
function CleanUp(const I: Integer): Boolean;
begin
  Result := False;
  if WizardSilent then exit;
  Result := CleanupPage.Values[Found[I].Item];
end;

// Records whether goz was kept, for the next upgrade's default.
procedure RegisterPreviousData(PreviousDataKey: Integer);
var
  I: Integer;
  Seen, Kept: Boolean;
begin
  Seen := False;
  Kept := False;
  for I := 0 to GetArrayLength(Found) - 1 do
    if Found[I].IsGoz then
    begin
      Seen := True;
      if not CleanUp(I) then
        Kept := True;
    end;
  if not Seen then exit;
  if Kept then
    SetPreviousData(PreviousDataKey, 'Goz', '1')
  else
    SetPreviousData(PreviousDataKey, 'Goz', '0');
end;

// Removes a hand install from Folder: its folder from both PATHs, because a
// hand install may use either, then the files its zip holds, then the folder
// when nothing else is left in it.
procedure RemoveHandInstall(const Folder, Client, Daemon: String);
begin
  RemoveFolderFromPath(HKLM, EnvironmentKey, Folder);
  RemoveFolderFromPath(HKCU, 'Environment', Folder);
  DeleteFile(Folder + '\' + Client);
  DeleteFile(Folder + '\' + Daemon);
  DeleteFile(Folder + '\LICENSE');
  DeleteFile(Folder + '\README.md');
  RemoveDir(Folder);
end;

// Removes the goz in Found[I]. When its service runs from there, goz's own
// uninstall stops the service and waits for it first, and its logs go too.
procedure RemoveGoz(const I: Integer);
var
  Code: Integer;
begin
  if Found[I].Daemon <> '' then
  begin
    if not Exec(Found[I].Daemon, 'uninstall', '', SW_HIDE, ewWaitUntilTerminated, Code)
      or (Code <> 0) then
    begin
      SuppressibleMsgBox(FmtMessage(CustomMessage('GozFailed'), [IntToStr(Code),
        Found[I].Daemon]), mbError, MB_OK, IDOK);
      exit;
    end;
    DelTree(ExpandConstant('{commonappdata}\goz'), True, True, True);
  end;
  RemoveHandInstall(Found[I].Folder, 'goz.exe', 'gozd.exe');
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
  Code, I: Integer;
begin
  if CurStep <> ssPostInstall then exit;
  AddAppToPath;
  WizardForm.StatusLabel.Caption := CustomMessage('StartService');
  // "install" registers the service, or points an existing one at this
  // gozlerd.exe, and starts it.
  if not Exec(ExpandConstant('{app}\gozlerd.exe'), 'install', '', SW_HIDE,
    ewWaitUntilTerminated, Code) or (Code <> 0) then
  begin
    SuppressibleMsgBox(FmtMessage(CustomMessage('ServiceFailed'), [IntToStr(Code)]),
      mbError, MB_OK, IDOK);
    // Nothing is removed while gozler's service does not run, so search keeps
    // working.
    exit;
  end;
  WizardForm.StatusLabel.Caption := CustomMessage('CleaningUp');
  // The gozler service now runs this setup's gozlerd.exe, so an earlier
  // gozler is unused.
  for I := 0 to GetArrayLength(Found) - 1 do
    if CleanUp(I) then
      if Found[I].IsGoz then
        RemoveGoz(I)
      else
        RemoveHandInstall(Found[I].Folder, 'gozler.exe', 'gozlerd.exe');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usUninstall then
    RemoveFolderFromPath(HKLM, EnvironmentKey, ExpandConstant('{app}'));
end;
