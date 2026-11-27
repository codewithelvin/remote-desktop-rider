param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = "",
    [string]$Pin = "rider-test-pin",
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-tunnel-switch")
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
        Start-Sleep -Milliseconds 500
    }
    throw "timed out waiting for '$pattern' in $file"
}
$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function Host-Cmd([string]$action, [string]$value = "") {
    Remove-Item (Join-Path $guest "host-cmd.out") -ErrorAction SilentlyContinue
    Guest ("host-cmd.ps1 $action $value").Trim()
    Wait-File "host-cmd.out" "^$action exit=" 30
}
function Through-Tunnel() {
    for ($attempt = 0; $attempt -lt 6; $attempt++) {
        $r = Through-Tunnel-Once
        if ($r.Why -notmatch 'busy with another session') { return $r }
        Start-Sleep 10
    }
    return $r
}
function Through-Tunnel-Once() {
    $v = New-Object Rdr+Proc $Client, "connect vm --pin-stdin --forward 0:127.0.0.1:18080", $env1
    $v.Send($Pin)
    $end = (Get-Date).AddSeconds(40)
    while ((Get-Date) -lt $end -and -not $v.P.HasExited -and $v.All() -notmatch 'Tunnel: 127\.0\.0\.1:(\d+)') { Start-Sleep -Milliseconds 300 }
    $got = $null
    if ($v.All() -match 'Tunnel: 127\.0\.0\.1:(\d+)') {
        $port = [int]$Matches[1]
        Start-Sleep 3
        try {
            $c = [System.Net.Sockets.TcpClient]::new(); $c.ReceiveTimeout = 15000; $c.Connect("127.0.0.1", $port)
            $got = [System.IO.StreamReader]::new($c.GetStream()).ReadLine(); $c.Close()
        } catch { $got = $null }
    }
    Start-Sleep 2
    $win = [Rdr]::WindowOf($v.P.Id)
    $why = $v.All() + $(if ($win -ne [IntPtr]::Zero) { [Rdr]::Title($win) } else { "" })
    $v.Stop()
    Start-Sleep 8
    return @{ Got = $got; Why = $why }
}

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["APPDATA"] = $Work; $env1["RIDER_LANG"] = "en"
$pairJob = $null
try {
    $c = New-Object Rdr+Proc $Client, "set-server $Server $Fingerprint", $env1
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
    Check "paired with the VM" ($pair.P.ExitCode -eq 0)
    Start-Sleep 3

    $token = "rdr-switch-" + [guid]::NewGuid().ToString("N").Substring(0, 12)
    Write-Host "  $(Host-Cmd tcp-target $token)"
    Write-Host "  $(Host-Cmd tunnels off)"
    $off = Through-Tunnel
    Check "tunnels off (the default): nothing gets through" (-not $off.Got)
    Check "tunnels off: the viewer is told they are turned off" ($off.Why -match 'turned off')
    Write-Host "  $(Host-Cmd tunnels allow)"
    $on = Through-Tunnel
    Check "tunnels allowed by the admin: data gets through" ($on.Got -eq $token)
    if ($on.Got -ne $token) { Write-Host "  got: '$($on.Got)'"; Write-Host "  viewer: $(($on.Why -split "`n" | Where-Object { $_ } | Select-Object -Last 4) -join ' | ')" }
} finally {
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    Host-Cmd tunnels off | Out-Null
    Host-Cmd revoke-all | Out-Null
    Guest "stop-tests.ps1"
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
