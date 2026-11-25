# Test VM only: how many request/chat windows (`RemoteDesktopRider
# --session-panel`) are running now (C:\rider-test\ent-panel.out).
$n = @(Get-CimInstance Win32_Process -Filter "Name='RemoteDesktopRider.exe'" |
    Where-Object { $_.CommandLine -like '*--session-panel*' }).Count
Set-Content C:\rider-test\ent-panel.out $n
