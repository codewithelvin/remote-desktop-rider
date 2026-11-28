$ErrorActionPreference = "Stop"
$dir = "C:\rider-test"; $out = "$dir\restart-panel.out"
Remove-Item $out -ErrorAction SilentlyContinue
function Log($line) { Add-Content -Path $out -Value $line -Encoding UTF8 }
trap { Log "error $_"; exit 1 }
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
function Until([scriptblock]$cond, [int]$seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) { $r = & $cond; if ($r) { return $r }; Start-Sleep -Milliseconds 300 }
    return $null
}
function Grab([Win+R]$r) {
    $img = New-Object System.Drawing.Bitmap ($r.Rt - $r.L), ($r.B - $r.T)
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen($r.L, $r.T, 0, 0, $img.Size)
    $img
}
function Is-Banner($p) {
    $dark = [Math]::Abs($p.R - 64) + [Math]::Abs($p.G - 30) + [Math]::Abs($p.B - 28) -lt 12
    $light = [Math]::Abs($p.R - 255) + [Math]::Abs($p.G - 228) + [Math]::Abs($p.B - 224) -lt 12
    $dark -or $light
}
function Is-Red($p) { $p.R -gt 200 -and $p.R - $p.G -gt 90 -and $p.R - $p.B -gt 90 }
function Cancel-Button([Win+R]$r) {
    $img = Grab $r
    $rows = @(0..([Math]::Min($img.Height, 300) - 1) | Where-Object { $_ % 2 -eq 0 -and (Is-Banner $img.GetPixel(12, $_)) })
    if ($rows.Count -lt 4) { return $null }
    $lower = @($rows | Where-Object { $_ -gt ($rows[0] + $rows[-1]) / 2 })
    for ($x = $img.Width - 4; $x -gt $img.Width / 2; $x -= 3) {
        foreach ($y in $lower) {
            if (Is-Red $img.GetPixel($x, $y)) { return @(($r.L + $x - 15), ($r.T + $y)) }
        }
    }
    return $null
}

$pp = Until {
    Get-CimInstance Win32_Process -Filter "Name='RemoteDesktopRider.exe'" |
        Where-Object { $_.CommandLine -like '*--session-panel*' } | Select-Object -First 1
} 60
if (-not $pp) { throw "no session panel" }
$panel = Until { $w = [Win]::Of([int]$pp.ProcessId); if ($w -ne [IntPtr]::Zero) { $w } } 30
Log "panel"
$panelRect = New-Object Win+R
$at = Until { if (-not [Win]::IsIconic($panel)) { [Win]::GetWindowRect($panel, [ref]$panelRect) | Out-Null; Cancel-Button $panelRect } } 16
if (-not $at) { [Win]::GetWindowRect($panel, [ref]$panelRect) | Out-Null; (Grab $panelRect).Save("$direstart-panel.png"); $all = New-Object Win+R; $all.Rt = 1280; $all.B = 720; (Grab $all).Save("$direstart-screen.png"); throw "no restart banner with a Cancel button (iconic: $([Win]::IsIconic($panel)))" }
Log "banner $($at[0]) $($at[1])"
[Win]::Click($at[0], $at[1])
Log "cancel-clicked"
