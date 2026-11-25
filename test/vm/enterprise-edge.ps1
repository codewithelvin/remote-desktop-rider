param([string]$Work = (Join-Path $env:TEMP "rdr-vm-enterprise-edge"))
. "$PSScriptRoot\entlib.ps1"
function Last([string]$text) { (($text -split "`n") | Where-Object { $_ } | Select-Object -Last 1) }

try {
    Guest "update-host.ps1"
    New-Company | Out-Null
    Guest "ent-host.ps1 enroll unattended"
    $pc = Wait-Online
    if (-not $pc) { throw "the VM did not come online on the test server" }
    $fp = $pc.fingerprint

    $r = Try-Connect $fp "" -Env $envStranger
    Check "a device that is not the admin cannot connect ('$(Last $r.Text)')" (-not $r.Connected)
    $l = Stranger "company list"
    Check "nor see the company's list ('$(Last $l.Text)')" ($l.Code -ne 0 -and $l.Text -notmatch 'RIDER-VM')

    $r = Try-Connect $fp "close"
    Check "closing the request window declines ('$(Last $r.Text)')" (-not $r.Connected -and $r.Text -match "declined")

    $r = Try-Connect $fp "watch"
    $gone = "$(Wait-GuestLine "ent-answer.out" '^gone ' 15)" -replace 'gone ', ''
    Check "no answer: declined after about 60 s ($($r.Seconds) s, '$(Last $r.Text)')" (-not $r.Connected -and $r.Text -match "nobody answered" -and $r.Seconds -ge 55 -and $r.Seconds -le 75)
    Check "and the request window goes away (after $gone s)" ($gone -and [int]$gone -le 70)

    $r = Try-Connect $fp "" -GiveUpAfter 6
    Guest "ent-log.ps1"
    Guest "ent-panel.ps1"
    $log = @(GuestOut "ent-log.out")
    $i = @(0..($log.Count - 1) | Where-Object { $log[$_] -match 'gave up before an answer' })[-1]
    $asked = if ($null -ne $i) { @($log[0..$i] -match 'connection request')[-1] }
    $time = { param($l) if ($l -match '^(\S+Z)') { [datetime]::Parse($Matches[1]) } }
    $late = if ($asked) { [math]::Round(((& $time $log[$i]) - (& $time $asked)).TotalSeconds) } else { 999 }
    $open = (GuestOut "ent-panel.out") -join ""
    Check "the admin gives up after 6 s: the computer hears it ($late s after the request) and the window is gone ($open open)" ($late -le 12 -and $open -eq "0")

    Remove-Item (Join-Path $guest "ent-answer.out") -ErrorAction SilentlyContinue
    Guest "run-as-user.ps1 ent-answer.ps1 accept"
    $first = New-Object Rdr+Proc $client, "company-connect $fp", $env1
    $end = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $end -and -not $first.Find("Connected to") -and -not $first.P.HasExited) { Start-Sleep -Milliseconds 300 }
    $second = Admin "company-connect $fp" 20
    Start-Sleep 2
    Check "a second request during a session is refused ('$(Last $second.Text)')" ($first.Find("Connected to") -and $second.Text -notmatch "Connected to" -and $second.Text -match "busy")
    Check "and the first session goes on" (-not $first.P.HasExited)
    $w = [Rdr]::WindowOf($first.P.Id)
    if ($w -ne [IntPtr]::Zero) { [Rdr]::Post($w, 0x0010, 0, [IntPtr]::Zero) }
    $first.P.WaitForExit(10000) | Out-Null
    $first.Stop()
    Start-Sleep 3

    Guest "run-as-user.ps1 lock.ps1"
    Guest "wait-lock.ps1 on"
    $lock = (GuestOut "wait-lock.out") -join ""
    if ($lock -notlike "on *") {
        Guest "run-as-user.ps1 lock.ps1"
        Guest "wait-lock.ps1 on"
        $lock = "again: " + ((GuestOut "wait-lock.out") -join "")
    }
    Check "the VM's screen is locked ($lock)" ($lock -match "on \d")
    $r = Try-Connect $fp ""
    Check "a locked computer is refused at once ($($r.Seconds) s, '$(Last $r.Text)')" (-not $r.Connected -and $r.Text -match "locked" -and $r.Seconds -le 15)
    Admin "company unattended $fp on" | Out-Null
    Start-Sleep 3
    $r = Try-Connect $fp ""
    Check "but an unattended one connects while locked" $r.Connected
    Guest "unlock.ps1" 180
    Guest "wait-lock.ps1 off"
    $lock = (GuestOut "wait-lock.out") -join ""
    Check "the VM's screen is unlocked again ($lock)" ($lock -like "off *")

    Admin "company unattended $fp off" | Out-Null
    Start-Sleep 3
    $r = Try-Connect $fp "decline"
    Check "unattended switched off: the employee is asked again" ((GuestOut "ent-answer.out") -contains "asked False" -and -not $r.Connected)

    Guest "relogon.ps1" 180
    $re = (GuestOut "relogon.out") -join ""
    $back = Wait-Online 90
    Check "signed out and in again ($re): the computer is back online" ([bool]$back)
    $r = Try-Connect $fp "decline"
    Check "and its employee is asked as before" ((GuestOut "ent-answer.out") -contains "answered decline" -and -not $r.Connected)

    $paired = & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $PSScriptRoot "paired-session.ps1") -Server $serverAddr -Fingerprint $serverFp 2>&1
    Write-Host (($paired | Select-String "PASS|FAIL") -join "`n")
    Check "a paired device on a company computer connects with its PIN (paired-session.ps1)" ($LASTEXITCODE -eq 0)

    Admin "company unattended $fp on" | Out-Null
    Server "sh systemctl restart rider-server" | Out-Null
    Start-Sleep 5
    $back = Wait-Online 90
    Check "after a server restart the computer is back online ('$($back.name)')" ($back -and $back.fingerprint -eq $fp)
    Check "and the company is kept: the admin's list, the unattended switch" ($back -and $back.unattended)
    $r = Try-Connect $fp ""
    Check "and connecting works again (unattended)" $r.Connected

    Guest "ent-host.ps1 leave"
    Check "the computer leaves the company ($(GuestOut 'ent-host.out'))" ((GuestOut "ent-host.out") -like "exit=0*")
    Start-Sleep 5
    $r = Try-Connect $fp ""
    Check "and the admin can no longer reach it ('$(Last $r.Text)')" (-not $r.Connected)
} finally {
    Guest "unlock.ps1" 180
    Guest "ent-host.ps1 restore"
    Server "down" | Out-Null
}
Finish 21
