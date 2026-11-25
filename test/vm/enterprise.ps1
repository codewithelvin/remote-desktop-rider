# Enterprise mode, connecting (Milestone 7, phase 4), across this PC and the
# Windows test VM. The company's server is a real installation: Ubuntu with
# systemd in Docker on this PC, deploy/install.sh, the production unit
# (test/server/server.sh), made fresh for each run at 192.168.56.1:47900;
# the company is created there the way its IT would. This PC is the admin (a scratch identity, only through `rider-client
# company ...` and `company-connect`: nothing is typed here); the VM's
# service is the company computer and the VM's user answers the request
# (guest/ent-answer.ps1, all typing inside the VM). Checks: sign-in with the
# setup code, joining with the token, Accept opens the session and the
# window goes down, Decline refuses, the admin's switch alone does not skip
# the question, installer flag + switch does. The VM's host config is put
# back and the server container removed afterwards.
#
#   cargo build --release --workspace
#   bash test/server/server.sh build      (the Linux rider-server)
#   cd test/vm && vagrant up
#   powershell -ExecutionPolicy Bypass -File test\vm\enterprise.ps1
#
# Exit code 0: every check passed.
param([string]$Work = (Join-Path $env:TEMP "rdr-vm-enterprise"))
. "$PSScriptRoot\entlib.ps1"

try {
    Guest "update-host.ps1"
    $signin = New-Company
    Check "the admin signs in with the setup code ($($signin.First.Text))" ($signin.First.Code -eq 0)
    Check "the setup code works once" ($signin.Again.Code -ne 0)

    Guest "ent-host.ps1 enroll"
    Check "the VM joins with the token ($(GuestOut 'ent-host.out'))" ((GuestOut "ent-host.out") -like "exit=0*")
    $pc = Wait-Online
    Check "it shows up online in the admin's list, as vagrant's computer ('$($pc.name)', user '$($pc.user)')" ($pc -and $pc.user -eq "vagrant" -and $null -eq $pc.room)
    if (-not $pc) { throw "the VM did not come online on the test server" }

    $r = Try-Connect $pc.fingerprint "accept"
    $ans = GuestOut "ent-answer.out"
    Check "the request comes up in the VM, not minimized ($($ans -join ', '))" ($ans -contains "asked False")
    Check "Accept opens the session" $r.Connected
    Check "after Accept the window goes down to the taskbar" ($ans -contains "after True")

    $r = Try-Connect $pc.fingerprint "decline"
    Check "Decline refuses ('$((($r.Text -split "`n") | Select-Object -Last 1))')" (-not $r.Connected -and $r.Text -match "declined")

    Admin "company unattended $($pc.fingerprint) on" | Out-Null
    $r = Try-Connect $pc.fingerprint "decline"
    Check "the admin's switch alone still asks" ((GuestOut "ent-answer.out") -contains "asked False" -and -not $r.Connected)

    Guest "ent-host.ps1 enroll unattended"
    $end = (Get-Date).AddSeconds(40)
    do { Start-Sleep 2; $u = Computers | Where-Object { $_.fingerprint -eq $pc.fingerprint -and $_.unattended } } while (-not $u -and (Get-Date) -lt $end)
    $r = Try-Connect $pc.fingerprint ""
    Check "installer flag + switch: connected without asking" $r.Connected
} finally {
    Guest "ent-host.ps1 restore"
    Server "down" | Out-Null
}
Finish 10
