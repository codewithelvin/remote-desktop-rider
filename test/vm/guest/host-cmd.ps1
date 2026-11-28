param([Parameter(Mandatory)][string]$Action, [string]$Value = "", [string]$Extra = "")
$exe = "C:\Program Files\RemoteDesktopRider\rider-host.exe"
if (-not (Test-Path $exe)) { $exe = "C:\rider\rider-host.exe" }
$log = "C:\rider-test\host-cmd.out"
switch ($Action) {
    "set-pin" { $out = $Value | & $exe set-pin --pin-stdin 2>&1 }
    "revoke-all" {
        $list = & $exe --json clients | ConvertFrom-Json
        $out = foreach ($c in @($list)) { & $exe revoke $c.fingerprint 2>&1 }
    }
    "clients" { $out = & $exe --json clients 2>&1 }
    "tunnels" { $out = & $exe tunnels $Value 2>&1 }
    "info" { $out = (& $exe --json info 2>&1) -join '' }
    "set-server" { $out = & $exe set-server $Value $Extra 2>&1 }
    "restart" {
        Restart-Service RemoteDesktopRider
        $out = (Get-Service RemoteDesktopRider).Status
    }
    "tcp-target" {
        Start-Process powershell -WindowStyle Hidden -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File C:\rider-test\tcp-target.ps1 -Port 18080 -Token $Value"
        $out = "started"
    }
}
"$Action exit=$LASTEXITCODE $($out -join ' ')" | Add-Content $log
