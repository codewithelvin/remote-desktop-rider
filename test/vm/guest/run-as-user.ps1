# Test VM only: run a script from C:\rider-test in the signed-in user's
# (vagrant's) desktop session, which WinRM cannot reach directly. Returns at
# once; the script keeps running there.
#   vagrant winrm -c 'powershell -File C:\rider-test\run-as-user.ps1 target.ps1'
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
