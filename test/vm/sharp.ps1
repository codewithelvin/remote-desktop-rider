param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = "",
    [string]$OldClient = "",
    [switch]$SkipUpdate
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
. "$PSScriptRoot\vmlib.ps1"
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @"
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class PixelDiff {
    static int[] Pixels(Bitmap b) {
        var d = b.LockBits(new Rectangle(0, 0, b.Width, b.Height), ImageLockMode.ReadOnly, PixelFormat.Format32bppRgb);
        var px = new int[b.Width * b.Height];
        for (int y = 0; y < b.Height; y++) Marshal.Copy(d.Scan0 + y * d.Stride, px, y * b.Width, b.Width);
        b.UnlockBits(d);
        return px;
    }
    public static double Same(Bitmap a, Bitmap b, int top) {
        if (a.Width != b.Width || a.Height != b.Height) return -1;
        int[] pa = Pixels(a), pb = Pixels(b);
        long same = 0, all = 0;
        for (int i = top * a.Width; i < pa.Length; i++) { all++; if ((pa[i] & 0xFFFFFF) == (pb[i] & 0xFFFFFF)) same++; }
        return all == 0 ? -1 : (double)same / all;
    }
    [DllImport("user32.dll")] static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    const uint ClientOnlyFullContent = 3;
    public static Bitmap GrabWindow(IntPtr win, int w, int h) {
        var b = new Bitmap(w, h, PixelFormat.Format32bppRgb);
        using (var g = Graphics.FromImage(b)) { var dc = g.GetHdc(); PrintWindow(win, dc, ClientOnlyFullContent); g.ReleaseHdc(dc); }
        return b;
    }
}
"@

function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    throw "timed out waiting for '$pattern' in $file"
}
$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}

function Measure-Viewer([string]$exe) {
    Remove-Item (Join-Path $guest "screen.out"), (Join-Path $guest "support.out"), (Join-Path $guest "pattern.png") -ErrorAction SilentlyContinue
    Set-Content (Join-Path $guest "support.args") "$Server $Fingerprint"
    Guest "start-sharp.ps1" | Out-Null
    $code = ((Wait-File "support.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"') | ConvertFrom-Json).code
    $env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $env1["RIDER_LANG"] = "en"
    $viewer = New-Object Rdr+Proc $exe, "quick $code", $env1
    try {
        Wait-File "support.out" '"support_request"' | Out-Null
        [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "y view`r`n")
        Wait-File "support.out" '"support_started"' | Out-Null
        $end = (Get-Date).AddSeconds(20)
        do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
        if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
        $end = (Get-Date).AddSeconds(30)
        while (-not (Test-Path (Join-Path $guest "pattern.png")) -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 300 }
        $ref = [System.Drawing.Bitmap]::FromFile((Join-Path $guest "pattern.png"))
        $c = [Rdr]::PlaceClient($win, 40, 40, $ref.Width, $ref.Height)
        Start-Sleep 6
        $shot = [PixelDiff]::GrabWindow($win, $c[2], $c[3])
        $same = [PixelDiff]::Same($ref, $shot, 60)
        $shot.Save((Join-Path $env:TEMP "rdr-sharp-$([IO.Path]::GetFileNameWithoutExtension((Split-Path $exe -Parent)))-viewer.png"))
        $ref.Dispose(); $shot.Dispose()
        [pscustomobject]@{ Same = $same; Client = "$($c[2])x$($c[3])"; Title = [Rdr]::Title($win) }
    } finally {
        $viewer.Stop()
        Guest "stop-tests.ps1" | Out-Null
    }
}

if (-not $SkipUpdate) { Guest "update-host.ps1" 300 | Out-Null }
$new = Measure-Viewer $Client
Write-Host ("  this viewer: {0:P2} of the pixels exactly as on the VM (client {1})" -f $new.Same, $new.Client)
Check "this viewer: text is pixel-exact once the screen is still" ($new.Same -ge 0.995)
if ($OldClient -and (Test-Path $OldClient)) {
    $old = Measure-Viewer $OldClient
    Write-Host ("  viewer older than 0.7: {0:P2} exact (video only)" -f $old.Same)
    Check "a viewer older than 0.7, video only, is not exact (the check can tell them apart)" ($old.Same -ge 0 -and $old.Same -lt 0.99)
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
