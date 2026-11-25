param([Parameter(Mandatory)][string]$Script, [string]$Arguments = "")
$name = "rider-" + [IO.Path]::GetFileNameWithoutExtension($Script)
$a = New-ScheduledTaskAction -Execute "powershell.exe" `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\rider-test\$Script $Arguments"
$p = New-ScheduledTaskPrincipal -UserId "vagrant" -LogonType Interactive
Register-ScheduledTask -TaskName $name -Action $a -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName $name
Start-Sleep 2
Unregister-ScheduledTask -TaskName $name -Confirm:$false
"started $Script"
