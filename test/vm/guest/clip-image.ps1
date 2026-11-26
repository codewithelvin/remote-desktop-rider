param([ValidateSet("set", "get", "clear")][string]$Mode = "get", [int]$Seconds = 20)
$out = "C:\rider-test\clip-image.out"
Remove-Item $out -ErrorAction SilentlyContinue
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
switch ($Mode) {
    "set" {
        $bmp = New-Object System.Drawing.Bitmap 320, 200
        for ($y = 0; $y -lt 200; $y++) {
            for ($x = 0; $x -lt 320; $x++) {
                $bmp.SetPixel($x, $y, [System.Drawing.Color]::FromArgb(255, ($x * 3) % 256, ($y * 5) % 256, (($x + $y) * 7) % 256))
            }
        }
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.FillRectangle([System.Drawing.Brushes]::White, 40, 60, 120, 40)
        $g.DrawString("from the VM", (New-Object System.Drawing.Font("Segoe UI", 11)), [System.Drawing.Brushes]::Black, 44, 68)
        $g.Dispose()
        $bmp.Save("C:\rider-test\clip-sent.png", [System.Drawing.Imaging.ImageFormat]::Png)
        [System.Windows.Forms.Clipboard]::SetImage($bmp)
        Set-Content $out "set 320x200"
    }
    "clear" {
        [System.Windows.Forms.Clipboard]::SetText("rider clipboard test")
        Set-Content $out "cleared"
    }
    "get" {
        $end = (Get-Date).AddSeconds($Seconds)
        while (-not [System.Windows.Forms.Clipboard]::ContainsImage() -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 300 }
        $img = [System.Windows.Forms.Clipboard]::GetImage()
        if ($img) {
            $img.Save("C:\rider-test\clip-got.png", [System.Drawing.Imaging.ImageFormat]::Png)
            Set-Content $out "got $($img.Width)x$($img.Height)"
        } else {
            Set-Content $out "none"
        }
    }
}
