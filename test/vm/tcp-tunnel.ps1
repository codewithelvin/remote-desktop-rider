param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = ""
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
. "$PSScriptRoot\vmlib.ps1"

function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 300
    }
    return $null
}
function Count([string]$pattern) {
    @(Get-Content (Join-Path $guest "support.out") -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern }).Count
}
$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function Fetch([int]$port) {
    try {
        $c = [System.Net.Sockets.TcpClient]::new()
        $c.ReceiveTimeout = 15000
        $c.Connect("127.0.0.1", $port)
        $r = [System.IO.StreamReader]::new($c.GetStream())
        $line = $r.ReadLine()
        $c.Close()
        return $line
    } catch { return $null }
}
function Answer([string]$target, [string]$verb) {
    $req = Wait-File "support.out" ('"tunnel_request".*' + [regex]::Escape($target)) 20
    if (-not $req) { return $false }
    $id = ($req | ConvertFrom-Json).id
    [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "$verb $id`r`n")
    return $true
}

$token = "rdr-tunnel-" + [guid]::NewGuid().ToString("N").Substring(0, 12)
Set-Content (Join-Path $guest "tcp-target.token") $token
Remove-Item (Join-Path $guest "support.out") -ErrorAction SilentlyContinue
Set-Content (Join-Path $guest "support.args") "$Server $Fingerprint"
$work = Join-Path $env:TEMP "rdr-tcp-tunnel"
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["RIDER_LANG"] = "en"; $env1["APPDATA"] = $work
$setup = New-Object Rdr+Proc $Client, "set-server $Server $Fingerprint", $env1
$setup.P.WaitForExit(20000) | Out-Null
$viewer = $null
try {
    Guest "start-tcp.ps1" | Out-Null
    $code = ((Wait-File "support.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 90) | ConvertFrom-Json).code
    $forwards = "--forward 0:127.0.0.1:18080 --forward 0:127.0.0.1:18081 --forward 0:8.8.8.8:53"
    $viewer = New-Object Rdr+Proc $Client, "quick $code $forwards", $env1
    Wait-File "support.out" '"support_request"' | Out-Null
    [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "y`r`n")
    Wait-File "support.out" '"support_started"' | Out-Null
    $end = (Get-Date).AddSeconds(20)
    while (@($viewer.All() -split "`n" | Where-Object { $_ -match '^Tunnel: ' }).Count -lt 3 -and (Get-Date) -lt $end) { Start-Sleep -Milliseconds 300 }
    $ports = @{}
    foreach ($l in ($viewer.All() -split "`n" | Where-Object { $_ -match '^Tunnel: 127\.0\.0\.1:(\d+) -> (\S+)' })) {
        if ($l -match '^Tunnel: 127\.0\.0\.1:(\d+) -> (\S+)') { $ports[$Matches[2].Trim()] = [int]$Matches[1] }
    }
    Check "the viewer listens for all three tunnels on this PC ($($ports.Count))" ($ports.Count -eq 3)

    $job = Start-Job -ArgumentList $ports["127.0.0.1:18080"] -ScriptBlock {
        param($p)
        try { $c = [System.Net.Sockets.TcpClient]::new(); $c.ReceiveTimeout = 30000; $c.Connect("127.0.0.1", $p)
            $line = [System.IO.StreamReader]::new($c.GetStream()).ReadLine(); $c.Close(); $line } catch { $null }
    }
    $asked = Answer "127.0.0.1:18080" "tunnel-allow"
    Check "the person at the host is asked before the first connection" $asked
    $got = Receive-Job -Job ($job | Wait-Job -Timeout 40)
    Check "an approved tunnel carries data from the host's port to this PC" ($got -eq $token)
    Check "a second connection to the same address is not asked again and works" ((Fetch $ports["127.0.0.1:18080"]) -eq $token -and (Count '"tunnel_request"') -eq 1)

    $job = Start-Job -ArgumentList $ports["127.0.0.1:18081"] -ScriptBlock {
        param($p)
        try { $c = [System.Net.Sockets.TcpClient]::new(); $c.ReceiveTimeout = 30000; $c.Connect("127.0.0.1", $p)
            $line = [System.IO.StreamReader]::new($c.GetStream()).ReadLine(); $c.Close(); $line } catch { $null }
    }
    $asked = Answer "127.0.0.1:18081" "tunnel-deny"
    $got = Receive-Job -Job ($job | Wait-Job -Timeout 40)
    Check "a denied tunnel carries nothing" ($asked -and -not $got)
    $again = Fetch $ports["127.0.0.1:18081"]
    Check "a denied address is refused again without asking" (-not $again -and (Count '"tunnel_request"') -eq 2)
    Check "the viewer shows why it was refused" ([bool]($viewer.All() + [Rdr]::Title([Rdr]::WindowOf($viewer.P.Id)) -match "declined"))

    $public = Fetch $ports["8.8.8.8:53"]
    Start-Sleep 1
    Check "an internet address is refused without asking" (-not $public -and (Count '"tunnel_request"') -eq 2)

    [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "access view`r`n")
    Start-Sleep 2
    Check "in View only, even an approved address is refused" (-not (Fetch $ports["127.0.0.1:18080"]))
    Check "the session is still up" (-not $viewer.P.HasExited)
} finally {
    if ($viewer -and -not $viewer.P.HasExited) { $viewer.P.Kill() }
    Guest "stop-tests.ps1" | Out-Null
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
