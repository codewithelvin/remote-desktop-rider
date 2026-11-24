# Test VM only: is the tests' agent (agent.ps1) running?
Get-ScheduledTask rider-test-agent -ErrorAction SilentlyContinue | Select-Object TaskName, State | Format-Table -AutoSize | Out-String
"last result: " + (Get-ScheduledTaskInfo rider-test-agent -ErrorAction SilentlyContinue).LastTaskResult
$procs = @(Get-CimInstance Win32_Process -Filter "Name='powershell.exe'" | Where-Object { $_.CommandLine -like '*\rider-test\agent.ps1*' })
"processes: " + $procs.Count
"folder: " + (Test-Path C:\rider-test\agent)
"agent log: " + (Get-Content C:\Windows\Temp\rider-test-agent.log -Raw -ErrorAction SilentlyContinue)
