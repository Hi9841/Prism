!macro NSIS_HOOK_PREINSTALL
  ; An update overwrites the existing installation in place. Restore the
  ; legacy Start-menu override before Tauri's installer stops the old process
  ; and replaces its executable.
  IfFileExists "$INSTDIR\prism.exe" 0 +2
  ExecWait '"$INSTDIR\prism.exe" --prism-restore-start-menu "$APPDATA\app.prism.launcher\start-menu-restore.json"'
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Register the installed executable for the current user's next sign-in.
  ; A named Run entry is surfaced by Windows in Task Manager > Startup apps.
  WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Prism" '"$INSTDIR\prism.exe" --autostart'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Restore any Start-menu provider setting before Prism and its recovery
  ; watchdog are removed. The helper exits before Tauri initializes.
  IfFileExists "$INSTDIR\prism.exe" 0 +2
  ExecWait '"$INSTDIR\prism.exe" --prism-restore-start-menu "$APPDATA\app.prism.launcher\start-menu-restore.json"'
!macroend
