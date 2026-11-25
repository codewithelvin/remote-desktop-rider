$out = "C:\rider-test\pair.out"
Remove-Item $out -ErrorAction SilentlyContinue
$psi = New-Object System.Diagnostics.ProcessStartInfo
$psi.FileName = "C:\Program Files\RemoteDesktopRider\rider-host.exe"
$psi.Arguments = "--json pair"
$psi.UseShellExecute = $false
$psi.RedirectStandardInput = $true
$psi.RedirectStandardOutput = $true
$psi.RedirectStandardError = $true
$p = [System.Diagnostics.Process]::Start($psi)
$deadline = (Get-Date).AddMinutes(9)
while (-not $p.HasExited -and (Get-Date) -lt $deadline) {
    $line = $p.StandardOutput.ReadLine()
    if ($null -eq $line) { break }
    Add-Content -Path $out -Value $line
    if ($line -match '"event":"request"') { $p.StandardInput.WriteLine("y"); $p.StandardInput.Flush() }
    if ($line -match '"event":"(paired|declined)"') { break }
}
Start-Sleep 2
$p.StandardInput.WriteLine("quit"); $p.StandardInput.Close()
if (-not $p.WaitForExit(15000)) { $p.Kill() }
Add-Content -Path $out -Value ("exit=" + $p.ExitCode + " stderr: " + $p.StandardError.ReadToEnd())
