param(
    [string[]]$Modes = @("cone", "symmetric"),
    [string]$Client = "",
    [string]$Pin = "rider-test-pin",
    [int]$Tries = 3,
    [int]$DelayMs = 0,
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-nat")
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
$routerDir = Join-Path $repo "test\server-vm"
$routerAddr = "192.168.56.10"
$vbox = Join-Path $env:ProgramFiles "Oracle\VirtualBox\VBoxManage.exe"
$hostOnly = "VirtualBox Host-Only Ethernet Adapter"
$expected = @{ cone = "direct"; symmetric = "relayed" }
. "$PSScriptRoot\vmlib.ps1"

function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    throw "timed out waiting for '$pattern' in $file"
}
$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function Router([string]$command) {
    Push-Location $routerDir
    $ErrorActionPreference = "Continue"
    try { (& vagrant ssh -c $command 2>$null) -join "`n" } finally { Pop-Location }
}
function Host-Cmd([string]$action, [string]$value = "", [string]$extra = "") {
    Remove-Item (Join-Path $guest "host-cmd.out") -ErrorAction SilentlyContinue
    Guest ("host-cmd.ps1 $action $value $extra").Trim()
    Wait-File "host-cmd.out" "^$action exit=" 30
}
function Guest-Net([string]$mode, [string]$mac) {
    Remove-Item (Join-Path $guest "nat-net.out") -ErrorAction SilentlyContinue
    Guest "nat-net.ps1 $mode $mac"
    $line = Wait-File "nat-net.out" "^$mode " 60
    if ($line -notmatch ' ok$') { throw "guest network: $line" }
}
function Router-Now { (Router "date +%s").Trim() }
function Wait-Online([string]$since) {
    $end = (Get-Date).AddSeconds(60)
    do {
        $log = (Router "sudo journalctl -u rider-server --no-pager -o cat --since @$since") -replace "$([char]27)\[[0-9;]*m", ''
        $hit = [regex]::Matches($log, 'online from (\S+)')
        if ($hit.Count) { return $hit[$hit.Count - 1].Groups[1].Value }
        Start-Sleep 2
    } while ((Get-Date) -lt $end)
    throw "the host did not come back online at the server"
}
function Connect-Once([string]$log) {
    for ($attempt = 0; $attempt -lt 6; $attempt++) {
        $clock = [Diagnostics.Stopwatch]::StartNew()
        $v = New-Object Rdr+Proc $Client, "connect vm --pin-stdin", $env1
        $v.Send($Pin)
        $end = (Get-Date).AddSeconds(30)
        do { Start-Sleep -Milliseconds 100; $text = $v.All() } while (-not $v.P.HasExited -and $text -notmatch 'Connected to' -and (Get-Date) -lt $end)
        $seconds = $clock.Elapsed.TotalSeconds
        $v.Stop()
        $text | Set-Content $log
        if ($text -notmatch 'busy with another session') { break }
        Start-Sleep 5
    }
    Start-Sleep 3
    $route = if ($text -match 'Connected to "[^"]*" \(([^)]*)\)') { $Matches[1] } else { "" }
    if (-not $route) { Write-Host "  viewer: $(($text -split "`n" | Where-Object { $_ } | Select-Object -Last 1))" }
    [pscustomobject]@{ Route = $route; Seconds = [math]::Round($seconds, 1) }
}

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["APPDATA"] = $Work; $env1["RIDER_LANG"] = "en"
if ($env:RUST_LOG) { $env1["RUST_LOG"] = $env:RUST_LOG }
$mac = ((& $vbox showvminfo rider-test-host --machinereadable) | Where-Object { $_ -match '^macaddress2=' }) -replace '^macaddress2="|"$', ''
$hostServer = $null; $pairJob = $null; $natOn = $false
$table = @()
try {
    Router "sudo install -m755 /src/target/dist-linux/rider-server /usr/local/bin/rider-server && sudo systemctl restart rider-server" | Out-Null
    Start-Sleep 2
    $fp = [regex]::Match((Router "sudo journalctl -u rider-server -n 50 --no-pager"), 'Server fingerprint: ([0-9a-f]{64})').Groups[1].Value
    if (-not $fp) { throw "no server fingerprint on the router" }
    Write-Host "router server $routerAddr $($fp.Substring(0, 12))..."

    $hostServer = ((Host-Cmd info) -replace '^info exit=\S* ', '' | ConvertFrom-Json).server
    & $vbox controlvm rider-test-host nic2 intnet rdr-nat
    $natOn = $true
    Write-Host "  $(Router 'sudo sh /src/test/server-vm/nat-router.sh cone')"
    Guest-Net on $mac
    Write-Host "  $(Host-Cmd set-server $routerAddr $fp)"

    $c = New-Object Rdr+Proc $Client, "set-server $routerAddr $fp", $env1
    $c.P.WaitForExit(15000) | Out-Null
    Remove-Item (Join-Path $guest "pair.out") -ErrorAction SilentlyContinue
    $pairJob = Guest-Start "pair-auto.ps1"
    $code = ((Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120) | ConvertFrom-Json).code
    $pair = New-Object Rdr+Proc $Client, "pair $code --name vm", $env1
    Wait-File "pair.out" '"event":"request"' 30 | Out-Null
    Start-Sleep 1
    $pair.Send("y")
    $pair.P.WaitForExit(60000) | Out-Null
    Wait-File "pair.out" '^exit=' 60 | Out-Null
    Check "paired through the NAT" ($pair.P.ExitCode -eq 0)

    foreach ($mode in $Modes) {
        Write-Host "  $(Router "sudo sh /src/test/server-vm/nat-router.sh $mode $DelayMs")"
        for ($i = 1; $i -le $Tries; $i++) {
            $since = Router-Now
            Host-Cmd restart | Out-Null
            $public = Wait-Online $since
            $r = Connect-Once (Join-Path $Work "viewer-$mode-$i.log")
            Write-Host ("  {0} #{1}: host seen as {2}, {3} in {4} s" -f $mode, $i, $public, $(if ($r.Route) { $r.Route } else { "no connection" }), $r.Seconds)
            $table += [pscustomobject]@{ Nat = $mode; Try = $i; HostSeenAs = $public; Route = $r.Route; Seconds = $r.Seconds }
        }
        $runs = @($table | Where-Object Nat -eq $mode)
        Check "$mode NAT: every try connects" (@($runs | Where-Object { -not $_.Route }).Count -eq 0)
        Check "$mode NAT: the path is $($expected[$mode])" (@($runs | Where-Object { $_.Route -notlike "$($expected[$mode])*" }).Count -eq 0)
    }
} finally {
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    if ($natOn) {
        Router "sudo sh /src/test/server-vm/nat-router.sh off" | Out-Null
        & $vbox controlvm rider-test-host nic2 hostonly $hostOnly
        Guest-Net off $mac
    }
    if ($hostServer) { Host-Cmd set-server $hostServer.addr $hostServer.fingerprint | Out-Null }
    Guest "stop-tests.ps1"
}
$table | Format-Table -AutoSize | Out-String | Write-Host
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
