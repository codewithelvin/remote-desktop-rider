# Test VM only: runs the test scripts the PC asks for, so the tests need no
# `vagrant winrm` per step (each one starts Ruby and opens WinRM: 5-15 s, and
# it can hang). Started at boot by install-agent.ps1, with full rights.
#
# The PC writes C:\rider-test\agent\<id>.run holding "<script> [arguments]";
# the agent runs C:\rider-test\<script>, writes its output to <id>.log and
# then <id>.done (0 or 1). C:\rider-test\agent\alive holds the time of the
# last loop: the PC uses the WinRM path when it is old.
#
# Scripts run inside this process (a fresh runspace each): starting a new
# powershell.exe takes about 6 s in this VM.
$dir = "C:\rider-test\agent"
New-Item -ItemType Directory -Force $dir | Out-Null
# Leftovers of earlier runs, but not a request that is already waiting.
Get-ChildItem $dir -File -Exclude *.run | Remove-Item -Force -ErrorAction SilentlyContinue
while ($true) {
    Set-Content "$dir\alive" (Get-Date -Format o)
    foreach ($req in Get-ChildItem $dir -Filter *.run -File -ErrorAction SilentlyContinue) {
        $id = $req.BaseName
        $line = Get-Content $req.FullName -Raw -ErrorAction SilentlyContinue
        Remove-Item $req.FullName -Force -ErrorAction SilentlyContinue
        if (-not $line) { continue }
        $script, $rest = $line.Trim() -split ' ', 2
        $ok = $false
        $ps = [powershell]::Create()
        try {
            [void]$ps.AddCommand("C:\rider-test\$script")
            foreach ($a in @($rest -split ' ' | Where-Object { $_ })) { [void]$ps.AddArgument($a) }
            $run = $ps.BeginInvoke()
            # Each script ends in seconds; a stuck one must not stop the agent.
            if ($run.AsyncWaitHandle.WaitOne(300000)) {
                $ps.EndInvoke($run) | Out-String | Set-Content "$dir\$id.log"
                $ok = -not $ps.HadErrors
                if (-not $ok) { $ps.Streams.Error | Out-String | Set-Content "$dir\$id.err" }
            } else {
                $ps.Stop()
                Set-Content "$dir\$id.err" "stopped after 5 minutes"
            }
        } catch {
            Set-Content "$dir\$id.err" $_
        } finally {
            $ps.Dispose()
        }
        Set-Content "$dir\$id.done" $(if ($ok) { 0 } else { 1 })
    }
    Start-Sleep -Milliseconds 200
}
