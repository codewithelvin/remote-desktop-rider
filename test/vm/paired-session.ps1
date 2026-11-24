# The everyday path across two machines: a paired, PIN-protected session from
# this PC to the Windows test VM's unattended service, through the real
# rendezvous server. A scratch viewer identity is paired (your own viewer
# settings are not touched). Checks: full access (clicks arrive), a click on
# the hidden banner's spot reaches the window under it, the session stays up.
#
#   cargo build --release --workspace
#   cd test/vm && vagrant up; vagrant winrm -c '& C:\rider\rider-host.exe service install'
#   powershell -ExecutionPolicy Bypass -File test\vm\paired-session.ps1 -Server rdm.example.com -Fingerprint <hex>
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

function Guest([string]$script) {
    Push-Location $vm
    $old = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    try { & vagrant winrm -c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\rider-test\$script" 2>&1 | Out-Null }
    finally { $ErrorActionPreference = $old; Pop-Location }
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

    # Pairing: the VM approves the first request (pair-auto.ps1, elevated over WinRM).
    Remove-Item (Join-Path $guest "pair.out") -ErrorAction SilentlyContinue
    $pairJob = Start-Job -ArgumentList $vm -ScriptBlock { param($vm) Set-Location $vm; & vagrant winrm -c "powershell -NoProfile -ExecutionPolicy Bypass -File C:\rider-test\pair-auto.ps1" 2>&1 | Out-Null }
    $code = ((Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120) | ConvertFrom-Json).code
    Write-Host "pairing code $code"
    $pair = New-Object Rdr+Proc $Client, "pair $code --name vm", $env1
    Start-Sleep 3
    $pair.Send("y")
    $pair.P.WaitForExit(60000) | Out-Null
    Wait-File "pair.out" '"event":"paired"' 60 | Out-Null
    Check "paired with the VM by code" ($pair.P.ExitCode -eq 0)
    Wait-Job $pairJob -Timeout 60 | Out-Null
    Start-Sleep 5   # the service resumes after pairing

    Guest "run-as-user.ps1 target.ps1"
    $size = (Wait-File "screen.out" '^\d+ \d+$') -split ' '
    $vw = [int]$size[0]; $vh = [int]$size[1]
    $viewer = New-Object Rdr+Proc $Client, "connect vm --pin-stdin", $env1
    $viewer.Send($Pin)
    $end = (Get-Date).AddSeconds(40)
    do { Start-Sleep -Milliseconds 300; $win = [Rdr]::WindowOf($viewer.P.Id) } while ($win -eq [IntPtr]::Zero -and (Get-Date) -lt $end -and -not $viewer.P.HasExited)
    if ($win -eq [IntPtr]::Zero) { throw "no viewer window: $($viewer.All())" }
    [Rdr]::Place($win, 100, 100, 1280, 800)
    Start-Sleep 7
    $title = [Rdr]::Title($win)
    Write-Host "  viewer title: $title"
    Check "full access: no access label in the title" (-not ($title -like "*[[]*"))
    $before = Clicks
    [Rdr]::ClickHostPixel($win, [int]($vw / 2 - 360), 150, $vw, $vh) | Out-Null; Start-Sleep 3
    Check "full access: the click arrives" ((Clicks) -eq $before + 1)
    [Rdr]::ClickHostPixel($win, [int]($vw / 2 + 150), 17, $vw, $vh) | Out-Null; Start-Sleep 3
    Check "a click on the banner spot reaches the window under it" ((Clicks) -eq $before + 2)
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
