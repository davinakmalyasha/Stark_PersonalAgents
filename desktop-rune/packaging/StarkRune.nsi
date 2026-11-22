; Stark Rune per-user bootstrap installer.
; First install/repair only: updates replace app/<version> and current.json
; through StarkRune.exe, so this never prompts during an update.
Unicode true
!include "MUI2.nsh"

!ifndef STAGE
  !error "STAGE is required"
!endif
!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef OUTFILE
  !define OUTFILE "StarkRune-Setup.exe"
!endif

Name "Stark Rune ${VERSION}"
OutFile "${OUTFILE}"
InstallDir "$LOCALAPPDATA\Programs\StarkRune"
InstallDirRegKey HKCU "Software\StarkRune" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
SetCompressorDictSize 32
BrandingText "Stark Rune"

!define MUI_ABORTWARNING
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_LANGUAGE "English"

Section "Stark Rune" SecMain
  SetOutPath "$INSTDIR"
  File /r "${STAGE}\*.*"
  CreateDirectory "$SMPROGRAMS\Stark Rune"
  CreateShortcut "$SMPROGRAMS\Stark Rune\Stark Rune.lnk" "$INSTDIR\StarkRune.exe"
  CreateShortcut "$DESKTOP\Stark Rune.lnk" "$INSTDIR\StarkRune.exe"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\StarkRune" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune" "DisplayName" "Stark Rune"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune" "Publisher" "Stark"
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune" "UninstallString" "$INSTDIR\Uninstall.exe"
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune" "NoModify" 1
  WriteRegDWORD HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune" "NoRepair" 1
SectionEnd

Section "Uninstall"
  RMDir /r "$INSTDIR\app"
  Delete "$INSTDIR\StarkRune.exe"
  Delete "$INSTDIR\current.json"
  Delete "$INSTDIR\pending.json"
  Delete "$INSTDIR\blocked.json"
  Delete "$INSTDIR\update.pub"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir /r "$INSTDIR\models"
  RMDir /r "$INSTDIR\licenses"
  RMDir /r "$INSTDIR\runtime"
  RMDir /r "$INSTDIR\updates"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Stark Rune\Stark Rune.lnk"
  RMDir "$SMPROGRAMS\Stark Rune"
  Delete "$DESKTOP\Stark Rune.lnk"
  DeleteRegKey HKCU "Software\StarkRune"
  DeleteRegKey HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\StarkRune"
  ; Data under %LOCALAPPDATA%\StarkRune\data is intentionally preserved.
SectionEnd

Function .onInstSuccess
  Exec "$INSTDIR\StarkRune.exe"
FunctionEnd
