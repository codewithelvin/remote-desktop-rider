# End-to-end check on this PC, no admin rights, nothing installed or changed:
# a remote click where the host's session banner is (top centre of the
# screen; the viewer cannot see it, it is excluded from capture) must reach
# the window underneath - a browser's tab strip, say - and must not end the
# session.
#
# Runs a scratch rider-server on loopback, `rider-host support` and
# `rider-client quick` with their settings in a temporary folder, a test
# window at the top centre (the "tab strip"), then clicks in the viewer
# window where the banner is. Moves the pointer for a moment.
#
#   cargo build --profile quick -p rider-host -p rider-client -p rider-server
#   powershell -ExecutionPolicy Bypass -File test\local-session\banner-click.ps1
#   ... -HostExe "C:\Program Files\RemoteDesktopRider\rider-host.exe"   # an installed host
#
# Exit code 0: the click went through and the session stayed up.
param(
    [string]$HostExe = "",
    [string]$Work = (Join-Path $env:TEMP "rdr-banner-click"),
    # Host pixel to click (default: on the banner). For checking the harness.
    [int]$X = -1, [int]$Y = -1,
    # Only check that the test window counts a direct click.
    [switch]$SelfTest
)
$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$bin = Join-Path $repo "target\quick"
if (-not $HostExe) { $HostExe = Join-Path $bin "rider-host.exe" }

Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Drawing;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows.Forms;

public static class Rdr {
    [DllImport("user32.dll")] static extern IntPtr SetProcessDpiAwarenessContext(IntPtr v);
    [DllImport("user32.dll")] public static extern uint GetDpiForSystem();
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
    [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint f);
    [DllImport("user32.dll")] static extern bool GetClientRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] static extern bool ClientToScreen(IntPtr h, ref POINT p);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x, int y);
    [DllImport("user32.dll")] static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    delegate bool EnumProc(IntPtr h, IntPtr l);
    public struct RECT { public int L, T, R, B; }
    public struct POINT { public int X, Y; }

    public static void DpiAware() { SetProcessDpiAwarenessContext(new IntPtr(-4)); }

    // The "tab strip": a topmost window that counts left clicks.
    public static int Clicks;
    public static Point LastClick;
    static Form form;
    public static void ShowTarget(int x, int y, int w, int h) {
        var t = new Thread(() => {
            form = new Form { FormBorderStyle = FormBorderStyle.None, TopMost = true, ShowInTaskbar = false,
                StartPosition = FormStartPosition.Manual, Bounds = new Rectangle(x, y, w, h),
                BackColor = Color.SteelBlue, Text = "rdr-test-tab-strip" };
            form.MouseUp += (s, e) => { if (e.Button == MouseButtons.Left) { LastClick = e.Location; Interlocked.Increment(ref Clicks); } };
            Application.Run(form);
        });
        t.SetApartmentState(ApartmentState.STA);
        t.IsBackground = true;
        t.Start();
    }
    public static void CloseTarget() { if (form != null) form.Invoke((Action)(() => form.Close())); }

    // A child process with its output collected line by line.
    public class Proc {
        public Process P;
        public List<string> Lines = new List<string>();
        public Proc(string file, string args, Dictionary<string, string> env) {
            var si = new ProcessStartInfo(file, args) { UseShellExecute = false, RedirectStandardInput = true,
                RedirectStandardOutput = true, RedirectStandardError = true, CreateNoWindow = true };
            foreach (var kv in env) si.EnvironmentVariables[kv.Key] = kv.Value;
            P = new Process { StartInfo = si };
            P.OutputDataReceived += (s, e) => { if (e.Data != null) lock (Lines) Lines.Add(e.Data); };
            P.ErrorDataReceived += (s, e) => { if (e.Data != null) lock (Lines) Lines.Add("err: " + e.Data); };
            P.Start();
            P.BeginOutputReadLine();
            P.BeginErrorReadLine();
        }
        public string Find(string text) { lock (Lines) foreach (var l in Lines) if (l.Contains(text)) return l; return null; }
        public string All() { lock (Lines) return string.Join("\n", Lines); }
        public void Send(string line) { P.StandardInput.WriteLine(line); P.StandardInput.Flush(); }
        public void Stop() { try { if (!P.HasExited) P.Kill(); } catch { } }
    }

    public static IntPtr WindowOf(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
            if (p == pid && IsWindowVisible(h)) { found = h; return false; } return true; }, IntPtr.Zero);
        return found;
    }
    public static void Place(IntPtr h, int x, int y, int w, int hh) { SetWindowPos(h, new IntPtr(-1), x, y, w, hh, 0x0040); } // topmost: nothing covers it

    public static void ClickAt(int x, int y) {
        SetCursorPos(x, y); Thread.Sleep(100);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); Thread.Sleep(60); mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }

    // Click in the viewer at the point that shows host pixel (hx, hy) of a
    // hw x hh screen (the picture is letterboxed to keep its aspect).
    public static string ClickHostPixel(IntPtr viewer, int hx, int hy, int hw, int hh) {
        RECT r; GetClientRect(viewer, out r);
        POINT o = new POINT(); ClientToScreen(viewer, ref o);
        double cw = r.R, ch = r.B, s = Math.Min(cw / hw, ch / hh);
        double ox = (cw - hw * s) / 2, oy = (ch - hh * s) / 2;
        int cx = (int)(ox + hx * s), cy = (int)(oy + hy * s);
        // Posted to the viewer, not real input: viewer and host share this
        // PC's one pointer, which the host moves when it replays the click.
        // Posted messages come before real input, so the viewer handles the
        // whole click before it sees the host's pointer move.
        IntPtr lp = new IntPtr((cy << 16) | (cx & 0xFFFF));
        PostMessage(viewer, 0x0200, IntPtr.Zero, lp); // WM_MOUSEMOVE
        Thread.Sleep(100);
        PostMessage(viewer, 0x0201, new IntPtr(1), lp); // WM_LBUTTONDOWN
        PostMessage(viewer, 0x0202, IntPtr.Zero, lp); // WM_LBUTTONUP
        int x = o.X + cx, y = o.Y + cy;
        return x + "," + y;
    }
}
"@

