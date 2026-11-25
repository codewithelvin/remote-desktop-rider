# Test VM only: unlock vagrant's console session without a password (as
# SYSTEM, the agent). tscon reconnects the session to the console, which
# unlocked it on older Windows; Windows 11 keeps the lock screen up. Then the
# session is signed out instead: the VM signs vagrant in again by itself
# (automatic sign-in), unlocked, and the host's agent follows the new session.
# Waits (up to 2 minutes) for the new desktop. Prints what it did.
. C:\rider-test\wait-lock.ps1 -Define
$id = (Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object UserName -like "*vagrant" | Select-Object -First 1).SessionId
if (-not $id) { "no session"; exit 0 }
if (-not [Wts.Api]::Locked()) { "not locked"; exit 0 }
& tscon.exe $id /dest:console 2>&1 | Out-Null
$end = (Get-Date).AddSeconds(5)
while ([Wts.Api]::Locked() -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 250 }
if (-not [Wts.Api]::Locked()) { "unlocked $id (tscon)"; exit 0 }
# Automatic sign-in after a sign-out too, not only at boot (test VM only:
# vagrant/vagrant is the box's well-known account).
$wl = "HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon"
Set-ItemProperty $wl AutoAdminLogon "1"
Set-ItemProperty $wl DefaultUserName "vagrant"
Set-ItemProperty $wl DefaultPassword "vagrant"
Set-ItemProperty $wl ForceAutoLogon "1"
& logoff.exe $id
$end = (Get-Date).AddSeconds(120)
do {
    Start-Sleep 1
    $new = Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue |
        Where-Object { $_.UserName -like "*vagrant" -and $_.SessionId -ne $id } | Select-Object -First 1
} while (-not ($new -and -not [Wts.Api]::Locked()) -and (Get-Date) -lt $end)
if ($new) { "unlocked: signed in again (session $($new.SessionId))" } else { "still locked" }
