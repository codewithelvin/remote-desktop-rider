param(
    [string]$Server = "",
    [string]$Fingerprint = "",
    [switch]$Local,
    [string]$Client = "",
    [string]$Limit = "1m",
    [int]$Seconds = 120,
    [int]$MaxStallMs = 2000,
    [string]$DropTo = "",
    [int]$DropAfter = 30,
    [switch]$Udp
)
if (-not $Local -and -not ($Server -and $Fingerprint)) { throw "give -Server and -Fingerprint, or -Local for the Docker test server (test/server/server.sh up)" }
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
. "$PSScriptRoot\vmlib.ps1"
$vbox = "C:\Program Files\Oracle\VirtualBox\VBoxManage.exe"
$vm = "rider-test-host"
$group = "rdr-net"

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

$info = & $vbox showvminfo $vm
if (-not (($info -match "NIC 1:.*Bandwidth group: $group") -and ($info -match "NIC 2:.*Bandwidth group: $group"))) {
    throw "the VM's network cards are not in bandwidth group $group; with the VM off run: `"$vbox`" bandwidthctl $vm add $group --type network --limit 1000m; `"$vbox`" modifyvm $vm --nicbandwidthgroup1 $group --nicbandwidthgroup2 $group"
}
$viewerServer = $Server
if ($Local) {
    $Fingerprint = (& bash (Join-Path $repo "test/server/server.sh") fp | Select-Object -Last 1).Trim()
    if ($Fingerprint -notmatch '^[0-9a-f]{64}$') { throw "no local test server: run test/server/server.sh up" }
    $Server = "rider-test.local:47900"
    $viewerServer = "192.168.56.1:47900"
    Guest "trust-test-server.ps1" | Out-Null
}
$work = Join-Path $env:TEMP "rdr-tunnel-slow"
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["RIDER_LANG"] = "en"; $env1["APPDATA"] = $work
$setup = New-Object Rdr+Proc $Client, "set-server $viewerServer $Fingerprint", $env1
$setup.P.WaitForExit(20000) | Out-Null

Remove-Item (Join-Path $guest "support.out"), (Join-Path $guest "support.err") -ErrorAction SilentlyContinue
Set-Content (Join-Path $guest "support.args") "$Server $Fingerprint"
if (-not $Udp) { Set-Content (Join-Path $guest "block-udp") "" }
$viewer = $null
try {
    & $vbox bandwidthctl $vm set $group --limit $Limit
    Guest "start-motion.ps1" | Out-Null
    $code = ((Wait-File "support.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 90) | ConvertFrom-Json).code
    $viewer = New-Object Rdr+Proc $Client, "quick $code", $env1
    Wait-File "support.out" '"support_request"' | Out-Null
    [IO.File]::AppendAllText((Join-Path $guest "support.cmd"), "y view`r`n")
    Wait-File "support.out" '"support_started"' | Out-Null
    $end = (Get-Date).AddSeconds(20)
    do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end)
    if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
    [Rdr]::PlaceClient($win, 40, 40, 1280, 720) | Out-Null

    $tunnel = [bool](Get-Content (Join-Path $guest "support.err") -ErrorAction SilentlyContinue | Where-Object { $_ -match "HTTPS tunnel" })
    if ($Udp) { Check "the host in the VM uses UDP, not the tunnel" (-not $tunnel) } else { Check "the host in the VM reaches the server through the HTTPS tunnel" $tunnel }

    $rtts = @(); $fps = @(); $lostAt = $null; $dropped = $false
    $t0 = Get-Date
    while (((Get-Date) - $t0).TotalSeconds -lt $Seconds) {
        Start-Sleep 1
        if ($DropTo -and -not $dropped -and ((Get-Date) - $t0).TotalSeconds -ge $DropAfter) { & $vbox bandwidthctl $vm set $group --limit $DropTo; $dropped = $true }
        if ($viewer.P.HasExited -or $viewer.Find("onnection lost")) { $lostAt = [int]((Get-Date) - $t0).TotalSeconds; break }
        $title = [Rdr]::Title($win)
        if ($title -match 'RTT (\d+) ms') { $rtts += [int]$Matches[1] }
        if ($title -match '(\d+) fps') { $fps += [int]$Matches[1] }
    }
    $max = ($rtts | Measure-Object -Maximum).Maximum
    $stalls = @($rtts | Where-Object { $_ -gt 1000 }).Count
    $median = if ($rtts.Count) { ($rtts | Sort-Object)[[int]($rtts.Count / 2)] } else { 0 }
    $avgFps = if ($fps.Count) { [math]::Round(($fps | Measure-Object -Average).Average, 1) } else { 0 }
    Write-Host ("  link {0}{6}: RTT median {1} ms, max {2} ms, {3} s over 1 s, {4} fps on average, {5} samples" -f $Limit, $median, $max, $stalls, $avgFps, $rtts.Count, $(if ($DropTo) { " then $DropTo after $DropAfter s" } else { "" }))
    Check ("the session stays up for {0} s{1}" -f $Seconds, $(if ($null -ne $lostAt) { " (lost after $lostAt s)" } else { "" })) ($null -eq $lostAt)
    Check ("round trip never above {0} ms (max {1} ms)" -f $MaxStallMs, $max) ($rtts.Count -gt 0 -and $max -le $MaxStallMs)
    Check "the picture keeps moving (average $avgFps fps)" ($avgFps -ge 1)
} finally {
    if ($viewer -and -not $viewer.P.HasExited) { $viewer.P.Kill() }
    Remove-Item (Join-Path $guest "block-udp") -ErrorAction SilentlyContinue
    Guest "stop-tests.ps1" | Out-Null
    & $vbox bandwidthctl $vm set $group --limit 1000m
    if ($Local) { Guest "trust-test-server.ps1 -Remove" | Out-Null }
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
