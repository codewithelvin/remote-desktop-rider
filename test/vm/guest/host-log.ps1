$d = "$env:ProgramData\RemoteDesktopRider\host\logs"
(Get-ChildItem $d -File | Sort-Object LastWriteTime | Select-Object -Last 6 | ForEach-Object { "## {0} {1} {2}" -f $_.Name, $_.Length, $_.LastWriteTime }) | Set-Content C:\rider-test\host-log.out
$a = Get-ChildItem $d -File -Filter "*agent*" | Sort-Object LastWriteTime | Select-Object -Last 1
if ($a) { Get-Content $a.FullName -Tail 200 | Add-Content C:\rider-test\host-log.out }
