$ErrorActionPreference = "Stop"
$pf = "C:\Program Files\RemoteDesktopRider"
Stop-Service RemoteDesktopRider -ErrorAction SilentlyContinue
Get-Process RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $pf | Out-Null
Copy-Item C:\rider\RemoteDesktopRider.exe $pf -Force
& C:\rider\rider-host.exe service install
if ($LASTEXITCODE) { throw "service install failed" }
"updated"
