; WiringCraft setup for Windows (Inno Setup 6). Built by .github/workflows/release.yml:
;   iscc /DAppVersion=0.1.0 /DSourceDir=<folder with the exes> /DOutputDir=<out> wiringcraft.iss
; Installs per user (no administrator rights needed), adds a Start menu entry for the app and
; puts the install folder on the user's PATH so `wiringcraft` works in a new terminal. The
; uninstaller removes all of it again, the PATH entry included.

#ifndef AppVersion
  #error AppVersion is not set: pass /DAppVersion=x.y.z
#endif
#ifndef SourceDir
  #error SourceDir is not set: pass /DSourceDir=<folder>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{6F1C2B7E-3D8A-4C55-9E0B-2A7F4D1C8B93}
AppName=WiringCraft
AppVersion={#AppVersion}
AppPublisher=WiringCraft
AppPublisherURL=https://github.com/phobicdotno/wiringcraft
DefaultDirName={localappdata}\Programs\WiringCraft
DefaultGroupName=WiringCraft
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
ChangesEnvironment=yes
OutputDir={#OutputDir}
OutputBaseFilename=wiringcraft-{#AppVersion}-windows-x64-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile={#SourceDir}\LICENSE-MIT
UninstallDisplayIcon={app}\wiringcraft-app.exe
WizardStyle=modern
; The setup program's own icon (the installed programs carry theirs, see icon_build.rs).
SetupIconFile={#SourceDir}\wiringcraft.ico

[Files]
Source: "{#SourceDir}\wiringcraft-app.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\wiringcraft.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\DISCLAIMER.md"; DestDir: "{app}"; Flags: ignoreversion
; The starter symbols and parts; the app looks for them next to itself.
Source: "{#SourceDir}\library\*"; DestDir: "{app}\library"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\WiringCraft"; Filename: "{app}\wiringcraft-app.exe"
Name: "{group}\Uninstall WiringCraft"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\wiringcraft-app.exe"; Description: "Start WiringCraft"; Flags: nowait postinstall skipifsilent

[Code]
const
  EnvKey = 'Environment';

function PathHas(Path, Dir: string): Boolean;
begin
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') > 0;
end;

procedure AddToPath(Dir: string);
var
  Path: string;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Path) then
    Path := '';
  if PathHas(Path, Dir) then
    exit;
  if (Path <> '') and (Copy(Path, Length(Path), 1) <> ';') then
    Path := Path + ';';
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Path + Dir);
end;

procedure RemoveFromPath(Dir: string);
var
  Path: string;
  P: Integer;
begin
  if not RegQueryStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Path) then
    exit;
  Path := ';' + Path + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Path));
  if P = 0 then
    exit;
  Delete(Path, P, Length(Dir) + 1);
  Path := Copy(Path, 2, Length(Path) - 2);
  RegWriteExpandStringValue(HKEY_CURRENT_USER, EnvKey, 'Path', Path);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if CurStep = ssPostInstall then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;
