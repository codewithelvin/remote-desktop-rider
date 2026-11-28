(Get-CimInstance Win32_OperatingSystem).LastBootUpTime.ToUniversalTime().ToString("o") | Set-Content C:\rider-test\boot.out
