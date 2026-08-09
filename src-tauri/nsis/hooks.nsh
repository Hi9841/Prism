!macro NSIS_HOOK_PREUNINSTALL
  ; Restore any Start-menu provider setting before Prism and its recovery
  ; watchdog are removed. The helper exits before Tauri initializes.
  IfFileExists "$INSTDIR\prism.exe" 0 +2
  ExecWait '"$INSTDIR\prism.exe" --prism-restore-start-menu "$APPDATA\app.prism.launcher\start-menu-restore.json"'
!macroend
