$dir = "C:\rider-test\agent"
New-Item -ItemType Directory -Force $dir | Out-Null
Get-ChildItem $dir -File -Exclude *.run | Remove-Item -Force -ErrorAction SilentlyContinue
Get-ChildItem $dir -Filter *.run -File | Where-Object { $_.LastWriteTime -lt (Get-Date).AddSeconds(-30) } |
    Remove-Item -Force -ErrorAction SilentlyContinue
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
