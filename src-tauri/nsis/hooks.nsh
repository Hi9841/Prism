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
  ; Only write the value when it is missing: if the user disabled the entry
  ; (Task Manager records that in StartupApproved\Run), rewriting the Run
  ; value on every update would silently re-enable autostart.
  ReadRegStr $0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Prism"
  ${If} $0 == ""
    WriteRegStr HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Prism" '"$INSTDIR\prism.exe" --autostart'
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ; Restore any Start-menu provider setting before Prism and its recovery
  ; watchdog are removed. The helper exits before Tauri initializes.
  IfFileExists "$INSTDIR\prism.exe" 0 +2
  ExecWait '"$INSTDIR\prism.exe" --prism-restore-start-menu "$APPDATA\app.prism.launcher\start-menu-restore.json"'
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ; Real uninstalls (not updates) clean up the autostart registration so
  ; Task Manager never shows a dead Prism entry pointing at a deleted
  ; executable, and remove the local state, caches, and custom icons.
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Prism"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run" "Prism"
    RMDir /r "$APPDATA\app.prism.launcher"
  ${EndIf}
!macroend
