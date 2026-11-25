# Test VM only: install the freshly built host from C:\rider the way the
# installer lays it out: the service's rider-host.exe and the app
# (RemoteDesktopRider.exe, which also draws a paired session's chat window)
# side by side in Program Files.
$ErrorActionPreference = "Stop"
$pf = "C:\Program Files\RemoteDesktopRider"
Stop-Service RemoteDesktopRider -ErrorAction SilentlyContinue
Get-Process RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
New-Item -ItemType Directory -Force $pf | Out-Null
Copy-Item C:\rider\RemoteDesktopRider.exe $pf -Force
& C:\rider\rider-host.exe service install
if ($LASTEXITCODE) { throw "service install failed" }
"updated"
