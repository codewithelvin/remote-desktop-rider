Get-WinEvent -FilterHashtable @{ LogName = "System"; Id = 1074, 6005, 6006, 6008, 41; StartTime = (Get-Date).AddHours(-2) } -ErrorAction SilentlyContinue |
    Sort-Object TimeCreated | ForEach-Object { "{0:o} {1} {2}" -f $_.TimeCreated.ToUniversalTime(), $_.Id, ($_.Message -split "`n")[0] } |
    Set-Content C:\rider-test\shutdown-events.out
