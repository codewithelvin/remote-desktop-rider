$dir = "C:\ProgramData\RemoteDesktopRider\host\logs"
$files = Get-ChildItem $dir -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 2
& {
    $files | Select-Object Name, LastWriteTime | Format-Table | Out-String
    foreach ($f in $files) { "== $($f.Name)"; Get-Content $f.FullName -Tail 400 }
} *>&1 | Out-File C:\rider-test\ent-log.out -Encoding UTF8
