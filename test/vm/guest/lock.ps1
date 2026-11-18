$a = New-ScheduledTaskAction -Execute "rundll32.exe" -Argument "user32.dll,LockWorkStation"
$p = New-ScheduledTaskPrincipal -UserId "vagrant" -LogonType Interactive
Register-ScheduledTask -TaskName "rider-lock" -Action $a -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "rider-lock"
Start-Sleep 2
Unregister-ScheduledTask -TaskName "rider-lock" -Confirm:$false
"locked"
