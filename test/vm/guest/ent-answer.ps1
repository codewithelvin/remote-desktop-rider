param([Parameter(Mandatory)][ValidateSet("accept", "decline", "close", "watch")][string]$Answer)
$ErrorActionPreference = "Stop"
$dir = "C:\rider-test"; $out = "$dir\ent-answer.out"
Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" |
    Where-Object { $_.CommandLine -like '*ent-answer.ps1*' -and $_.ProcessId -ne $PID } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
Remove-Item $out -ErrorAction SilentlyContinue
function Log($line) { Add-Content -Path $out -Value $line -Encoding UTF8 }
trap { Log "error $_"; exit 1 }
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @"
using System; using System.Runtime.InteropServices; using System.Text;
public static class Win {
    delegate bool P(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(P f, IntPtr l);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassName(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern void keybd_event(byte vk, byte scan, uint flags, UIntPtr extra);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
    public static string Title(IntPtr h) { var b = new StringBuilder(256); GetWindowText(h, b, 256); return b.ToString(); }
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern bool IsWindow(IntPtr h);
    public static IntPtr Of(int pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p); var c = new StringBuilder(256); GetClassName(h, c, 256);
            if (p == pid && IsWindowVisible(h) && c.ToString() != "Winit Thread Event Target") { found = h; return false; } return true; }, IntPtr.Zero);
        return found;
    }
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
    $img.Save("$dir\shot-ent-$name.png")
}
$h = [IntPtr]::Zero
$end = (Get-Date).AddSeconds(60)
while ($h -eq [IntPtr]::Zero -and (Get-Date) -lt $end) {
    foreach ($p in Get-Process RemoteDesktopRider -ErrorAction SilentlyContinue) {
        $w = [Win]::Of($p.Id)
        if ($w -ne [IntPtr]::Zero -and [Win]::Title($w) -like 'Chat with*') { $h = $w; break }
    }
    Start-Sleep -Milliseconds 300
}
if ($h -eq [IntPtr]::Zero) { Log "none"; exit 0 }
Start-Sleep 2
Log "asked $([Win]::IsIconic($h))"
Shot "$Answer-asked"
if ($Answer -eq "watch") {
    $t0 = Get-Date
    while ([Win]::IsWindow($h) -and ((Get-Date) - $t0).TotalSeconds -lt 90) { Start-Sleep -Milliseconds 300 }
    Log "gone $([math]::Round(((Get-Date) - $t0).TotalSeconds))"
    exit 0
}
if ($Answer -eq "close") {
    [Win]::PostMessage($h, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero) | Out-Null
    Log "answered close"
    exit 0
}
[Win]::Front($h) | Out-Null
$keys = if ($Answer -eq "accept") { "{TAB}{ENTER}" } else { "{TAB}{TAB}{ENTER}" }
[System.Windows.Forms.SendKeys]::SendWait($keys)
Log "answered $Answer"
Start-Sleep 3
Log "after $([Win]::IsIconic($h))"
Shot "$Answer-after"
