$name = "rider-test-agent"
$home_ = "C:\ProgramData\rider-test-agent"
New-Item -ItemType Directory -Force $home_ | Out-Null
Set-Content "$home_\launch.ps1" @'
while (-not (Test-Path C:\rider-test\agent.ps1)) { Start-Sleep -Milliseconds 500 }
& C:\rider-test\agent.ps1
'@
$a = New-ScheduledTaskAction -Execute "powershell.exe" `
    -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File $home_\launch.ps1"
$t = New-ScheduledTaskTrigger -AtStartup
$p = New-ScheduledTaskPrincipal -UserId "SYSTEM" -LogonType ServiceAccount -RunLevel Highest
$s = New-ScheduledTaskSettingsSet -ExecutionTimeLimit ([TimeSpan]::Zero) -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries
Register-ScheduledTask -TaskName $name -Action $a -Trigger $t -Principal $p -Settings $s -Force | Out-Null
Stop-ScheduledTask -TaskName $name -ErrorAction SilentlyContinue
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" |
    Where-Object { ($_.CommandLine -like '*\rider-test\agent.ps1*' -or $_.CommandLine -like '*rider-test-agent\launch.ps1*') -and
        $_.ProcessId -ne $PID } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Start-ScheduledTask -TaskName $name
"agent started"
