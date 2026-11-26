$ErrorActionPreference = "Stop"
$dir = "C:\rider-test"; $out = "$dir\tunnel-panel.out"
$trace = "$dir\tunnel-panel.trace"
Remove-Item $out, $trace -ErrorAction SilentlyContinue
function Log($line) {
    Add-Content -Path $out -Value $line -Encoding UTF8
    Add-Content -Path $trace -Value ("{0:HH:mm:ss.fff} {1}" -f (Get-Date), $line) -Encoding UTF8
}
trap { Log "error $_"; Log "done"; exit 1 }
$end = (Get-Date).AddSeconds(180)
while (-not (Test-Path "$dir\tunnel-panel.args") -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 300 }
Start-Sleep -Milliseconds 300
$server, $fp, $code, $pin, $token = (Get-Content "$dir\tunnel-panel.args" -Raw).Trim() -split ' '
Remove-Item "$dir\tunnel-panel.args"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class Win {
    delegate bool P(IntPtr h, IntPtr l);
    [StructLayout(LayoutKind.Sequential)] public struct R { public int L, T, Rt, B; }
    [DllImport("user32.dll")] static extern bool EnumWindows(P f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
    [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    public static IntPtr Of(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var c = new StringBuilder(256); GetClassName(h, c, 256);
            if (p == pid && IsWindowVisible(h) && c.ToString() != "Winit Thread Event Target") { found = h; return false; } return true; }, IntPtr.Zero);
        return found;
    }
    public static void Click(int x, int y) {
        SetCursorPos(x - 4, y); System.Threading.Thread.Sleep(200);
        SetCursorPos(x, y); System.Threading.Thread.Sleep(400);
        mouse_event(2, 0, 0, 0, UIntPtr.Zero); System.Threading.Thread.Sleep(60); mouse_event(4, 0, 0, 0, UIntPtr.Zero);
    }
}
"@
function Shot($name) {
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $img = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen(0, 0, 0, 0, $img.Size)
    $img.Save("$dir\shot-tunnel-$name.png")
}
function Until([scriptblock]$cond, [int]$seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) { $r = & $cond; if ($r) { return $r }; Start-Sleep -Milliseconds 300 }
    return $null
}
function Start-Client($arguments) {
    $si = New-Object System.Diagnostics.ProcessStartInfo $client, $arguments
    $si.UseShellExecute = $false; $si.CreateNoWindow = $true; $si.RedirectStandardInput = $true
    [System.Diagnostics.Process]::Start($si)
}
function Panel-Process {
    Get-CimInstance Win32_Process -Filter "Name='RemoteDesktopRider.exe'" | Where-Object { $_.CommandLine -like '*--session-panel*' } | Select-Object -First 1
}
function Fetch-Later([int]$port) {
    $c = [System.Net.Sockets.TcpClient]::new()
    $c.Connect("127.0.0.1", $port)
    @{ Client = $c; Line = [System.IO.StreamReader]::new($c.GetStream()).ReadLineAsync() }
}
function Fetched($pending) {
    try {
        if (-not $pending.Line.Wait(70000)) { return "timeout" }
        return $pending.Line.Result
    } catch { return "refused" } finally { $pending.Client.Close() }
}
function Pixel([int]$x, [int]$y) {
    $img = New-Object System.Drawing.Bitmap 1, 1
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen($x, $y, 0, 0, $img.Size)
    $img.GetPixel(0, 0)
}
function Banner-Up([IntPtr]$window) {
    $bounds = New-Object Win+R
    [Win]::GetWindowRect($window, [ref]$bounds) | Out-Null
    Until {
        $p = Pixel ($bounds.L + 12) ($bounds.T + 100)
        $dark = [Math]::Abs($p.R - 58) + [Math]::Abs($p.G - 46) + [Math]::Abs($p.B - 20) -lt 12
        $light = [Math]::Abs($p.R - 255) + [Math]::Abs($p.G - 244) + [Math]::Abs($p.B - 214) -lt 12
        $dark -or $light
    } 20
}
function Press([IntPtr]$window, [string]$button, [string]$name) {
    Log ("banner-$name {0}" -f [bool](Banner-Up $window))
    $r = New-Object Win+R
    [Win]::GetWindowRect($window, [ref]$r) | Out-Null
    Log ("rect-$name {0} {1} {2} {3}" -f $r.L, $r.T, $r.Rt, $r.B)
    Shot "$name-asked"
    $offsets = Until { if (Test-Path "$dir\tunnel-click") { (Get-Content "$dir\tunnel-click" -Raw).Trim() } } 300
    $parts = $offsets -split ' '
    $i = if ($button -eq "allow") { 0 } else { 2 }
    Log "click-$name"
    $script:clickedAt = Get-Date
    [Win]::Click($r.Rt - [int]$parts[$i], $r.T + [int]$parts[$i + 1])
    Log "clicked-$name"
    Start-Sleep 1
    Shot "$name-clicked"
    Log "shot-$name"
}

$client = "C:\rider\rider-client.exe"
$env:RIDER_LANG = "en"
Get-Process rider-client, RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
$targets = foreach ($t in @(@(18080, $token), @(18081, "other"))) {
    Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File $dir\tcp-target.ps1 -Port $($t[0]) -Token $($t[1])"
}
& $client set-server $server $fp | Out-Null

$pair = Start-Client "pair $code --name self"
Until { Select-String -Path "$dir\pair.out" -Pattern '"event":"request"' -Quiet -ErrorAction SilentlyContinue } 60 | Out-Null
Start-Sleep 1
$pair.StandardInput.WriteLine("y"); $pair.StandardInput.Flush()
$pair.WaitForExit(60000) | Out-Null
Log "paired $($pair.ExitCode)"
Until { Select-String -Path "$dir\pair.out" -Pattern '^exit=' -Quiet -ErrorAction SilentlyContinue } 30 | Out-Null
Start-Sleep 4

$viewer = Start-Client "connect self --pin-stdin --forward 28080:127.0.0.1:18080 --forward 28081:127.0.0.1:18081"
$viewer.StandardInput.WriteLine($pin); $viewer.StandardInput.Flush()
$h = Until { $w = [Win]::Of($viewer.Id); if ($w -ne [IntPtr]::Zero) { $w } } 60
if (-not $h) { throw "no viewer window" }
[Win]::ShowWindow($h, 6) | Out-Null
$pp = Until { Panel-Process } 20
if (-not $pp) { throw "no session panel" }
$ph = Until { $w = [Win]::Of([int]$pp.ProcessId); if ($w -ne [IntPtr]::Zero) { $w } } 20
Start-Sleep 2

$pending = Fetch-Later 28080
$up = Until { -not [Win]::IsIconic($ph) } 20
Log "panel-up $([bool]$up)"
Press $ph "allow" "allow"
Log ("allow-got " + (Fetched $pending))
Start-Sleep 1
Shot "allow-after"

$pending = Fetch-Later 28081
Press $ph "deny" "deny"

$got = Fetched $pending
Log ([string]::Format([cultureinfo]::InvariantCulture, "deny-got {0}|{1:0.0}", $got, ((Get-Date) - $script:clickedAt).TotalSeconds))
Start-Sleep 1
Shot "deny-after"

$viewer.Kill()
$targets | Stop-Process -Force -ErrorAction SilentlyContinue
Log "done"
