# Test VM only: sign vagrant out and let automatic sign-in bring a new session
# (like an employee signing out and in again). Waits up to 2 minutes.
$wl = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"
Set-ItemProperty $wl ForceAutoLogon "1"
$id = (Get-Process explorer -IncludeUserName | Where-Object UserName -like "*vagrant" | Select-Object -First 1).SessionId
& logoff.exe $id
$end = (Get-Date).AddSeconds(120)
do { Start-Sleep 1; $new = Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object { $_.UserName -like "*vagrant" -and $_.SessionId -ne $id } } while (-not $new -and (Get-Date) -lt $end)
Set-Content C:\rider-test\relogon.out "old $id new $($new.SessionId)"
