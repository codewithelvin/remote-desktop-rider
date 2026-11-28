param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$RestoreServer = "rdm.codex.az",
    [string]$RestoreFingerprint = "ca3dbb3efab7faacb4f4bb18f7db38c3d8eb0744eee4d7b992d41ed6083f1961",
    [string]$Client = "",
    [string]$Pin = "rider-test-pin",
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-wake")
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
$vbox = Join-Path $env:ProgramFiles "Oracle\VirtualBox\VBoxManage.exe"
. "$PSScriptRoot\vmlib.ps1"

$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    throw "timed out waiting for '$pattern' in $file"
}
function Wake-Guest([string]$words) {
    Set-Content (Join-Path $guest "wake-args.txt") $words -NoNewline
    Remove-Item (Join-Path $guest "wake-guest.out") -ErrorAction SilentlyContinue
    Guest "wake-guest.ps1" 120
    (Get-Content (Join-Path $guest "wake-guest.out") -Raw).Trim()
}
function Magic-Hex([string]$mac) {
    $m = ($mac -replace '[-:]', '').ToUpper()
    ("FF" * 6) + ($m * 16)
}
function Trace-Has([string]$file, [string]$mac) {
    $bytes = [IO.File]::ReadAllBytes($file)
    [BitConverter]::ToString($bytes).Replace("-", "").Contains((Magic-Hex $mac))
}
function Viewer([string]$arguments) {
    $p = New-Object Rdr+Proc $Client, $arguments, $env1
    $p.P.WaitForExit(30000) | Out-Null
    $p
}

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["APPDATA"] = $Work; $env1["RIDER_LANG"] = "en"
$trace = Join-Path $Work "hostonly.pcap"
$viewer = $null; $pairJob = $null
try {
    Write-Host "  $(Wake-Guest "server $Server $Fingerprint")"
    Start-Sleep 5
    Viewer "set-server $Server $Fingerprint" | Out-Null
    Remove-Item (Join-Path $guest "pair.out") -ErrorAction SilentlyContinue
    $pairJob = Guest-Start "pair-auto.ps1"
    $code = ((Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120) | ConvertFrom-Json).code
    $pair = New-Object Rdr+Proc $Client, "pair $code --name vm", $env1
    Wait-File "pair.out" '"event":"request"' 30 | Out-Null
    Start-Sleep 1
    $pair.Send("y")
    $pair.P.WaitForExit(60000) | Out-Null
    Wait-File "pair.out" '^exit=' 30 | Out-Null
    Check "paired with the VM through the test server" ($pair.P.ExitCode -eq 0)
    Start-Sleep 3

    $config = Join-Path $Work "RemoteDesktopRider\client\client.toml"
    $before = Get-Content $config -Raw
    Check "a new pairing has no wake addresses yet" ($before -notmatch 'wake = ')
    $viewer = New-Object Rdr+Proc $Client, "connect vm --pin-stdin", $env1
    $viewer.Send($Pin)
    $saved = Wait-Until { (Get-Content $config -Raw) -match 'wake = \[' } 40
    $viewer.P.Kill(); $viewer = $null
    $toml = Get-Content $config -Raw
    $macs = (Wake-Guest "macs") -replace '^macs ', '' -split ' '
    $hostOnly = (& $vbox showvminfo rider-test-host --machinereadable | Select-String '^macaddress2="(\w+)"').Matches[0].Groups[1].Value
    $hostOnly = ($hostOnly -split '(..)' | Where-Object { $_ }) -join '-'
    Check "the viewer saved the computer's wired addresses after a session" ([bool]$saved -and ($macs | ForEach-Object { $toml -match [regex]::Escape($_) }) -notcontains $false)
    Write-Host "  wake addresses: $($macs -join ', ')"

    $armed = Wake-Guest "armed"
    Check "remote access armed the wired adapters to wake the computer" ($armed -match 'PRO/1000')
    Write-Host "  $armed"

    Write-Host "  $(Wake-Guest "helper $Server $Fingerprint")"
    Start-Sleep 8
    Write-Host "  $(Wake-Guest "stop")"
    Start-Sleep 5

    & $vbox controlvm rider-test-host nictracefile2 $trace
    & $vbox controlvm rider-test-host nictrace2 on
    Start-Sleep 1
    $wake = Viewer "wake vm"
    $said = $wake.All()
    Start-Sleep 3
    & $vbox controlvm rider-test-host nictrace2 off
    Write-Host "  viewer: $($said.Trim())"
    Check "the server passed the wake-up to one helper on the computer's network" ($said -match 'through 1 computer')
    Check "the helper broadcast a magic packet for the sleeping computer on its network" ((Test-Path $trace) -and (Trace-Has $trace $hostOnly))

    $stranger = Join-Path $Work "stranger"
    $env2 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $env2["APPDATA"] = $stranger; $env2["RIDER_LANG"] = "en"
    (New-Object Rdr+Proc $Client, "set-server $Server $Fingerprint", $env2).P.WaitForExit(15000) | Out-Null
    $strangerCfg = Join-Path $stranger "RemoteDesktopRider\client\client.toml"
    $strangerToml = Get-Content $strangerCfg -Raw
    $hostBlock = ($toml -split '(?=\[\[hosts\]\])' | Where-Object { $_ -match '^\[\[hosts\]\]' }) -join ''
    Set-Content $strangerCfg ($strangerToml.TrimEnd() + "`n`n" + $hostBlock) -NoNewline
    $denied = New-Object Rdr+Proc $Client, "wake vm", $env2
    $denied.P.WaitForExit(30000) | Out-Null
    Write-Host "  stranger: $($denied.All().Trim())"
    Check "a device the computer never paired with cannot wake it through the server" ($denied.P.ExitCode -ne 0 -and $denied.All() -match 'does not know where')
} finally {
    if ($viewer -and -not $viewer.P.HasExited) { $viewer.P.Kill() }
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    & $vbox controlvm rider-test-host nictrace2 off 2>&1 | Out-Null
    try { Write-Host "  $(Wake-Guest "unhelp")" } catch { Write-Host "  cleanup: $_" }
    try { Write-Host "  $(Wake-Guest "server $RestoreServer $RestoreFingerprint")" } catch { Write-Host "  cleanup: $_" }
    try { Write-Host "  $(Wake-Guest "start")" } catch { Write-Host "  cleanup: $_" }
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
Write-Host ""
Write-Host "  $($results.Count - $failed)/$($results.Count) passed"
exit $failed
