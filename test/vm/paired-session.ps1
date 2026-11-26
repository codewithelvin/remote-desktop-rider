param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Client = "",
    [string]$Pin = "rider-test-pin",
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-paired")
)
. "$PSScriptRoot\..\local-session\lib.ps1"
if (-not $Client) { $Client = Join-Path $repo "target\release\rider-client.exe" }
$guest = Join-Path $PSScriptRoot "guest"
$vm = $PSScriptRoot

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
function Clicks() { @(Get-Content (Join-Path $guest "clicks.out") -ErrorAction SilentlyContinue | Where-Object { $_ }).Count }
$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}

if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
New-Item -ItemType Directory -Force $Work | Out-Null
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["APPDATA"] = $Work; $env1["RIDER_LANG"] = "en"
$viewer = $null; $pairJob = $null
try {
    $c = New-Object Rdr+Proc $Client, "set-server $Server $Fingerprint", $env1
    $c.P.WaitForExit(15000) | Out-Null

    Remove-Item (Join-Path $guest "pair.out") -ErrorAction SilentlyContinue
    $pairJob = Guest-Start "pair-auto.ps1"
    $code = ((Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120) | ConvertFrom-Json).code
    Write-Host "pairing code $code"
    $pair = New-Object Rdr+Proc $Client, "pair $code --name vm", $env1
    Wait-File "pair.out" '"event":"request"' 30 | Out-Null
    Start-Sleep 1
    $pair.Send("y")
    $pair.P.WaitForExit(60000) | Out-Null
    Wait-File "pair.out" '"event":"paired"' 60 | Out-Null
    Check "paired with the VM by code" ($pair.P.ExitCode -eq 0)
    Wait-File "pair.out" '^exit=' 30 | Out-Null
    Start-Sleep 3

    Remove-Item (Join-Path $guest "screen.out") -ErrorAction SilentlyContinue
    Guest "run-as-user.ps1 target.ps1"
    $size = (Wait-File "screen.out" '^\d+ \d+$') -split ' '
    $vw = [int]$size[0]; $vh = [int]$size[1]
    $viewer = New-Object Rdr+Proc $Client, "connect vm --pin-stdin", $env1
    $viewer.Send($Pin)
    $end = (Get-Date).AddSeconds(40)
    do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end -and -not $viewer.P.HasExited)
    if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
    [Rdr]::Place($win, 100, 100, 1000, 900)
    Wait-Until { [Rdr]::Title($win) -match ' [1-9]\d* fps' } 20 | Out-Null
    Start-Sleep 3
    $title = [Rdr]::Title($win)
    Write-Host "  viewer title: $title"
    Check "full access: no access label in the title" (-not ($title -like "*[[]*"))
    $before = Clicks
    [Rdr]::ClickHostPixel($win, [int]($vw / 2 - 360), 150, $vw, $vh) | Out-Null
    Check "full access: the click arrives" ((Wait-Until { (Clicks) -ge $before + 1 } 5) -and (Clicks) -eq $before + 1)
    [Rdr]::ClickHostPixel($win, [int]($vw / 2 + 150), 17, $vw, $vh) | Out-Null
    Check "a click on the banner spot reaches the window under it" ((Wait-Until { (Clicks) -ge $before + 2 } 5) -and (Clicks) -eq $before + 2)
    Check "the session is still up" (-not $viewer.P.HasExited)
} finally {
    if ($viewer) { $viewer.Stop() }
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
    Guest "stop-tests.ps1"
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
