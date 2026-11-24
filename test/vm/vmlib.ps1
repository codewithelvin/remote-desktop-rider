# Shared by the test/vm scripts (dot-source it): run a script from guest/ in
# the Windows test VM. Through the agent in the VM (guest/agent.ps1, about
# half a second) when it is running, otherwise through `vagrant winrm`
# (5-15 s). Either way with a time limit: a stuck VM fails the test instead
# of hanging it.
$vmDir = $PSScriptRoot
$guestDir = Join-Path $PSScriptRoot "guest"

function Agent-Alive {
    # The time written inside (the shared folder does not update the file's
    # own time on this side). The VM's clock follows this PC's.
    $f = Join-Path $guestDir "agent\alive"
    $t = Get-Content $f -Raw -ErrorAction SilentlyContinue
    if (-not $t) { return $false }
    try { [math]::Abs(((Get-Date) - [datetime]::Parse($t.Trim())).TotalSeconds) -lt 10 } catch { $false }
}

# Guest "start-tests.ps1" / Guest "run-as-user.ps1 target.ps1"
function Guest([string]$script, [int]$seconds = 120) {
    if (Agent-Alive) {
        $id = [guid]::NewGuid().ToString("N").Substring(0, 8)
        $agent = Join-Path $guestDir "agent"
        Set-Content -Path (Join-Path $agent "$id.tmp") -Value $script -NoNewline
        Rename-Item (Join-Path $agent "$id.tmp") "$id.run"   # whole, never half-written
        $done = Join-Path $agent "$id.done"
        $end = (Get-Date).AddSeconds($seconds)
        while (-not (Test-Path $done)) {
            if ((Get-Date) -gt $end) { throw "the VM did not finish '$script' in $seconds s" }
            Start-Sleep -Milliseconds 100
        }
        Remove-Item (Join-Path $agent "$id.*") -ErrorAction SilentlyContinue
        return
    }
    $job = Start-Job -ArgumentList $vmDir, $script -ScriptBlock {
        param($dir, $script)
        Set-Location $dir
        & vagrant winrm -c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\rider-test\$script" 2>&1 | Out-Null
    }
    if (-not (Wait-Job $job -Timeout $seconds)) {
        Stop-Job $job; Remove-Job $job -Force
        throw "vagrant winrm did not finish '$script' in $seconds s (restart the VM: vagrant reload)"
    }
    Remove-Job $job -Force
}

# The same in the background (a script that waits, such as pair-auto.ps1).
function Guest-Start([string]$script) {
    Start-Job -ArgumentList $PSScriptRoot, $script -ScriptBlock {
        param($dir, $script)
        . (Join-Path $dir "vmlib.ps1")
        Guest $script 300
    }
}

# Poll until $test returns something truthy (or the time is up); returns it.
function Wait-Until([scriptblock]$test, [int]$seconds = 10) {
    $end = (Get-Date).AddSeconds($seconds)
    do {
        $r = & $test
        if ($r) { return $r }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $end)
    return $null
}
