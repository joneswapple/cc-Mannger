; NSIS installer hooks for CC Manager (bundle.windows.nsis.installerHooks).

!macro NSIS_HOOK_POSTINSTALL
  ; Let terminals on the private LAN reach the proxy. Needs elevation; a
  ; per-user install without it just skips the rule (Windows will ask on
  ; first listen instead).
  nsExec::Exec 'netsh advfirewall firewall add rule name="CC Manager" dir=in action=allow program="$INSTDIR\${MAINBINARYNAME}.exe" enable=yes profile=private'
  Pop $0
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::Exec 'netsh advfirewall firewall delete rule name="CC Manager"'
  Pop $0
!macroend
