. C:\rider-test\wait-lock.ps1 -Define
$id = (Get-Process explorer -IncludeUserName -ErrorAction SilentlyContinue | Where-Object UserName -like "*vagrant" | Select-Object -First 1).SessionId
if (-not $id) { "no session"; exit 0 }
if (-not [Wts.Api]::Locked()) { "not locked"; exit 0 }
& tscon.exe $id /dest:console 2>&1 | Out-Null
$end = (Get-Date).AddSeconds(5)
while ([Wts.Api]::Locked() -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 250 }
if (-not [Wts.Api]::Locked()) { "unlocked $id (tscon)"; exit 0 }
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
