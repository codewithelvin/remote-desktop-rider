# Test VM only: start the "tab strip" (target.ps1) and a one-time support
# host (support-auto.ps1) in the signed-in user's session. The server and
# its fingerprint come from C:\rider-test\support.args ("<server> <hex>").
# Leftovers of an earlier run would read the same command file.
& C:\rider-test\stop-tests.ps1 | Out-Null
$args0 = (Get-Content C:\rider-test\support.args -Raw).Trim()
& C:\rider-test\run-as-user.ps1 target.ps1
& C:\rider-test\run-as-user.ps1 support-auto.ps1 $args0
