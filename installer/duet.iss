; Duet for Windows: the installer (Inno Setup 6), made by CI from PyInstaller's dist\Duet folder:
;   iscc /DAppVersion=0.3.110 /DSourceDir=..\dist\Duet /DOutDir=..\dist installer\duet.iss
; Installs for the current user only (no admin rights) into %LOCALAPPDATA%\Programs\Duet, with a
; Start menu shortcut and an uninstaller. Duet's updater runs it with /VERYSILENT: it closes Duet,
; replaces the files and starts Duet again. Songs and settings are in %APPDATA%\Duet and stay.

#ifndef AppVersion
  #define AppVersion "0.3.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\dist\Duet"
#endif
#ifndef OutDir
  #define OutDir "..\dist"
#endif

[Setup]
AppId={{6B1F3E2A-5C4D-4E8F-9A7B-D0E7C2A9F1B3}
AppName=Duet
AppVersion={#AppVersion}
AppPublisher=Duet
DefaultDirName={localappdata}\Programs\Duet
DisableProgramGroupPage=yes
DisableDirPage=yes
DisableReadyPage=yes
PrivilegesRequired=lowest
OutputDir={#OutDir}
OutputBaseFilename=Duet-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\Duet.exe
CloseApplications=force
RestartApplications=no
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[InstallDelete]
; what the last version had and this one doesn't must not linger
Type: filesandordirs; Name: "{app}\_internal"

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Duet"; Filename: "{app}\Duet.exe"
Name: "{autodesktop}\Duet"; Filename: "{app}\Duet.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "A shortcut on the desktop"; Flags: unchecked

[Run]
; after an install, and after a silent update too: start Duet
Filename: "{app}\Duet.exe"; Description: "Start Duet"; Flags: nowait postinstall
