& C:\rider-test\stop-tests.ps1 | Out-Null
$args0 = (Get-Content C:\rider-test\support.args -Raw).Trim()
& C:\rider-test\run-as-user.ps1 target.ps1
& C:\rider-test\run-as-user.ps1 support-auto.ps1 $args0
