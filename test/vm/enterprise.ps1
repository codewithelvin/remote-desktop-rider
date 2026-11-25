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
