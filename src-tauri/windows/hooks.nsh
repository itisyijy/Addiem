; Addiem was called Neo-Anki. Both use the same identifier, so they share one data folder,
; and the old program would otherwise stay installed next to Addiem (QA QB-02). Its uninstaller,
; run with "delete app data" ticked, would also wipe Addiem's data (QB-01).
; So on install, remove the old Neo-Anki program first. A silent uninstall never ticks
; "delete app data" (the box only exists on its confirm page), so the data folder stays.
!define OLD_PRODUCT "Neo-Anki"

!macro NSIS_HOOK_PREINSTALL
  Push $R0
  Push $R1
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Uninstall\${OLD_PRODUCT}" "InstallLocation"
  StrCpy $R1 $R0 1
  ${If} $R1 == '"'
    StrCpy $R0 $R0 -1 1
  ${EndIf}
  ${If} $R0 != ""
  ${AndIf} ${FileExists} "$R0\uninstall.exe"
    DetailPrint "Removing the previous ${OLD_PRODUCT} program (data is kept)"
    ; _?= runs the uninstaller in place so ExecWait really waits; it leaves itself behind.
    ExecWait '"$R0\uninstall.exe" /S _?=$R0'
    Delete "$R0\uninstall.exe"
    RMDir "$R0"
  ${EndIf}
  Pop $R1
  Pop $R0
!macroend
