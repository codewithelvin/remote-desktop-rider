# Test VM only: start chat-viewer.ps1 in the signed-in user's session (WinRM
# cannot pass it arguments intact; it reads chat-viewer.args).
# The installed app could come to the front and take the keys meant for the session window.
Get-Process rider-client, RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
Remove-Item C:\rider-test\chat-viewer.out, C:\rider-test\received-* -ErrorAction SilentlyContinue
& C:\rider-test\run-as-user.ps1 chat-viewer.ps1
