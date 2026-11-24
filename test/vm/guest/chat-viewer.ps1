# Test VM only: the viewer side of test/vm/chat.ps1, in the signed-in
# user's session (start it with start-chat-viewer.ps1). Connects with the
# one-time code in chat-viewer.args, opens the chat, types a message, sends
# a file through the attach button, then reports what it sees until the
# session ends:
#   C:\rider-test\chat-viewer.out   one line per observation:
#     window | typed | attached | title <text> | got <name> <bytes> | gone <name> | exit
# Typing happens here, in the VM: never on the PC running the test.
$ErrorActionPreference = "Stop"
$dir = "C:\rider-test"; $out = "$dir\chat-viewer.out"
Remove-Item $out -ErrorAction SilentlyContinue
function Log($line) { Add-Content -Path $out -Value $line -Encoding UTF8 }
$server, $fp, $code = (Get-Content "$dir\chat-viewer.args" -Raw).Trim() -split ' '
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class Win {
    delegate bool P(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(P f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    public static IntPtr Of(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var c = new StringBuilder(256); GetClassName(h, c, 256);
            if (p == pid && IsWindowVisible(h) && c.ToString() != "Winit Thread Event Target") { found = h; return false; } return true; }, IntPtr.Zero);
        return found;
    }
    public static string Title(IntPtr h) { var b = new StringBuilder(1024); GetWindowText(h, b, 1024); return b.ToString(); }
    public static bool Front(IntPtr h) {
        keybd_event(0x87, 0, 0, UIntPtr.Zero); keybd_event(0x87, 0, 2, UIntPtr.Zero);
        SetForegroundWindow(h); System.Threading.Thread.Sleep(300); return GetForegroundWindow() == h;
    }
}
"@
# A picture of the screen, for looking into failures (C:\rider-test\shot-*.png).
function Shot($name) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $img = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen(0, 0, 0, 0, $img.Size)
    $img.Save("$dir\shot-$name.png")
}
function Keys($k) { [System.Windows.Forms.SendKeys]::SendWait($k); Start-Sleep -Milliseconds 400 }
function Literal($t) { [regex]::Replace($t, '[+^%~(){}\[\]]', '{$0}') }

$client = "C:\rider\rider-client.exe"
& $client set-server $server $fp | Out-Null
$env:RIDER_LANG = "en"
# No console window: it would open in front of the session window and take the keys.
$si = New-Object System.Diagnostics.ProcessStartInfo $client, "quick $code"
$si.UseShellExecute = $false; $si.CreateNoWindow = $true
$p = [System.Diagnostics.Process]::Start($si)
$h = [IntPtr]::Zero
$end = (Get-Date).AddSeconds(90)
while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $end -and -not $p.HasExited) { Start-Sleep 1; $h = [Win]::Of($p.Id) }
if ($h -eq [IntPtr]::Zero) { Log "no-window"; exit 1 }
Log "window"
# With the chat closed, a message from the host shows in the title.
$em = [char]0x2014
$end = (Get-Date).AddSeconds(20)
while ((Get-Date) -lt $end) {
    $t = [Win]::Title($h)
    if ($t -like "*from the host*") { Log ("title " + ($t -split " $em ")[0]); break }
    Start-Sleep -Milliseconds 300
}
Start-Sleep 2

Log ("front " + [Win]::Front($h))
Keys "^%+c"
Start-Sleep 1
Shot "1-chat-open"
Keys "Hello from the viewer{ENTER}"
Log "typed"
Shot "2-typed"

$file = "$dir\from-viewer.txt"
Set-Content -Path $file -Value ("viewer file " + [guid]::NewGuid()) -NoNewline
[Win]::Front($h) | Out-Null
Keys "+{TAB}"   # from the message box to the attach button
Keys " "        # the file dialog
Start-Sleep 2
Shot "3-dialog"
Keys (Literal $file)
Keys "{ENTER}"
Log "attached"
Start-Sleep 2
Shot "4-attached"

# Watch until the session ends: titles (messages show there) and received files.
$sessions = Join-Path $env:TEMP "RemoteDesktopRider\sessions"
$seen = @{}; $lastTitle = ""
while (-not $p.HasExited) {
    $t = [Win]::Title($h)
    # Only titles with the host's message (the rest change every second).
    $msg = ($t -split " $([char]0x2014) ")[0]   # the part before the first em dash
    if ($t -like "*from the host*" -and $msg -ne $lastTitle) { Log "title $msg"; $lastTitle = $msg }
    Get-ChildItem $sessions -Recurse -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne ".lock" } | ForEach-Object {
        if (-not $seen.ContainsKey($_.FullName)) {
            Start-Sleep 1
            $zone = Get-Content -Path $_.FullName -Stream Zone.Identifier -ErrorAction SilentlyContinue
            Log ("got {0} {1} {2}" -f $_.Name, (Get-Item $_.FullName).Length, $(if ("$zone" -like "*ZoneId=3*") { "marked" } else { "unmarked" }))
            $seen[$_.FullName] = (Get-Content $_.FullName -Raw)
            Copy-Item $_.FullName "$dir\received-$($_.Name)" -Force
        }
    }
    Start-Sleep -Milliseconds 500
}
Log "exit"
Start-Sleep 2
foreach ($f in $seen.Keys) { if (-not (Test-Path $f)) { Log "gone $(Split-Path $f -Leaf)" } else { Log "left $(Split-Path $f -Leaf)" } }