[Rdr]::DpiAware()
$sw = [Rdr]::GetSystemMetrics(0); $sh = [Rdr]::GetSystemMetrics(1)
Write-Host "screen ${sw}x${sh} at $([Rdr]::GetDpiForSystem()) dpi; host: $HostExe"

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force "$Work\srv", "$Work\host-pd", "$Work\host-ad", "$Work\client-ad" | Out-Null
$port = 47950
$serverEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$hostEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$hostEnv["ProgramData"] = "$Work\host-pd"; $hostEnv["APPDATA"] = "$Work\host-ad"; $hostEnv["RIDER_LANG"] = "en"
$clientEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$clientEnv["APPDATA"] = "$Work\client-ad"; $clientEnv["RIDER_LANG"] = "en"

function Wait-For($proc, $text, $seconds = 20) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $l = $proc.Find($text); if ($l) { return $l }
        Start-Sleep -Milliseconds 200
    }
    throw "timed out waiting for '$text'. Output so far:`n$($proc.All())"
}

$server = $null; $hostP = $null; $client = $null
$failed = $true
try {
    $fp = ((& "$bin\rider-server.exe" --state-dir "$Work\srv" info) | Select-String "fingerprint: (\w+)").Matches[0].Groups[1].Value
    $server = New-Object Rdr+Proc "$bin\rider-server.exe", "--state-dir `"$Work\srv`" run --listen 127.0.0.1:$port --public-ip 127.0.0.1 --relay-ports 47951-47960", $serverEnv
    Start-Sleep 1

    # The "tab strip" under where the banner will be (created first: the banner goes above it).
    [Rdr]::ShowTarget([int]($sw / 2 - 500), 0, 1000, 200)
    if ($SelfTest) {
        Start-Sleep 1; [Rdr]::ClickAt([int]($sw / 2 - 400), 150); Start-Sleep 1
        Write-Host "self test: tab strip clicks $([Rdr]::Clicks)"; $failed = [Rdr]::Clicks -lt 1; return
    }
    Start-Sleep 1

    $hostP = New-Object Rdr+Proc $HostExe, "--json support 127.0.0.1:$port $fp", $hostEnv
    $reg = Wait-For $hostP '"registered"'
    $code = ($reg | ConvertFrom-Json).code
    Write-Host "support code $code"

    $c = New-Object Rdr+Proc "$bin\rider-client.exe", "set-server 127.0.0.1:$port $fp", $clientEnv
    $c.P.WaitForExit(10000) | Out-Null
    $client = New-Object Rdr+Proc "$bin\rider-client.exe", "quick $code", $clientEnv
    Wait-For $hostP '"support_request"' | Out-Null
    $hostP.Send("y")
    Wait-For $hostP '"support_started"' | Out-Null

    $end = (Get-Date).AddSeconds(15)
    do { Start-Sleep -Milliseconds 300; $viewer = [Rdr]::WindowOf($client.P.Id) } while ($viewer -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
    if ($viewer -eq [IntPtr]::Zero) { throw "no viewer window. Client:`n$($client.All())" }
    # Below the tab strip, so the click the host injects lands on the strip, not on the viewer.
    [Rdr]::Place($viewer, 100, 300, 1280, 760)
    Start-Sleep 6   # the viewer's own toolbar intro is over

    # On the banner (it is at least 520 px wide at the top centre), off the
    # middle so it misses the viewer's toolbar tab.
    $hx = [int]($sw / 2 + 150); $hy = 17
    if ($X -ge 0) { $hx = $X; $hy = $Y }
    $at = [Rdr]::ClickHostPixel($viewer, $hx, $hy, $sw, $sh)
    Write-Host "clicked the viewer at $at (host pixel $hx,$hy)"
    Start-Sleep 3

    $clicks = [Rdr]::Clicks
    $ended = $hostP.Find('"support_ended"')
    $alive = -not $client.P.HasExited
    Write-Host "tab strip clicks: $clicks (last at $([Rdr]::LastClick)); viewer running: $alive; host ended the session: $([bool]$ended)"
    if ($client.P.HasExited -or $env:RDR_VERBOSE) { Write-Host "client said:`n$($client.All())`nhost said:`n$($hostP.All())" }
    $failed = -not ($clicks -ge 1 -and $alive -and -not $ended)
} finally {
    if ($client) { $client.Stop() }
    if ($hostP) { try { $hostP.Send("quit") } catch {}; Start-Sleep 1; $hostP.Stop() }
    if ($server) { $server.Stop() }
    try { [Rdr]::CloseTarget() } catch {}
}
if ($failed) { Write-Host "FAIL: the remote click did not go through, or it ended the session"; exit 1 }
Write-Host "PASS: the remote click reached the window under the banner and the session stayed up"
exit 0
