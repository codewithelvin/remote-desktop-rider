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
$si = New-Object System.Diagnostics.ProcessStartInfo $client, "quick $code"
$si.UseShellExecute = $false; $si.CreateNoWindow = $true
$p = [System.Diagnostics.Process]::Start($si)
$h = [IntPtr]::Zero
$end = (Get-Date).AddSeconds(90)
while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $end -and -not $p.HasExited) { Start-Sleep 1; $h = [Win]::Of($p.Id) }
if ($h -eq [IntPtr]::Zero) { Log "no-window"; exit 1 }
Log "window"
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
Keys "+{TAB}"
Keys " "
Start-Sleep 2
Shot "3-dialog"
Keys (Literal $file)
Keys "{ENTER}"
Log "attached"
Start-Sleep 2
Shot "4-attached"

$sessions = Join-Path $env:TEMP "RemoteDesktopRider\sessions"
$seen = @{}; $lastTitle = ""
while (-not $p.HasExited) {
    $t = [Win]::Title($h)
    $msg = ($t -split " $([char]0x2014) ")[0]
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
