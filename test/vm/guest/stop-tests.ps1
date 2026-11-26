Add-Content C:\rider-test\support.cmd "quit" -ErrorAction SilentlyContinue
Start-Sleep 2
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" |
    Where-Object { $_.CommandLine -like '*target.ps1*' -or $_.CommandLine -like '*support-auto.ps1*' -or $_.CommandLine -like '*pattern.ps1*' -or $_.CommandLine -like '*motion.ps1*' } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Remove-NetFirewallRule -DisplayName "rdr-test-block-udp" -ErrorAction SilentlyContinue
"stopped"
