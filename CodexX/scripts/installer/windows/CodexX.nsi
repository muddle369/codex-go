Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!define ROOT "..\..\.."

Name "CodexGO"
OutFile "${ROOT}\dist\windows\CodexGO-${VERSION}-windows-x64-setup.exe"
InstallDir "$LOCALAPPDATA\Programs\CodexGO"
InstallDirRegKey HKCU "Software\CodexGO" "InstallDir"
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

  nsExec::ExecToLog 'taskkill /IM codexgo.exe /F'
  Pop $0
  nsExec::ExecToLog 'taskkill /IM codexgo-manager.exe /F'
  Pop $0

  File "${ROOT}\dist\windows\app\codexgo.exe"
  File "${ROOT}\dist\windows\app\codexgo-manager.exe"
  File "${ROOT}\dist\windows\app\WebView2Loader.dll"
  File "${ROOT}\assets\images\codex-go.ico"
  File "${ROOT}\assets\images\tray-icon.ico"

  Delete "$DESKTOP\CodexGO 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\CodexGO\CodexGO 绠＄悊宸ュ叿.lnk"

  CreateShortcut "$DESKTOP\CodexGO.lnk" "$INSTDIR\codexgo.exe" "" "$INSTDIR\codexgo.exe" 0
  CreateDirectory "$SMPROGRAMS\CodexGO"
  CreateShortcut "$SMPROGRAMS\CodexGO\CodexGO.lnk" "$INSTDIR\codexgo.exe" "" "$INSTDIR\codexgo.exe" 0
  CreateShortcut "$SMPROGRAMS\CodexGO\卸载 CodexGO.lnk" "$INSTDIR\uninstall.exe" "" "$INSTDIR\codexgo.exe" 0

  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKCU "Software\CodexGO" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO" "DisplayName" "CodexGO"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO" "Publisher" "BigPizzaV3"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO" "DisplayIcon" "$INSTDIR\codexgo.exe"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO" "UninstallString" "$INSTDIR\uninstall.exe"
SectionEnd

Section "Uninstall"
  nsExec::ExecToLog 'taskkill /IM codexgo.exe /F'
  Pop $0
  nsExec::ExecToLog 'taskkill /IM codexgo-manager.exe /F'
  Pop $0

  Delete "$DESKTOP\CodexGO.lnk"
  Delete "$DESKTOP\CodexGO Manager.lnk"
  Delete "$DESKTOP\CodexGO 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\CodexGO\CodexGO.lnk"
  Delete "$SMPROGRAMS\CodexGO\CodexGO Manager.lnk"
  Delete "$SMPROGRAMS\CodexGO\CodexGO 绠＄悊宸ュ叿.lnk"
  Delete "$SMPROGRAMS\CodexGO\卸载 CodexGO.lnk"
  RMDir "$SMPROGRAMS\CodexGO"

  Delete "$INSTDIR\codexgo.exe"
  Delete "$INSTDIR\codexgo-manager.exe"
  Delete "$INSTDIR\WebView2Loader.dll"
  Delete "$INSTDIR\codex-go.ico"
  Delete "$INSTDIR\tray-icon.ico"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\CodexGO"
  DeleteRegKey HKCU "Software\CodexGO"
SectionEnd
