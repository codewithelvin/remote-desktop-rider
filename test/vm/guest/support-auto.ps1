param([string]$Server, [string]$Fingerprint)
$out = "C:\rider-test\support.out"; $cmd = "C:\rider-test\support.cmd"
Remove-Item $out, $cmd, "C:\rider-test\support.err" -ErrorAction SilentlyContinue
New-Item -ItemType File $cmd | Out-Null
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = "C:\Program Files\RemoteDesktopRider\rider-host.exe"
if (Test-Path C:\rider-test\host-override.exe) { $psi.FileName = "C:\rider-test\host-override.exe" }
$psi.Arguments = "--json support $Server $Fingerprint"
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$psi.EnvironmentVariables["RIDER_LANG"] = "en"
$p = [System.Diagnostics.Process]::Start($psi)
$job = Register-ObjectEvent $p OutputDataReceived -Action { if ($EventArgs.Data) { Add-Content "C:\rider-test\support.out" $EventArgs.Data } }
$p.BeginOutputReadLine()
$errJob = Register-ObjectEvent $p ErrorDataReceived -Action { if ($EventArgs.Data) { Add-Content "C:\rider-test\support.err" $EventArgs.Data } }
$p.BeginErrorReadLine()
$sent = 0
$deadline = (Get-Date).AddMinutes(30)
while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
    $lines = @(Get-Content $cmd -ErrorAction SilentlyContinue)
    while ($sent -lt $lines.Count) {
        $p.StandardInput.WriteLine($lines[$sent]); $p.StandardInput.Flush()
        Add-Content $out "{""sent"":""$($lines[$sent])""}"
        $sent++
    }
    Start-Sleep -Milliseconds 300
}
if (-not $p.HasExited) { $p.StandardInput.WriteLine("quit"); if (-not $p.WaitForExit(5000)) { $p.Kill() } }
Add-Content $out ("{""exit"":" + $p.ExitCode + "}")
