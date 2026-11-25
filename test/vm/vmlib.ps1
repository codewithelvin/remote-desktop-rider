$vmDir = $PSScriptRoot
$guestDir = Join-Path $PSScriptRoot "guest"

function Agent-Alive {
    $f = Join-Path $guestDir "agent\alive"
    $t = Get-Content $f -Raw -ErrorAction SilentlyContinue
    if (-not $t) { return $false }
    try { [math]::Abs(((Get-Date) - [datetime]::Parse($t.Trim())).TotalSeconds) -lt 10 } catch { $false }
}

function Guest([string]$script, [int]$seconds = 120) {
    $alive = Agent-Alive
    for ($i = 0; -not $alive -and $i -lt 16 -and (Test-Path (Join-Path $guestDir "agent\alive")); $i++) {
        Start-Sleep -Milliseconds 500
        $alive = Agent-Alive
    }
    if ($alive) {
        $id = [guid]::NewGuid().ToString("N").Substring(0, 8)
        $agent = Join-Path $guestDir "agent"
        Set-Content -Path (Join-Path $agent "$id.tmp") -Value $script -NoNewline
        Rename-Item (Join-Path $agent "$id.tmp") "$id.run"
        $done = Join-Path $agent "$id.done"
        $end = (Get-Date).AddSeconds($seconds)
        while (-not (Test-Path $done)) {
            if ((Get-Date) -gt $end) {
                Remove-Item (Join-Path $agent "$id.run") -ErrorAction SilentlyContinue
                throw "the VM did not finish '$script' in $seconds s"
            }
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

function Guest-Start([string]$script) {
    Start-Job -ArgumentList $PSScriptRoot, $script -ScriptBlock {
        param($dir, $script)
        . (Join-Path $dir "vmlib.ps1")
        Guest $script 300
    }
}

function Wait-Until([scriptblock]$test, [int]$seconds = 10) {
    $end = (Get-Date).AddSeconds($seconds)
    do {
        $r = & $test
        if ($r) { return $r }
        Start-Sleep -Milliseconds 200
    } while ((Get-Date) -lt $end)
    return $null
}
