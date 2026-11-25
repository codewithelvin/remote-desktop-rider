Get-Process rider-client, RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item C:\rider-test\chat-viewer.out, C:\rider-test\received-* -ErrorAction SilentlyContinue
& C:\rider-test\run-as-user.ps1 chat-viewer.ps1
