# Test VM only: start agent.ps1 now and at every boot, as SYSTEM (full
# rights like WinRM, so run-as-user.ps1 works from it). Run by the
# Vagrantfile on every `vagrant up`.
$name = "rider-test-agent"
$a = New-ScheduledTaskAction -Execute "powershell.exe" `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File C:\rider-test\agent.ps1"
$t = New-ScheduledTaskTrigger -AtStartup
$p = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $name -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null
Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" |
    # Not this script ("install-agent.ps1" contains "agent.ps1" too).
    Where-Object { $_.CommandLine -like '*\rider-test\agent.ps1*' -and $_.ProcessId -ne $PID } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Start-ScheduledTask -TaskName $name
"agent started"
