; neoprint — Windows installer (Inno Setup 6).
;
; Per-user: no administrator rights, installs to %LOCALAPPDATA%\Programs\Neoprint,
; registers "start with Windows" in HKCU\...\Run, and launches neoprint when
; the wizard finishes (also in /VERYSILENT mode). Built by
; .github/workflows/neoprint-build.yml:
;   ISCC.exe /DAppVersion=<version> /DSourceExe=<path to neoprint.exe> neoprint.iss
; and optionally signed there (signtool) when a code-signing cert secret exists.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\x86_64-pc-windows-msvc\release\neoprint.exe"
#endif

[Setup]
AppId={{5D8B6E52-2C0B-4E3C-9A57-6B9F0C0A4E11}
AppName=neoprint
AppVersion={#AppVersion}
AppPublisher=NeoGeniux
AppPublisherURL=https://neogeniux.com
DefaultDirName={localappdata}\Programs\Neoprint
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=Output
OutputBaseFilename=neoprint-setup
Compression=lzma2
SolidCompression=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
WizardStyle=modern
UninstallDisplayName=neoprint (impresión silenciosa)
UninstallDisplayIcon={app}\neoprint.exe
SetupIconFile=..\..\assets\neoprint.ico
CloseApplications=no
RestartApplications=no

[Languages]
Name: "spanish"; MessagesFile: "compiler:Languages\Spanish.isl"

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "neoprint.exe"; Flags: ignoreversion
; SumatraPDF (GPLv3) prints PDFs silently, so cashiers need no Adobe Reader.
; CI downloads + hash-verifies it into vendor\ before compiling; neoprint falls
; back to the Windows PrintTo verb when it is absent.
#ifexist "vendor\SumatraPDF.exe"
Source: "vendor\SumatraPDF.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "vendor\SUMATRA-LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
#endif

[Registry]
; Start with Windows. `--serve` skips the first-run self-installer.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Neoprint"; ValueData: """{app}\neoprint.exe"" --serve"; Flags: uninsdeletevalue

[Icons]
Name: "{userprograms}\neoprint"; Filename: "{app}\neoprint.exe"; Parameters: "--serve"; IconFilename: "{app}\neoprint.exe"

[Run]
; Start the sidecar right away (the tray icon appears), even in silent mode.
Filename: "{app}\neoprint.exe"; Parameters: "--serve"; Flags: nowait runhidden

[Code]
// A running neoprint locks its exe: stop it before files are replaced / removed.
procedure StopNeoprint();
var
  ResultCode: Integer;
begin
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/IM neoprint.exe /F', '', SW_HIDE,
    ewWaitUntilTerminated, ResultCode);
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
begin
  StopNeoprint();
  Result := '';
end;

function InitializeUninstall(): Boolean;
begin
  StopNeoprint();
  Result := True;
end;
