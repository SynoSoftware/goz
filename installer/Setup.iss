#if Ver < EncodeVer(7, 1, 0)
  #error Inno Setup 7.1 or later is required.
#endif
#ifndef AppVersion
  #error Run installer\Build.ps1, which supplies the version from Cargo.toml.
#endif

; The service name, gozler_core::proto::SERVICE_NAME. It stays "gozler" across
; versions, and it is not "goz", the upstream service installed beside it.
#define ServiceName "gozler"
; The install folder and the folder of the service log that
; init_service_tracing in service.rs writes.
#define AppFolder "{autopf}\Gozler"
#define LogFolder "{win}\Logs\Gozler"
; The window's single-instance mutex, window class and exit request, and the
; answer that it exits: INSTANCE_MUTEX, WINDOW_CLASS, EXIT_MESSAGE and
; EXIT_CLOSES in gozler-ui's app module.
#define InstanceMutex "Local\gozler.ui"
#define WindowClass "gozler.ui"
#define ExitMessage "gozler.ui.exit"
#define ExitCloses 1

[Setup]
AppId=gozler
AppName=Gozler
AppVersion={#AppVersion}
AppPublisher={#AppPublisher}
AppCopyright=Copyright (C) 2026 Synapse N.V. and Mustafa Ahci
AppPublisherURL={#AppUrl}
AppUpdatesURL={#AppUrl}/releases/latest
VersionInfoVersion={#AppVersion}
DefaultDirName={#AppFolder}
; Every install goes to the default folder, so an upgrade from a setup that
; wrote the folder's name in lower case gets its new name.
UsePreviousAppDir=no
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

[Messages]
UninstallAppRunningError=Gozler is running. To close it, choose File > Exit in its window, or Exit on the menu of its icon in the notification area.%n%nThen click OK to continue, or Cancel to stop uninstalling.

[CustomMessages]
StartService=Starting the Gozler service...
ServiceFailed=The Gozler service could not be installed (exit code %1). Search does not work until it runs. Run "gozlerd install" in an elevated terminal to retry.
CleanupCaption=Keep or remove other programs
CleanupDescription=Gozler works with or without these programs.
CleanupText=For each program you select, Setup deletes its files and removes its folder from PATH. Other files in that folder stay.
CleanGoz=Remove goz from %1
CleanOldGozler=Remove the other copy of Gozler from %1
CleaningUp=Finishing installation...
SeeEveryFileCaption=Choose who sees every file
SeeEveryFileDescription=Gozler shows each user only the files in folders they can open.
SeeEveryFileText=When this is checked, %1 also sees the names of files in folders they can't open. It doesn't let them open those files.%n%nTo change who sees every file later, open Settings > Visibility.
SeeEveryFileItem=Let %1 see the names of every file on this PC
WindowStays=Gozler is still running, so Setup can't replace it.%n%nExit Gozler, then select Back and Install again.
GozFailed=Setup couldn't remove goz (error code %1).%n%nTo remove goz's service, open a terminal as administrator and run:%n  pushd "%2"%n  .\gozd.exe uninstall%n%nIf the command succeeds, delete goz.exe and gozd.exe from that folder.

[Files]
Source: "..\target\release\gozlerd.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\gozler.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\gozler-ui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Gozler"; Filename: "{app}\gozler-ui.exe"

[Run]
; The window runs as the person who started the setup, not as administrator,
; so its settings, its sign-in entry and its File Explorer command are that
; person's.
Filename: "{app}\gozler-ui.exe"; Description: "{cm:LaunchProgram,Gozler}"; Flags: postinstall nowait skipifsilent runasoriginaluser

[UninstallRun]
Filename: "{app}\gozler-ui.exe"; Parameters: "--unregister-shortcut"; Flags: runhidden waituntilterminated; RunOnceId: "RestoreWindowsShortcut"
Filename: "{app}\gozlerd.exe"; Parameters: "uninstall"; Flags: runhidden waituntilterminated; RunOnceId: "RemoveService"

[UninstallDelete]
Type: filesandordirs; Name: "{#LogFolder}"

[Code]
const
  EnvironmentKey = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';
  SMTO_ABORTIFHUNG = 2;
  SMTO_NOTIMEOUTIFNOTHUNG = 8;
  // The service's settings key and its list of users and groups who see
  // every file: SETTINGS_KEY and SEE_EVERY_FILE_VALUE in gozler-core's
  // include module.
  SettingsKey = 'SOFTWARE\Gozler';
  SeeEveryFileValue = 'SeeEveryFile';
  WTS_CURRENT_SESSION = $FFFFFFFF;
  WTSUserName = 5;
  WTSDomainName = 7;

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
  // Gozler's folder, the default one, because the directory page is off.
  InstallFolder: String;
  Found: array of TFound;
  // Created only when something is found.
  CleanupPage: TInputOptionWizardPage;
  // Created only when the setup offers SignedInUser a place on the list of
  // users who see every file.
  SeeEveryFilePage: TInputOptionWizardPage;
  SignedInUser: String;

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

// Buffer receives the address of the text. Pascal Script has no pointer
// type, so it is a pointer-sized integer, which only lstrlenW, lstrcpynW and
// WTSFreeMemory read.
function WTSQuerySessionInformation(Server: THandle; SessionId: DWORD; InfoClass: Integer;
  var Buffer: NativeInt; var Bytes: DWORD): BOOL;
  external 'WTSQuerySessionInformationW@wtsapi32.dll stdcall';
procedure WTSFreeMemory(Memory: NativeInt);
  external 'WTSFreeMemory@wtsapi32.dll stdcall';
function lstrlenW(Text: NativeInt): Integer;
  external 'lstrlenW@kernel32.dll stdcall';
function lstrcpynW(Target: String; Source: NativeInt; MaxLength: Integer): NativeInt;
  external 'lstrcpynW@kernel32.dll stdcall';

// The InfoClass text of the Windows session that runs the setup, or '' when
// Windows gives none.
function SessionText(const InfoClass: Integer): String;
var
  Buffer: NativeInt;
  Bytes: DWORD;
  Count: Integer;
begin
  Result := '';
  if not WTSQuerySessionInformation(0, WTS_CURRENT_SESSION, InfoClass, Buffer, Bytes) then
    exit;
  Count := lstrlenW(Buffer);
  if Count > 0 then
  begin
    SetLength(Result, Count);
    lstrcpynW(Result, Buffer, Count + 1);
  end;
  WTSFreeMemory(Buffer);
end;

// The user signed in to the session that runs the setup, as DOMAIN\name, or
// '' when nobody is, as in session 0. It is the session's user, not the
// account that approved the elevation, so an install that another
// administrator approved names the person who started it.
function SessionUser: String;
var
  Name, Domain: String;
begin
  Result := '';
  Name := SessionText(WTSUserName);
  if Name = '' then exit;
  Domain := SessionText(WTSDomainName);
  if Domain = '' then
    Result := Name
  else
    Result := Domain + '\' + Name;
end;

// Offers the signed-in user a place on the list of users who see every
// file. Only a first install asks, when no list is stored: an upgrade or a
// new install after an uninstall keeps the list, so a user an administrator
// removed is never added back. A silent install asks nobody, so it grants
// nobody.
procedure CreateSeeEveryFilePage;
var
  After: Integer;
begin
  if WizardSilent or RegValueExists(HKLM64, SettingsKey, SeeEveryFileValue) then exit;
  SignedInUser := SessionUser;
  if SignedInUser = '' then exit;
  // Granting a user is unrelated to removing other programs, so it gets its
  // own page, the last choice before Ready.
  if CleanupPage <> nil then
    After := CleanupPage.ID
  else
    After := wpLicense;
  SeeEveryFilePage := CreateInputOptionPage(After, CustomMessage('SeeEveryFileCaption'),
    CustomMessage('SeeEveryFileDescription'),
    FmtMessage(CustomMessage('SeeEveryFileText'), [SignedInUser]), False, False);
  SeeEveryFilePage.Add(FmtMessage(CustomMessage('SeeEveryFileItem'), [SignedInUser]));
  SeeEveryFilePage.Values[0] := True;
end;

// The page that offers to remove each goz and earlier gozler in Found.
procedure CreateCleanupPage;
var
  I: Integer;
begin
  CleanupPage := CreateInputOptionPage(wpLicense, CustomMessage('CleanupCaption'),
    CustomMessage('CleanupDescription'), CustomMessage('CleanupText'), False, False);
  for I := 0 to GetArrayLength(Found) - 1 do
    if Found[I].IsGoz then
    begin
      Found[I].Item := CleanupPage.Add(
        FmtMessage(CustomMessage('CleanGoz'), [Found[I].Folder]));
      // Every setup starts with goz kept, because gozler runs beside it and
      // removing goz is a choice, never a step gozler needs.
      CleanupPage.Values[Found[I].Item] := False;
    end
    else
    begin
      Found[I].Item := CleanupPage.Add(
        FmtMessage(CustomMessage('CleanOldGozler'), [Found[I].Folder]));
      CleanupPage.Values[Found[I].Item] := True;
    end;
end;

procedure InitializeWizard;
var
  Daemon: String;
begin
  InstallFolder := ExpandConstant('{#AppFolder}');
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
  if GetArrayLength(Found) > 0 then
    CreateCleanupPage;
  CreateSeeEveryFilePage;
end;

// Whether the person chose to remove Found[I]. A silent install removes
// nothing, so a script never removes a program by accident.
function CleanUp(const I: Integer): Boolean;
begin
  Result := False;
  if WizardSilent then exit;
  Result := CleanupPage.Values[Found[I].Item];
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
        Found[I].Folder]), mbError, MB_OK, IDOK);
      exit;
    end;
    DelTree(ExpandConstant('{commonappdata}\goz'), True, True, True);
  end;
  RemoveHandInstall(Found[I].Folder, 'goz.exe', 'gozd.exe');
end;

// Gives an existing Folder the case of its name in Folder, such as Gozler for
// the gozler folder an earlier setup made. Windows keeps a folder's name when
// files are written into it, so only a rename changes it. A folder in use
// keeps its old name, which works the same.
procedure MatchFolderCase(const Folder: String);
var
  FindRec: TFindRec;
  Actual: String;
begin
  if not FindFirst(Folder, FindRec) then exit;
  Actual := FindRec.Name;
  FindClose(FindRec);
  if (Actual <> ExtractFileName(Folder)) and
    not RenameFile(ExtractFilePath(Folder) + Actual, Folder) then
    Log(Format('Could not rename %s%s to %s', [ExtractFilePath(Folder), Actual, Folder]));
end;

function RegisterWindowMessage(Name: String): Cardinal;
  external 'RegisterWindowMessageW@user32.dll stdcall';
function SendMessageTimeout(Window: HWND; Msg: Cardinal; WParam, LParam: Longint;
  Flags, Timeout: Cardinal; var Answer: Longint): Longint;
  external 'SendMessageTimeoutW@user32.dll stdcall';
function GetWindowThreadProcessId(Window: HWND; var ProcessId: Cardinal): Cardinal;
  external 'GetWindowThreadProcessId@user32.dll stdcall';
function AllowSetForegroundWindow(ProcessId: Cardinal): BOOL;
  external 'AllowSetForegroundWindow@user32.dll stdcall';

// A running window locks gozler-ui.exe and does not exit when the Restart
// Manager asks it to. So this asks it to exit as File > Exit does, the
// request a start of another build sends (DESIGN.md: Window lifetime): it
// exits at once, or asks the person first while file operations run. True
// when no window runs any more.
function CloseWindow: Boolean;
var
  Window: HWND;
  ProcessId: Cardinal;
  Answer: Longint;
  Waited: Integer;
begin
  Window := FindWindowByClassName('{#WindowClass}');
  if Window <> 0 then
  begin
    // Lets the window and its Exit question come in front of the setup.
    GetWindowThreadProcessId(Window, ProcessId);
    AllowSetForegroundWindow(ProcessId);
    // No timeout while the window answers other messages, so the person can
    // take their time over the Exit question.
    if (SendMessageTimeout(Window, RegisterWindowMessage('{#ExitMessage}'), 0, 0,
      SMTO_ABORTIFHUNG or SMTO_NOTIMEOUTIFNOTHUNG, 5000, Answer) <> 0) and
      (Answer = {#ExitCloses}) then
    begin
      // The process holds the mutex until it has joined its threads, which
      // can take longer than its window.
      Waited := 0;
      while CheckForMutexes('{#InstanceMutex}') and (Waited < 10000) do
      begin
        Sleep(100);
        Waited := Waited + 100;
      end;
    end;
  end;
  Result := not CheckForMutexes('{#InstanceMutex}');
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  // Gozler closes when the person selects Install, not when Setup starts. A
  // failure here keeps Back, so the person can exit Gozler and try again.
  if not CloseWindow then
  begin
    Result := CustomMessage('WindowStays');
    exit;
  end;
  // The running daemon locks gozlerd.exe, and "gozlerd install" does not
  // restart a service that already runs, so stop it before the files change.
  // net.exe waits for the stop; its error when nothing runs is expected.
  Exec(ExpandConstant('{sys}\net.exe'), 'stop {#ServiceName}', '', SW_HIDE,
    ewWaitUntilTerminated, Code);
  Log(Format('net stop {#ServiceName}: code=%d', [Code]));
  // The stopped service no longer holds its log open.
  MatchFolderCase(ExpandConstant('{app}'));
  MatchFolderCase(ExpandConstant('{#LogFolder}'));
  Result := '';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Code, I: Integer;
  Parameters: String;
begin
  if CurStep <> ssPostInstall then exit;
  AddAppToPath;
  WizardForm.StatusLabel.Caption := CustomMessage('StartService');
  // "install" registers the service, or points an existing one at this
  // gozlerd.exe, and starts it. With --see-every-file it first stores the
  // choice made on SeeEveryFilePage, so the first search follows it.
  Parameters := 'install';
  if SeeEveryFilePage <> nil then
    if SeeEveryFilePage.Values[0] then
      Parameters := Parameters + ' --see-every-file "' + SignedInUser + '"'
    else
      Parameters := Parameters + ' --see-every-file none';
  if not Exec(ExpandConstant('{app}\gozlerd.exe'), Parameters, '', SW_HIDE,
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

// Removes what the window wrote for its user, the values in
// gozler-ui's win::shell, so Windows neither starts a missing program at
// sign-in nor shows "Search with Gozler" in File Explorer. Uninstall runs
// elevated, so HKCU is the hive of the account that elevated it.
procedure RemoveWindowRegistration;
begin
  RegDeleteValue(HKCU, 'Software\Microsoft\Windows\CurrentVersion\Run', 'gozler');
  RegDeleteValue(HKCU,
    'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run', 'gozler');
  RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\Directory\shell\gozler');
  RegDeleteKeyIncludingSubkeys(HKCU, 'Software\Classes\Directory\Background\shell\gozler');
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // After the person confirms the uninstall. Abort ends it without a
  // message.
  if CurUninstallStep = usAppMutexCheck then
    while not CloseWindow do
      if SuppressibleMsgBox(SetupMessage(msgUninstallAppRunningError), mbError,
        MB_OKCANCEL, IDCANCEL) <> IDOK then
        Abort;
  if CurUninstallStep = usUninstall then
  begin
    RemoveFolderFromPath(HKLM, EnvironmentKey, ExpandConstant('{app}'));
    RemoveWindowRegistration;
  end;
end;
