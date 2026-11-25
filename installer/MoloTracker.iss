#ifndef AppVersion
  #error AppVersion is required. Run installer/build.ps1.
#endif

[Setup]
AppId=MoloTracker
AppName=Molo Tracker
AppVersion={#AppVersion}
AppPublisher=Molo Tracker
AppPublisherURL=https://github.com/AmirMolotov/molo-tracker
DefaultDirName={localappdata}\Programs\Molo Tracker
DisableProgramGroupPage=yes
DisableDirPage=auto
AppMutex=Local\MoloTracker.FocusRecorder
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\assets\molo.ico
UninstallDisplayIcon={app}\molo-tracker.exe
LicenseFile=Terms.txt
WizardStyle=modern
OutputDir=..\target\installer
OutputBaseFilename=MoloTracker-Setup-{#AppVersion}-x64

[Files]
Source: "..\target\release\molo-tracker.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "Terms.txt"; DestDir: "{app}"
Source: "..\assets\fonts\*-OFL.txt"; DestDir: "{app}\licenses"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Icons]
Name: "{userprograms}\Molo Tracker"; Filename: "{app}\molo-tracker.exe"; WorkingDir: "{app}"
Name: "{userdesktop}\Molo Tracker"; Filename: "{app}\molo-tracker.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\molo-tracker.exe"; Description: "Launch Molo Tracker"; Flags: nowait postinstall skipifsilent unchecked

[Code]
const
  StartupKey = 'Software\Microsoft\Windows\CurrentVersion\Run';
  StartupName = 'Molo Tracker';

function InstalledStartupCommand: String;
begin
  Result := '"' + ExpandConstant('{app}\molo-tracker.exe') + '"';
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  Command: String;
begin
  if (CurStep = ssPostInstall) and RegValueExists(HKCU, StartupKey, StartupName) then
  begin
    Command := InstalledStartupCommand;
    if Length(Command) <= 260 then
      if not RegWriteStringValue(HKCU, StartupKey, StartupName, Command) then
        Log('Could not update the Molo Tracker startup entry.');
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  Command: String;
begin
  if (CurUninstallStep = usPostUninstall) and
     RegQueryStringValue(HKCU, StartupKey, StartupName, Command) and
     (CompareText(Command, InstalledStartupCommand) = 0) then
    if not RegDeleteValue(HKCU, StartupKey, StartupName) then
      Log('Could not remove the Molo Tracker startup entry.');
end;
