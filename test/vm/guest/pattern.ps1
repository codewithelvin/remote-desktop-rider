$out = "C:\rider-test\pattern.png"
Remove-Item $out -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System.Runtime.InteropServices;
public static class PatternDpi { [DllImport("user32.dll")] public static extern bool SetProcessDPIAware(); }
"@
[PatternDpi]::SetProcessDPIAware() | Out-Null
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = "None"; $f.TopMost = $true; $f.ShowInTaskbar = $false
$f.StartPosition = "Manual"; $f.Bounds = $b; $f.Text = "rdr-test-pattern"
$f.BackColor = [System.Drawing.Color]::FromArgb(24, 36, 90)
$colors = @("Red", "Lime", "Yellow", "Cyan", "Magenta", "White", "Orange", "DeepPink")
$f.Add_Paint({
    param($s, $e)
    $g = $e.Graphics
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::ClearTypeGridFit
    $font = New-Object System.Drawing.Font("Segoe UI", 9)
    $line = "The quick brown fox jumps over the lazy dog 0123456789 Salam dunya: sharp text test"
    for ($y = 60; $y -lt $b.Height - 20; $y += 15) {
        $c = [System.Drawing.Color]::FromName($colors[($y / 15) % $colors.Count])
        $brush = New-Object System.Drawing.SolidBrush $c
        $g.DrawString($line, $font, $brush, 10, $y)
        $g.DrawString($line, $font, $brush, [int]($b.Width / 2) + 10, $y)
        $brush.Dispose()
    }
    $font.Dispose()
})
$shot = New-Object System.Windows.Forms.Timer
$shot.Interval = 4000
$shot.Add_Tick({
    $shot.Stop()
    $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CopyFromScreen($b.X, $b.Y, 0, 0, $bmp.Size)
    $g.Dispose()
    $bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Dispose()
})
$shot.Start()
$t = New-Object System.Windows.Forms.Timer; $t.Interval = 30 * 60 * 1000; $t.Add_Tick({ $f.Close() }); $t.Start()
[System.Windows.Forms.Application]::Run($f)
