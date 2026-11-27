param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = "",
    [string]$Pin = "rider-test-pin",
    [string]$NewPin = "rider-test-pin-2",
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-pin-revoke")
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
function Try-Connect([string]$pin) {
    for ($attempt = 0; $attempt -lt 6; $attempt++) {
        $v = New-Object Rdr+Proc $Client, "connect vm --pin-stdin", $env1
        $v.Send($pin)
        $end = (Get-Date).AddSeconds(30)
        do { Start-Sleep -Milliseconds 300; $text = $v.All() } while (-not $v.P.HasExited -and $text -notmatch 'Connected to' -and (Get-Date) -lt $end)
        Start-Sleep 3
        $text = $v.All()
        $up = -not $v.P.HasExited -and $text -match 'Connected to'
        $v.Stop()
        if ($text -notmatch 'busy with another session') { break }
        Start-Sleep 5
    }
    Start-Sleep 5
    $last = ($text -split "`n" | Where-Object { $_ } | Select-Object -Last 1)
    Write-Host "  viewer: $last"
    return @{ Up = $up; WrongPin = $text -match 'wrong PIN'; NotPaired = $text -match 'not paired' }
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

    Check "connects with the current PIN" (Try-Connect $Pin).Up
    Write-Host "  $(Host-Cmd set-pin $NewPin)"
    $old = Try-Connect $Pin
    Check "the old PIN is refused as wrong right after set-pin, no restart" ((-not $old.Up) -and $old.WrongPin)
    Check "the new PIN works right away" (Try-Connect $NewPin).Up
    Write-Host "  $(Host-Cmd set-pin $Pin)"
    Write-Host "  $(Host-Cmd revoke-all)"
    $gone = Try-Connect $Pin
    Check "a removed device is refused as not paired right away, no restart" ((-not $gone.Up) -and $gone.NotPaired)
} finally {
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    Host-Cmd set-pin $Pin | Out-Null
    Guest "stop-tests.ps1"
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
