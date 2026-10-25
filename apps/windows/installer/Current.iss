; The Windows installer (Inno Setup 7, ADR 0009): a per-user install without admin rights into %LOCALAPPDATA%\Programs\Current, with a
; Start menu shortcut that carries the app's AppUserModelID. It never names the data folder (%LOCALAPPDATA%\Current), so neither an
; upgrade nor uninstall can remove it. build-installer.ps1 passes AppVersion, SourceDir (the published app), InnoLicense and OutputDir.

#ifndef AppVersion
  #error Pass /DAppVersion=<version>
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<published app folder>
#endif
#ifndef InnoLicense
  #error Pass /DInnoLicense=<Inno Setup's license.txt>
#endif
#ifndef OutputDir
  #error Pass /DOutputDir=<folder for the setup file>
#endif

[Setup]
; Fixed forever: an install with the same AppId upgrades in place. The doubled brace is Inno Setup's escape for "{".
AppId={{21A6CF25-84E6-463B-84C5-304469A6490C}
AppName=Current
AppVersion={#AppVersion}
AppPublisher=Skeksik
AppVerName=Current {#AppVersion}
VersionInfoVersion={#AppVersion}
; Per user, no elevation: {autopf} is then the user's Program Files folder, %LOCALAPPDATA%\Programs.
PrivilegesRequired=lowest
DefaultDirName={autopf}\Current
DisableProgramGroupPage=yes
; A 64-bit installer (Inno Setup 7); this also makes x64compatible the default for ArchitecturesAllowed and
; ArchitecturesInstallIn64BitMode, set here anyway so the script says what it does.
SetupArchitecture=x64
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Setup and Uninstall refuse to go on while Current runs: the app holds this mutex (Current.Shell/AppIdentity.cs). Inno's AppMutex
; topic: "[Setup or Uninstall] has detected that [AppName] is currently running."
AppMutex=Skeksik.Current.Running
UninstallDisplayIcon={app}\Current.exe
UninstallDisplayName=Current
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
OutputDir={#OutputDir}
OutputBaseFilename=Current-{#AppVersion}-x64-setup

[Files]
; The published app as build.ps1 leaves it, which includes the libmpv DLL set next to Current.exe (mpv-2.dll, the six FFmpeg
; libraries, libplacebo, FriBidi, uchardet, shaderc and SPIRV-Cross) and its licence texts and components.md in licences\libmpv
; (docs/notes/windows-mpv-build.md; build-installer.ps1 checks both are there). The Visual C++ runtime they need is not installed
; here yet (ADR 0003; a roadmap follow-up).
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs
; The uninstaller (unins000.exe) is Inno Setup's: its licence asks that its notices be kept (ADR 0003).
Source: "{#InnoLicense}"; DestDir: "{app}\licences"; DestName: "InnoSetup-license.txt"; Flags: ignoreversion

[Icons]
; The same AppUserModelID as the process sets (Current.Shell/AppIdentity.cs), so the taskbar ties the window to this shortcut.
Name: "{autoprograms}\Current"; Filename: "{app}\Current.exe"; AppUserModelID: "Skeksik.Current"

[UninstallDelete]
; The program folder itself, once its files are gone; never anything else, and never the data folder.
Type: dirifempty; Name: "{app}"

[Run]
Filename: "{app}\Current.exe"; Description: "Open Current"; Flags: nowait postinstall skipifsilent
