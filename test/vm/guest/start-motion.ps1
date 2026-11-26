& C:\rider-test\stop-tests.ps1 | Out-Null
$args0 = (Get-Content C:\rider-test\support.args -Raw).Trim()
if (Test-Path C:\rider-test\block-udp) {
    New-NetFirewallRule -DisplayName "rdr-test-block-udp" -Direction Outbound -Protocol UDP -RemotePort 47900-47999 -Action Block | Out-Null
}
& C:\rider-test\run-as-user.ps1 motion.ps1
& C:\rider-test\run-as-user.ps1 support-auto.ps1 $args0
