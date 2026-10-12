; Prism also starts from a sign-in task. Explorer starts Run entries one
; after another, late in a busy sign-in, and until Prism runs the Win key
; opens the native Start menu. The Run entry stays as Task Manager's switch:
; Prism exits at once when the switch is off or another Prism runs. A task
; for the current user only needs no administrator rights.
!define PRISM_TASK_NAME "Prism Startup"

!macro PrismWriteStartupTask
  Push $0
  Push $1
  Push $2
  InitPluginsDir
  ReadEnvStr $1 USERDOMAIN
  ReadEnvStr $2 USERNAME
  FileOpen $0 "$PLUGINSDIR\prism-startup-task.xml" w
  FileWriteUTF16LE /BOM $0 '<?xml version="1.0" encoding="UTF-16"?>$\r$\n'
  FileWriteUTF16LE $0 '<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">$\r$\n'
  FileWriteUTF16LE $0 '  <RegistrationInfo><Description>Starts Prism when you sign in.</Description></RegistrationInfo>$\r$\n'
  FileWriteUTF16LE $0 '  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>$1\$2</UserId></LogonTrigger></Triggers>$\r$\n'
  FileWriteUTF16LE $0 '  <Principals><Principal id="Author"><UserId>$1\$2</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>$\r$\n'
  FileWriteUTF16LE $0 '  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Priority>4</Priority></Settings>$\r$\n'
  FileWriteUTF16LE $0 '  <Actions Context="Author"><Exec><Command>"$INSTDIR\prism.exe"</Command><Arguments>--autostart</Arguments><WorkingDirectory>$INSTDIR</WorkingDirectory></Exec></Actions>$\r$\n'
  FileWriteUTF16LE $0 '</Task>$\r$\n'
  FileClose $0
  nsExec::Exec '"$SYSDIR\schtasks.exe" /create /tn "${PRISM_TASK_NAME}" /xml "$PLUGINSDIR\prism-startup-task.xml" /f'
  Pop $0
  Delete "$PLUGINSDIR\prism-startup-task.xml"
  Pop $2
  Pop $1
  Pop $0
!macroend

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
  !insertmacro PrismWriteStartupTask
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
    nsExec::Exec '"$SYSDIR\schtasks.exe" /delete /tn "${PRISM_TASK_NAME}" /f'
    Pop $0
  ${EndIf}
!macroend
