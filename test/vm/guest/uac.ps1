$a = New-ScheduledTaskAction -Execute "powershell.exe" -Argument '-WindowStyle Hidden -Command "Start-Process cmd.exe -Verb RunAs -ArgumentList ''/k title RIDER-ELEVATED'' "'
$p = New-ScheduledTaskPrincipal -UserId "vagrant" -LogonType Interactive -RunLevel Limited
Register-ScheduledTask -TaskName "rider-uac" -Action $a -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "rider-uac"
Start-Sleep 3
Unregister-ScheduledTask -TaskName "rider-uac" -Confirm:$false
"uac requested"
