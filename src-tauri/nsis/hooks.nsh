; Windows Firewall rules for LocalDrop (PRD §7.1).
;
; Private and Domain profiles only. The Public profile is deliberately excluded:
; broadcasting device presence on an untrusted network is not a shippable
; default. Users who want it can add the rule manually.
;
; Ports match protocol.rs — UDP 57321 (discovery), TCP 57322 (transfer). The
; +9 fallback range (§6.2) is covered so a port collision does not silently
; break discovery behind the firewall.

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Adding Windows Firewall rules for LocalDrop..."

  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="LocalDrop Discovery (UDP-In)" dir=in action=allow protocol=UDP localport=57321-57330 profile=private,domain program="$INSTDIR\LocalDrop.exe" enable=yes'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="LocalDrop Discovery (UDP-Out)" dir=out action=allow protocol=UDP localport=57321-57330 profile=private,domain program="$INSTDIR\LocalDrop.exe" enable=yes'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="LocalDrop Transfer (TCP-In)" dir=in action=allow protocol=TCP localport=57322-57331 profile=private,domain program="$INSTDIR\LocalDrop.exe" enable=yes'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall add rule name="LocalDrop Transfer (TCP-Out)" dir=out action=allow protocol=TCP profile=private,domain program="$INSTDIR\LocalDrop.exe" enable=yes'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  DetailPrint "Removing Windows Firewall rules for LocalDrop..."

  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="LocalDrop Discovery (UDP-In)"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="LocalDrop Discovery (UDP-Out)"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="LocalDrop Transfer (TCP-In)"'
  Pop $0
  nsExec::ExecToLog 'netsh advfirewall firewall delete rule name="LocalDrop Transfer (TCP-Out)"'
  Pop $0
!macroend
