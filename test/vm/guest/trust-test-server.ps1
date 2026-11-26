param([switch]$Remove)
$hosts = "$env:SystemRoot\System32\drivers\etc\hosts"
$line = "192.168.56.1 rider-test.local"
$kept = @(Get-Content $hosts | Where-Object { $_ -notmatch "rider-test\.local" })
Get-ChildItem Cert:\LocalMachine\Root | Where-Object Subject -eq "CN=rider-test.local" | Remove-Item
if (-not $Remove) {
    Import-Certificate -FilePath C:\rider-test\rider-test-cert.pem -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    $kept += $line
}
Set-Content $hosts $kept
Clear-DnsClientCache
"trusted: $(-not $Remove)"
