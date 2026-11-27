param([string]$State = "enterprise")
$Out = "shot-$State.png"
$Log = "C:\rider-test\app-preview.out"
$ErrorActionPreference = "Stop"
"start $State" | Set-Content $Log
try {
    Add-Type -AssemblyName System.Windows.Forms, System.Drawing
    Get-Process RemoteDesktopRider -ErrorAction SilentlyContinue | Stop-Process -Force
    $env:RIDER_PREVIEW = $State
    $app = Start-Process "C:\rider\RemoteDesktopRider.exe" -PassThru
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
