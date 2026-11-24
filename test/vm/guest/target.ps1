# Test VM only: a topmost "tab strip" window at the top centre of the screen
# (where browser tabs and the session banner are) that logs every left
# click to C:\rider-test\clicks.out, one line each: "x,y". Run it in the
# signed-in user's session (see run-as-user.ps1); it closes after 30 minutes.
$out = "C:\rider-test\clicks.out"
Remove-Item $out -ErrorAction SilentlyContinue
New-Item -ItemType File $out | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System.Runtime.InteropServices;
public static class Dpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
"@
[Dpi]::SetProcessDPIAware() | Out-Null
$sw = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Width
$sh = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds.Height
Set-Content "C:\rider-test\screen.out" "$sw $sh"
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = "None"; $f.TopMost = $true; $f.ShowInTaskbar = $false
$f.StartPosition = "Manual"; $f.Bounds = New-Object System.Drawing.Rectangle ([int]($sw / 2 - 500)), 0, 1000, 200
$f.BackColor = [System.Drawing.Color]::SteelBlue; $f.Text = "rdr-test-tab-strip"
$f.Add_MouseUp({ param($s, $e) if ($e.Button -eq "Left") { Add-Content $out "$($e.X),$($e.Y)" } })
$t = New-Object System.Windows.Forms.Timer; $t.Interval = 30 * 60 * 1000; $t.Add_Tick({ $f.Close() }); $t.Start()
[System.Windows.Forms.Application]::Run($f)
