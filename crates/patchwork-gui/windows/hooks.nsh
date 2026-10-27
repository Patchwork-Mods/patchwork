; Run by Tauri's NSIS installer (bundle.windows.nsis.installerHooks in
; tauri.conf.json). The patchwork CLI is installed next to the app, so its
; folder goes on the user's PATH, and comes off again on uninstall. Updates
; run the install hook again, which leaves PATH alone if it's already there.
; Uninstalling also removes the sync daemon's login item.
; ($$ is a literal $ to NSIS, for PowerShell's variables.)

; Runs SCRIPT on the user's PATH as the registry stores it, unexpanded, so
; entries like %USERPROFILE%\bin stay that way, and keeping its type. SCRIPT
; gets PATH's entries in $$p and this install's folder in $$d, and sets $$q
; to the entries PATH should have if it should change. The folder reaches
; PowerShell through the environment rather than in the script, so nothing
; in its name (like the ' in C:\Users\O'Brien) can break the script's quoting.
!macro PATCHWORK_EDIT_PATH SCRIPT
  System::Call 'Kernel32::SetEnvironmentVariable(t "PATCHWORK_DIR", t "$INSTDIR")'
  nsExec::Exec `powershell.exe -NoProfile -NonInteractive -Command "$$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$kind = if ($$k.GetValueNames() -contains 'Path') { $$k.GetValueKind('Path') } else { 'ExpandString' }; $$d = $$env:PATCHWORK_DIR.TrimEnd('\'); $$p = @($$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames') -split ';' | Where-Object { $$_ }); $$q = $$null; ${SCRIPT}; if ($$null -ne $$q) { $$k.SetValue('Path', ($$q -join ';'), $$kind) }"`
  ; WM_SETTINGCHANGE to every window, so new terminals see the change
  ; without signing out.
  SendMessage 0xFFFF 0x1A 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  !insertmacro PATCHWORK_EDIT_PATH `if (-not ($$p | Where-Object { $$_.TrimEnd('\') -eq $$d })) { $$q = $$p + $$d }`
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro PATCHWORK_EDIT_PATH `$$r = @($$p | Where-Object { $$_.TrimEnd('\') -ne $$d }); if ($$r.Count -ne $$p.Count) { $$q = $$r }`
  ; The sync daemon's login item, if Settings installed one (see
  ; daemon/service.rs in patchwork-sync); it'd only fail to start otherwise.
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "PatchworkSync"
!macroend
