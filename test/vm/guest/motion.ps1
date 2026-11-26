Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System.Runtime.InteropServices;
public static class MotionDpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
"@
[MotionDpi]::SetProcessDPIAware() | Out-Null
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = "None"; $f.TopMost = $true; $f.ShowInTaskbar = $false
$f.StartPosition = "Manual"; $f.Bounds = $b; $f.Text = "rdr-test-motion"
$rng = New-Object System.Random 7
$script:tick = 0
$f.Add_Paint({
    param($s, $e)
    $g = $e.Graphics
    $font = New-Object System.Drawing.Font("Segoe UI", 11)
    for ($y = 0; $y -lt $b.Height; $y += 24) {
        for ($x = 0; $x -lt $b.Width; $x += 160) {
            $c = [System.Drawing.Color]::FromArgb($rng.Next(256), $rng.Next(256), $rng.Next(256))
            $brush = New-Object System.Drawing.SolidBrush $c
            $g.FillRectangle($brush, $x, $y, 160, 24)
            $g.DrawString(("{0:D6}" -f ($script:tick * 7919 + $x + $y)), $font, [System.Drawing.Brushes]::Black, $x + 4, $y + 2)
            $brush.Dispose()
        }
    }
    $font.Dispose()
})
$t = New-Object System.Windows.Forms.Timer
$t.Interval = 40
$t.Add_Tick({ $script:tick++; $f.Invalidate() })
$t.Start()
[System.Windows.Forms.Application]::Run($f)
