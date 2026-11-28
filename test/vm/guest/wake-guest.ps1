$exe = "C:\Program Files\RemoteDesktopRider\rider-host.exe"
if (-not (Test-Path $exe)) { $exe = "C:\rider\rider-host.exe" }
$out = "C:\rider-test\wake-guest.out"
$words = (Get-Content "C:\rider-test\wake-args.txt" -Raw).Trim() -split '\s+'
$action = $words[0]
switch ($action) {
    "server" {
        $r = & $exe set-server $words[1] $words[2] 2>&1
        Restart-Service RemoteDesktopRider -Force
        $r = "$r restarted"
    }
    "stop" { Stop-Service RemoteDesktopRider -Force; $r = (Get-Service RemoteDesktopRider).Status }
    "start" { Start-Service RemoteDesktopRider; $r = (Get-Service RemoteDesktopRider).Status }
    "helper" {
        $log = "C:\rider-test\wake-helper.log"
        Remove-Item $log -ErrorAction SilentlyContinue
        $cmd = "`$env:RUST_LOG='info'; & '$exe' support $($words[1]) $($words[2]) *> '$log'"
        Start-Process powershell -WindowStyle Hidden -ArgumentList "-NoProfile", "-Command", $cmd
        $r = "started"
    }
    "unhelp" {
        Get-CimInstance Win32_Process -Filter "Name = 'rider-host.exe'" |
            Where-Object { $_.CommandLine -match ' support ' } |
            ForEach-Object { Stop-Process -Id $_.ProcessId -Force }
        $r = "stopped"
    }
    "macs" {
        $r = (Get-NetAdapter -Physical | ForEach-Object { $_.MacAddress }) -join ' '
    }
    "armed" { $r = (powercfg /devicequery wake_armed) -join '|' }
}
"$action $r" | Set-Content $out
