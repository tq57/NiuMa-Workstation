; NiuMa-Workstation · 安装版（Inno Setup 6）
; 由 scripts/package.ps1 调用，版本号通过 /DAppVersion= 传入。

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

#define AppName "NiuMa-Workstation"
#define AppExeName "NiuMa-Workstation.exe"

[Setup]
AppId={{7B3E6B4A-6C1E-4F4E-9B5A-1B2C3D4E5F60}
AppName={#AppName}
AppVersion={#AppVersion}
AppPublisher=NiuMa-Workstation
DefaultDirName={localappdata}\Programs\NiuMa-Workstation
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=..\dist
OutputBaseFilename=NiuMa-Workstation-{#AppVersion}-setup
SetupIconFile=..\crates\app\assets\icon.ico
UninstallDisplayIcon={app}\{#AppExeName}
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
MinVersion=10.0
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\NiuMa-Workstation.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExeName}"
Name: "{group}\卸载 {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExeName}"; Tasks: desktopicon

[Run]
Filename: "{app}\{#AppExeName}"; Description: "{cm:LaunchProgram,{#StringChange(AppName, '&', '&&')}}"; Flags: nowait postinstall skipifsilent
; 应用内更新：应用以 /AUTOUPDATE=1 调起安装包，装完自动拉起新版本
Filename: "{app}\{#AppExeName}"; Flags: nowait; Check: IsAutoUpdate

[Code]
function IsAutoUpdate: Boolean;
begin
  Result := ExpandConstant('{param:AUTOUPDATE|0}') = '1';
end;
