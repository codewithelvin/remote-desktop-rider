param([string]$Work = (Join-Path $env:TEMP "rdr-chat"))
. "$PSScriptRoot\lib.ps1"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Front {
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    public static bool Bring(IntPtr h) {
        keybd_event(0x87, 0, 0, UIntPtr.Zero); keybd_event(0x87, 0, 2, UIntPtr.Zero);
        SetForegroundWindow(h);
        System.Threading.Thread.Sleep(300);
        return GetForegroundWindow() == h;
    }
}
"@

$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function Keys([string]$keys) { [System.Windows.Forms.SendKeys]::SendWait($keys); Start-Sleep -Milliseconds 400 }
function Json([string]$text) { ConvertTo-Json $text -Compress }
function Literal([string]$text) { [regex]::Replace($text, '[+^%~(){}\[\]]', '{$0}') }
$sessions = Join-Path $env:TEMP "RemoteDesktopRider\sessions"

$s = $null
try {
    $s = Start-Session -Work $Work -Answer "y"

    $bs = [char]92
    $s.Host.Send('chat "Salam ' + $bs + 'u0259 from the host"')
    Start-Sleep 2
    $title = [Rdr]::Title($s.Viewer)
    Check "a message from the host shows on the viewer ('$title')" ($title -like "*Salam $([char]0x259) from the host*")

    Check "the viewer window came to the front" ([Front]::Bring($s.Viewer))
    Keys "^%+c"
    Start-Sleep 1
    Keys "Hello from the viewer{ENTER}"
    $got = $null
    try { $got = Wait-For $s.Host '"event":"chat"' 10 } catch {}
    $text = if ($got) { ($got | ConvertFrom-Json).text } else { "" }
    Check "a message typed in the viewer's chat reaches the host ('$text')" ($text -eq "Hello from the viewer")
    Check "typing in the chat did not click or type on the host" ([Rdr]::Clicks -eq 0)

    $src = Join-Path $Work "from-host.txt"
    Set-Content -Path $src -Value ("host file " + [guid]::NewGuid()) -NoNewline
    $s.Host.Send("send-file " + (Json $src))
    $done = $null
    try { $done = Wait-For $s.Host '"event":"file_done"' 15 } catch {}
    Start-Sleep 1
    $arrived = Get-ChildItem $sessions -Recurse -Filter "from-host.txt" -ErrorAction SilentlyContinue | Select-Object -First 1
    Check "a file sent by the host arrives in the viewer's session folder" ($done -and $arrived -and ((Get-Content $arrived.FullName -Raw) -eq (Get-Content $src -Raw)))
    $zone = if ($arrived) { Get-Content -Path $arrived.FullName -Stream Zone.Identifier -ErrorAction SilentlyContinue } else { "" }
    Check "it is marked as from the internet" ("$zone" -like "*ZoneId=3*")

    $up = Join-Path $Work "from-viewer.txt"
    Set-Content -Path $up -Value ("viewer file " + [guid]::NewGuid()) -NoNewline
    [Front]::Bring($s.Viewer) | Out-Null
    Keys "+{TAB}"
    Keys " "
    Start-Sleep 2
    Keys (Literal $up)
    Keys "{ENTER}"
    $in = $null
    try {
        Wait-For $s.Host '"dir":"in"' 15 | Out-Null
        $in = Wait-For $s.Host '"path":' 15
    } catch {}
    $kept = if ($in) { ($in | ConvertFrom-Json).path } else { "" }
    Check "a file attached in the viewer arrives on the host ('$kept')" ($kept -and (Test-Path $kept) -and ((Get-Content $kept -Raw) -eq (Get-Content $up -Raw)))
    Check "the host keeps it in its session folder, not Downloads" ($kept -like "*RemoteDesktopRider\sessions\*")

    $s.Host.Send("access standard")
    Wait-For $s.Host '"access":"standard"' | Out-Null
    Start-Sleep 1
    $s.Host.Send("send-file " + (Json $src))
    $failed = $null
    try { $failed = Wait-For $s.Host '"event":"file_failed"' 10 } catch {}
    Check "Standard access: the host cannot send files" ($null -ne $failed)

    $viewerFile = if ($arrived) { $arrived.FullName } else { "" }
    $s.Host.Send("quit")
    $closed = $s.Client.P.WaitForExit(15000)
    Start-Sleep 2
    Check "the viewer closed when the host ended the session" $closed
    Check "the viewer's received file is gone" (-not $viewerFile -or -not (Test-Path $viewerFile))
    Check "the host's received file is gone" (-not $kept -or -not (Test-Path $kept))
} finally {
    Stop-Session $s
}

$failedCount = @($results | Where-Object Result -eq "FAIL").Count
Write-Host ""
$results | Format-Table -AutoSize | Out-String | Write-Host
if ($failedCount) { Write-Host "FAIL: $failedCount check(s)"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
