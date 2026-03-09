; NSIS installer hooks for AuraTerm
; Adds "Open in AuraTerm" to Windows Explorer folder context menu

!macro NSIS_HOOK_POSTINSTALL
  ; Add context menu entry for directories
  WriteRegStr HKCU "Software\Classes\Directory\shell\AuraTerm" "" "Open in AuraTerm"
  WriteRegStr HKCU "Software\Classes\Directory\shell\AuraTerm" "Icon" "$INSTDIR\AuraTerm.exe,0"
  WriteRegStr HKCU "Software\Classes\Directory\shell\AuraTerm\command" "" '"$INSTDIR\AuraTerm.exe" "%V"'

  ; Add context menu entry for directory background (right-click in empty space)
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\AuraTerm" "" "Open in AuraTerm"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\AuraTerm" "Icon" "$INSTDIR\AuraTerm.exe,0"
  WriteRegStr HKCU "Software\Classes\Directory\Background\shell\AuraTerm\command" "" '"$INSTDIR\AuraTerm.exe" "%V"'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Remove context menu entries
  DeleteRegKey HKCU "Software\Classes\Directory\shell\AuraTerm"
  DeleteRegKey HKCU "Software\Classes\Directory\Background\shell\AuraTerm"
!macroend
