$ErrorActionPreference = "Stop"
$dir = "C:\rider-test"; $out = "$dir\paired-chat.out"
Remove-Item $out -ErrorAction SilentlyContinue
function Log($line) { Add-Content -Path $out -Value $line -Encoding UTF8 }
trap { Log "error $_"; Log "done"; exit 1 }
$end = (Get-Date).AddSeconds(180)
while (-not (Test-Path "$dir\paired-chat.args") -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 300 }
Start-Sleep -Milliseconds 300
$server, $fp, $code, $pin = (Get-Content "$dir\paired-chat.args" -Raw).Trim() -split ' '
Remove-Item "$dir\paired-chat.args"
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class Win {
    delegate bool P(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(P f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
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
    $img.Save("$dir\shot-paired-$name.png")
}
function Keys($k) { [System.Windows.Forms.SendKeys]::SendWait($k); Start-Sleep -Milliseconds 400 }
function Literal($t) { [regex]::Replace($t, '[+^%~(){}\[\]]', '{$0}') }
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
function Attach($window, $file) {
    [Win]::Front($window) | Out-Null
    Keys "+{TAB}"
    Keys " "
    Until { [Win]::Title([Win]::GetForegroundWindow()) -match '^(Open|Choose|Select)' } 15 | Out-Null
    Start-Sleep 1
    Keys (Literal ($file -replace '^[A-Za-z]:', ''))
    Keys "{ENTER}"
    Start-Sleep 1
}

$client = "C:\rider\rider-client.exe"
$env:RIDER_LANG = "en"
Get-Process rider-client, RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
& $client set-server $server $fp | Out-Null

$pair = Start-Client "pair $code --name self"
Until { Select-String -Path "$dir\pair.out" -Pattern '"event":"request"' -Quiet -ErrorAction SilentlyContinue } 60 | Out-Null
Start-Sleep 1
$pair.StandardInput.WriteLine("y"); $pair.StandardInput.Flush()
$pair.WaitForExit(60000) | Out-Null
Log "paired $($pair.ExitCode)"
Until { Select-String -Path "$dir\pair.out" -Pattern '^exit=' -Quiet -ErrorAction SilentlyContinue } 30 | Out-Null
Start-Sleep 4

$viewer = Start-Client "connect self --pin-stdin"
$viewer.StandardInput.WriteLine($pin); $viewer.StandardInput.Flush()
$h = Until { $w = [Win]::Of($viewer.Id); if ($w -ne [IntPtr]::Zero) { $w } } 60
if (-not $h) { throw "no viewer window" }
Log "window"
[Win]::ShowWindow($h, 3) | Out-Null
Start-Sleep 3
$pp = Until { Panel-Process } 20
if (-not $pp) { throw "no chat window process" }
$ph = Until { $w = [Win]::Of([int]$pp.ProcessId); if ($w -ne [IntPtr]::Zero) { $w } } 20
Start-Sleep 2
Log ("panel {0} {1}" -f (Invoke-CimMethod -InputObject $pp -MethodName GetOwner).User, [Win]::IsIconic($ph))
Shot "1-connected"

[Win]::ShowWindow($ph, 9) | Out-Null
[Win]::Front($ph) | Out-Null
Keys "Salam from the host panel{ENTER}"
$em = [char]0x2014
$t = Until { $x = [Win]::Title($h); if ($x -like "*from the host panel*") { $x } } 20
Log ("title " + ($t -split " $em ")[0])
Shot "2-host-wrote"
[Win]::ShowWindow($ph, 6) | Out-Null

[Win]::Front($h) | Out-Null
Keys "^%+c"
Start-Sleep 1
Keys "Hello from the viewer{ENTER}"
$up = Until { -not [Win]::IsIconic($ph) } 10
Start-Sleep 1
Log ("panel-up {0} {1}" -f [Win]::IsIconic($ph), ([Win]::GetForegroundWindow() -eq $h))
Shot "3-viewer-wrote"

$downloads = Join-Path $env:USERPROFILE "Downloads\Rider"
$stamp = [guid]::NewGuid().ToString("N").Substring(0, 8)
$vfile = "$dir\pv-$stamp.txt"
Set-Content -Path $vfile -Value "viewer file $stamp" -NoNewline
Attach $h $vfile
$saved = Until { Get-Item (Join-Path $downloads "pv-$stamp.txt") -ErrorAction SilentlyContinue } 30
if ($saved) { Log "saved $($saved.Name) $($saved.Length) $((Get-Acl $saved.FullName).Owner)" }
Shot "4-viewer-file"

$hfile = "$dir\ph-$stamp.txt"
Set-Content -Path $hfile -Value "host file $stamp" -NoNewline
[Win]::ShowWindow($ph, 9) | Out-Null
Attach $ph $hfile
$sessions = Join-Path $env:TEMP "RemoteDesktopRider\sessions"
$got = Until { Get-ChildItem $sessions -Recurse -File -Filter "ph-$stamp.txt" -ErrorAction SilentlyContinue | Select-Object -First 1 } 30
if ($got) { Log "got $($got.Name) $($got.Length)" }
Shot "5-host-file"

$viewer.Kill()
$t0 = Get-Date
$gone = Until { -not (Panel-Process) } 60
Log ("panel-gone {0}" -f $(if ($gone) { [int]((Get-Date) - $t0).TotalSeconds } else { "never" }))
Remove-Item $vfile, $hfile -ErrorAction SilentlyContinue
Log "done"
