param([string]$State = "enterprise", [string]$Window = "main")
$Out = "shot-$State.png"
$Log = "C:\rider-test\app-preview.out"
$ErrorActionPreference = "Stop"
"start $State" | Set-Content $Log
try {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Get-Process RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
    $env:RIDER_PREVIEW = $State
    $start = @{ FilePath = "C:\rider\RemoteDesktopRider.exe"; PassThru = $true }
    if ($Window -eq "manage") { $start.ArgumentList = "--manage" }
    $app = Start-Process @start
    Start-Sleep -Seconds 6
    $b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $img = New-Object System.Drawing.Bitmap $b.Width, $b.Height
    [System.Drawing.Graphics]::FromImage($img).CopyFromScreen(0, 0, 0, 0, $img.Size)
    $img.Save("C:\rider-test\$Out")
    Stop-Process -Id $app.Id -Force
    "done" | Add-Content $Log
} catch {
    "error: $_" | Add-Content $Log
}
