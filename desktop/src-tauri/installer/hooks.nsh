; The Windows installer's look. Tauri includes this file before its NSIS template defines the
; pages (bundle > windows > nsis > installerHooks in tauri.conf.json), so the Modern UI settings
; below apply to them; NSIS_HOOK_POSTUNINSTALL at the end cleans up. The texts are English only, like the
; installer (bundle > windows > nsis > languages is unset).

; Windows' own UI font rather than the dialog default (Tahoma on Windows 10 and 11).
SetFont "Segoe UI" 9

; Inner pages: title and description on the left, the app icon on the right (header.bmp).
!define MUI_HEADERIMAGE_RIGHT

; Welcome and finish pages, beside sidebar.bmp.
!define MUI_WELCOMEPAGE_TITLE "Install Akev"
!define MUI_WELCOMEPAGE_TEXT "Intercept, inspect, mock and replay HTTP(S) traffic from browsers, apps, terminals and devices.$\r$\n$\r$\nAkev installs for your Windows account. Click Next to continue."
!define MUI_FINISHPAGE_TITLE "Akev is ready"
!define MUI_FINISHPAGE_TEXT "Akev is installed and in your Start menu."
!define MUI_FINISHPAGE_RUN_TEXT "Start Akev"

; Uninstall: when "Delete the application data" is ticked, also remove Akev's own data
; directory (settings, rules, sessions, the CA and its key), which lives under the app's name
; rather than Tauri's bundle identifier. Local and Roaming: older versions used Roaming.
; The CA is removed from the Windows certificate store too, if Akev added it there.
!macro NSIS_HOOK_POSTUNINSTALL
  ${If} $DeleteAppDataCheckboxState = 1
    IfFileExists "$LOCALAPPDATA\akev\ca.pem" 0 +2
      nsExec::Exec 'powershell -NoProfile -NonInteractive -Command "$$c = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 \"$LOCALAPPDATA\akev\ca.pem\"; Get-ChildItem Cert:\CurrentUser\Root | Where-Object { $$_.Thumbprint -eq $$c.Thumbprint } | Remove-Item"'
    IfFileExists "$APPDATA\akev\ca.pem" 0 +2
      nsExec::Exec 'powershell -NoProfile -NonInteractive -Command "$$c = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 \"$APPDATA\akev\ca.pem\"; Get-ChildItem Cert:\CurrentUser\Root | Where-Object { $$_.Thumbprint -eq $$c.Thumbprint } | Remove-Item"'
    RMDir /r "$LOCALAPPDATA\akev"
    RMDir /r "$APPDATA\akev"
  ${EndIf}
!macroend
