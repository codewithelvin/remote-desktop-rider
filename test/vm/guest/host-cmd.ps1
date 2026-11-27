param([Parameter(Mandatory)][string]$Action, [string]$Value = "")
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
}
"$Action exit=$LASTEXITCODE $($out -join ' ')" | Add-Content $log
