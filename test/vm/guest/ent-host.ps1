# Test VM only: the company side of this VM's host for test/vm/enterprise.ps1.
#   ent-host.ps1 enroll [unattended]   join with C:\rider-test\ent-token.txt
#   ent-host.ps1 leave                 rider-host leave-company
#   ent-host.ps1 restore               leave: back to the host config from before
# The host config is saved once before the first change (host.toml.ent-backup).
param([Parameter(Mandatory)][string]$Mode, [string]$Flag = "")
$ErrorActionPreference = "Stop"
$hostExe = "C:\Program Files\RemoteDesktopRider\rider-host.exe"
$cfg = "C:\ProgramData\RemoteDesktopRider\host\host.toml"
$backup = "$cfg.ent-backup"
$out = "C:\rider-test\ent-host.out"
switch ($Mode) {
    "enroll" {
        if (-not (Test-Path $backup)) { Copy-Item $cfg $backup }
        $token = (Get-Content "C:\rider-test\ent-token.txt" -Raw).Trim()
        $args2 = @("enroll", $token)
        if ($Flag -eq "unattended") { $args2 += "--unattended" }
        $r = & $hostExe @args2 2>&1
        Set-Content $out "exit=$LASTEXITCODE $r"
    }
    "leave" {
        $r = & $hostExe leave-company 2>&1
        Set-Content $out "exit=$LASTEXITCODE $r"
    }
    "restore" {
        if (Test-Path $backup) { Copy-Item $backup $cfg -Force; Remove-Item $backup }
        Set-Content $out "restored"
    }
}
Get-Content $out
