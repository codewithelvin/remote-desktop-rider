"--- service"
Get-Service RemoteDesktopRider -ErrorAction SilentlyContinue | Format-List Name, Status, StartType
"--- rider processes"
Get-CimInstance Win32_Process | Where-Object Name -eq 'rider-host.exe' | ForEach-Object { "pid=$($_.ProcessId) session=$($_.SessionId) cmd=$($_.CommandLine)" }
"--- console session"
query session 2>&1
"--- logs"
Get-ChildItem C:\ProgramData\RemoteDesktopRider\host\logs -ErrorAction SilentlyContinue | ForEach-Object { "== $($_.Name)"; Get-Content $_.FullName -Tail 15 }
"--- firewall"
netsh advfirewall firewall show rule name="Remote Desktop Rider (UDP)" | Select-String "Enabled|LocalPort|Program"
