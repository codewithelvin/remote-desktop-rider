& C:\rider-test\stop-tests.ps1 | Out-Null
$args0 = (Get-Content C:\rider-test\support.args -Raw).Trim()
$token = (Get-Content C:\rider-test\tcp-target.token -Raw).Trim()
Start-Process powershell -WindowStyle Hidden -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File C:\rider-test\tcp-target.ps1 -Port 18080 -Token $token"
Start-Process powershell -WindowStyle Hidden -ArgumentList "-NoProfile -ExecutionPolicy Bypass -File C:\rider-test\tcp-target.ps1 -Port 18081 -Token other"
& C:\rider-test\run-as-user.ps1 support-auto.ps1 $args0
