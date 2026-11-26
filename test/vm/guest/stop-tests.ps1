Add-Content C:\rider-test\support.cmd "quit" -ErrorAction SilentlyContinue
Start-Sleep 2
$scripts = 'target.ps1', 'support-auto.ps1', 'pattern.ps1', 'motion.ps1', 'tcp-target.ps1', 'pair-auto.ps1', 'tunnel-panel.ps1'
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" |
    Where-Object { $line = $_.CommandLine; $scripts | Where-Object { $line -like "*$_*" } } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Remove-NetFirewallRule -DisplayName "rdr-test-block-udp" -ErrorAction SilentlyContinue
$pairing = Get-CimInstance Win32_Process -Filter "Name='rider-host.exe'" | Where-Object { $_.CommandLine -match '\s(pair|support)(\s|$)' }
$pairing | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Get-Process rider-client -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
if ($pairing -and (Get-Service RemoteDesktopRider -ErrorAction SilentlyContinue).Status -ne 'Running') {
    Start-Service RemoteDesktopRider -ErrorAction SilentlyContinue
}
"stopped"
