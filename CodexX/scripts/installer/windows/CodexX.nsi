Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define ROOT "..\..\.."

Name "CodexX"
OutFile "${ROOT}\dist\windows\CodexX-${VERSION}-windows-x64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\CodexX"
InstallDirRegKey HKCU "Software\CodexX" "InstallDir"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!define MUI_ICON "${ROOT}\apps\codexx-manager\src-tauri\icons\icon.ico"
!define MUI_UNICON "${ROOT}\apps\codexx-manager\src-tauri\icons\icon.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "SimpChinese"
!insertmacro MUI_LANGUAGE "English"

Section "Install"
  SetOutPath "$INSTDIR"

  nsExec::ExecToLog 'taskkill /IM codexx.exe /F'
  Pop $0
  nsExec::ExecToLog 'taskkill /IM codexx-manager.exe /F'
  Pop $0

  File "${ROOT}\dist\windows\app\codexx.exe"
  File "${ROOT}\dist\windows\app\codexx-manager.exe"

  Delete "$DESKTOP\CodexX 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\CodexX\CodexX 绠＄悊宸ュ叿.lnk"

  CreateShortcut "$DESKTOP\CodexX.lnk" "$INSTDIR\codexx.exe" "" "$INSTDIR\codexx.exe"
  CreateShortcut "$DESKTOP\CodexX Manager.lnk" "$INSTDIR\codexx-manager.exe" "" "$INSTDIR\codexx-manager.exe"
  CreateDirectory "$SMPROGRAMS\CodexX"
  CreateShortcut "$SMPROGRAMS\CodexX\CodexX.lnk" "$INSTDIR\codexx.exe" "" "$INSTDIR\codexx.exe"
  CreateShortcut "$SMPROGRAMS\CodexX\CodexX Manager.lnk" "$INSTDIR\codexx-manager.exe" "" "$INSTDIR\codexx-manager.exe"
  CreateShortcut "$SMPROGRAMS\CodexX\卸载 CodexX.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\codexx-manager.exe"

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\CodexX" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX" "DisplayName" "CodexX"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX" "Publisher" "BigPizzaV3"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX" "DisplayIcon" "$INSTDIR\codexx-manager.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX" "UninstallString" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'taskkill /IM codexx.exe /F'
  Pop $0
  nsExec::ExecToLog 'taskkill /IM codexx-manager.exe /F'
  Pop $0

  Delete "$DESKTOP\CodexX.lnk"
  Delete "$DESKTOP\CodexX Manager.lnk"
  Delete "$DESKTOP\CodexX 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\CodexX\CodexX.lnk"
  Delete "$SMPROGRAMS\CodexX\CodexX Manager.lnk"
  Delete "$SMPROGRAMS\CodexX\CodexX 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\CodexX\卸载 CodexX.lnk"
  RMDir "$SMPROGRAMS\CodexX"

  Delete "$INSTDIR\codexx.exe"
  Delete "$INSTDIR\codexx-manager.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexX"
  DeleteRegKey HKCU "Software\CodexX"
SectionEnd
