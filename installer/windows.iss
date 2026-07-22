; Inno Setup script -- builds the Windows installer.
;
; Build locally with:
;   iscc installer\windows.iss
; Override the version at build time (this is what CI does):
;   iscc /DMyAppVersion=1.2.3 installer\windows.iss
;
; Expects the release binary at target\release\afm_viewer.exe,
; so run `cargo build --release` first. Output lands in dist\.

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#define MyAppName "Kintuba AFM Viewer"
#define MyAppPublisher "Yuhei Yamada"
#define MyAppURL "https://github.com/IndigoCarmine/afm_files"
#define MyAppExeName "afm_viewer.exe"

; "x64compatible" (native x64 + ARM64 running x64 code) is preferred, but it only
; exists on Inno Setup 6.3+; older compilers error on it, so fall back to "x64".
#if Ver >= EncodeVer(6,3,0)
  #define ArchId "x64compatible"
#else
  #define ArchId "x64"
#endif

[Setup]
; A stable AppId is what makes upgrades replace the previous install instead of
; stacking up alongside it. Do not change this GUID once released.
AppId={{21B3D55F-F121-4C26-BF6C-FF041AE7CC81}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppVerName={#MyAppName} {#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
AppSupportURL={#MyAppURL}
AppUpdatesURL={#MyAppURL}/releases
DefaultDirName={autopf}\{#MyAppName}
DefaultGroupName={#MyAppName}
DisableProgramGroupPage=yes
; Lets the user pick a per-user install (no admin rights needed) or machine-wide.
PrivilegesRequiredOverridesAllowed=dialog commandline
OutputDir=..\dist
OutputBaseFilename=afm-viewer-{#MyAppVersion}-windows-x64-setup
SetupIconFile=..\assets\icon.ico
UninstallDisplayIcon={app}\{#MyAppExeName}
UninstallDisplayName={#MyAppName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed={#ArchId}
ArchitecturesInstallIn64BitMode={#ArchId}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "japanese"; MessagesFile: "compiler:Languages\Japanese.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\{#MyAppExeName}"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\icon.ico"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"
Name: "{group}\{cm:UninstallProgram,{#MyAppName}}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#MyAppName}"; Filename: "{app}\{#MyAppExeName}"; IconFilename: "{app}\icon.ico"; Tasks: desktopicon

[Run]
Filename: "{app}\{#MyAppExeName}"; Description: "{cm:LaunchProgram,{#MyAppName}}"; Flags: nowait postinstall skipifsilent
