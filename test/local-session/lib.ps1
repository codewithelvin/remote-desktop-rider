$ErrorActionPreference = "Stop"
$repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$bin = Join-Path $repo "target\quick"

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

    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, System.Text.StringBuilder s, int n);
    public static IntPtr WindowOf(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
            var c = new System.Text.StringBuilder(256); GetClassName(h, c, 256);
            if (p == pid && IsWindowVisible(h) && c.ToString() != "Winit Thread Event Target") { found = h; return false; }
            return true; }, IntPtr.Zero);
        return found;
    }
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int n);
    public static string Title(IntPtr h) { var b = new System.Text.StringBuilder(1024); GetWindowText(h, b, 1024); return b.ToString(); }
    public static void Place(IntPtr h, int x, int y, int w, int hh) { SetWindowPos(h, new IntPtr(-1), x, y, w, hh, 0x0040); }

    public static void Post(IntPtr h, uint m, int w, IntPtr l) { PostMessage(h, m, new IntPtr(w), l); }

    public static void ClickAt(int x, int y) {
        SetCursorPos(x, y); Thread.Sleep(100);
        mouse_event(0x0002, 0, 0, 0, UIntPtr.Zero); Thread.Sleep(60); mouse_event(0x0004, 0, 0, 0, UIntPtr.Zero);
    }

    public static string ClickHostPixel(IntPtr viewer, int hx, int hy, int hw, int hh) {
        RECT r; GetClientRect(viewer, out r);
        POINT o = new POINT(); ClientToScreen(viewer, ref o);
        double cw = r.R, ch = r.B, s = Math.Min(cw / hw, ch / hh);
        double ox = (cw - hw * s) / 2, oy = (ch - hh * s) / 2;
        int cx = (int)(ox + hx * s), cy = (int)(oy + hy * s);
        IntPtr lp = new IntPtr((cy << 16) | (cx & 0xFFFF));
        PostMessage(viewer, 0x0200, IntPtr.Zero, lp);
        PostMessage(viewer, 0x0201, new IntPtr(1), lp);
        PostMessage(viewer, 0x0202, IntPtr.Zero, lp);
        int x = o.X + cx, y = o.Y + cy;
        return x + "," + y;
    }
}
"@

[Rdr]::DpiAware()
$sw = [Rdr]::GetSystemMetrics(0); $sh = [Rdr]::GetSystemMetrics(1)

function Wait-For($proc, $text, $seconds = 20) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $l = $proc.Find($text); if ($l) { return $l }
        Start-Sleep -Milliseconds 200
    }
    throw "timed out waiting for '$text'. Output so far:`n$($proc.All())"
}

function Start-Session([string]$Work, [string]$HostExe, [string]$ClientExe, [string]$Answer = "y") {
    if (-not $HostExe) { $HostExe = Join-Path $bin "rider-host.exe" }
    if (-not $ClientExe) { $ClientExe = Join-Path $bin "rider-client.exe" }
    if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
    New-Item -ItemType Directory -Force "$Work\srv", "$Work\host-pd", "$Work\host-ad", "$Work\client-ad" | Out-Null
    $port = 47950
    $noEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $hostEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $hostEnv["ProgramData"] = "$Work\host-pd"; $hostEnv["APPDATA"] = "$Work\host-ad"; $hostEnv["RIDER_LANG"] = "en"
    $clientEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $clientEnv["APPDATA"] = "$Work\client-ad"; $clientEnv["RIDER_LANG"] = "en"
    $s = @{ Server = $null; Host = $null; Client = $null; Viewer = [IntPtr]::Zero }
    Write-Host "screen ${sw}x${sh}; host: $HostExe; viewer: $ClientExe"
    $fp = ((& "$bin\rider-server.exe" --state-dir "$Work\srv" info) | Select-String "fingerprint: (\w+)").Matches[0].Groups[1].Value
    $s.Server = New-Object Rdr+Proc "$bin\rider-server.exe", "--state-dir `"$Work\srv`" run --listen 127.0.0.1:$port --public-ip 127.0.0.1 --relay-ports 47951-47960", $noEnv
    Start-Sleep 1
    [Rdr]::Clicks = 0
    [Rdr]::ShowTarget([int]($sw / 2 - 500), 0, 1000, 200)
    Start-Sleep 1
    $s.Host = New-Object Rdr+Proc $HostExe, "--json support 127.0.0.1:$port $fp", $hostEnv
    $reg = Wait-For $s.Host '"registered"'
    $code = ($reg | ConvertFrom-Json).code
    $c = New-Object Rdr+Proc $ClientExe, "set-server 127.0.0.1:$port $fp", $clientEnv
    $c.P.WaitForExit(10000) | Out-Null
    $s.Client = New-Object Rdr+Proc $ClientExe, "quick $code", $clientEnv
    Wait-For $s.Host '"support_request"' | Out-Null
    $s.Host.Send($Answer)
    Wait-For $s.Host '"support_started"' | Out-Null
    $end = (Get-Date).AddSeconds(15)
    do { Start-Sleep -Milliseconds 300; $s.Viewer = [Rdr]::WindowOf($s.Client.P.Id) } while ($s.Viewer -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
    if ($s.Viewer -eq [IntPtr]::Zero) { throw "no viewer window. Client:`n$($s.Client.All())" }
    [Rdr]::Place($s.Viewer, 100, 300, 1280, 760)
    Start-Sleep 6
    return $s
}

function Click-Host($s, [int]$hx, [int]$hy) {
    $at = [Rdr]::ClickHostPixel($s.Viewer, $hx, $hy, $sw, $sh)
    Write-Host "clicked the viewer at $at (host pixel $hx,$hy)"
    Start-Sleep 2
}

function Stop-Session($s) {
    if (-not $s) { return }
    if ($s.Client) { $s.Client.Stop() }
    if ($s.Host) { try { $s.Host.Send("quit") } catch {}; Start-Sleep 1; $s.Host.Stop() }
    if ($s.Server) { $s.Server.Stop() }
    try { [Rdr]::CloseTarget() } catch {}
}
