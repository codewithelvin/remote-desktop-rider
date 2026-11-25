# Chat in a paired session: the Windows test VM's viewer pairs with and
# connects to the VM's own unattended service (through the real rendezvous
# server), so the chat window the service opens for the signed-in user and
# the viewer are both in the VM, where all typing happens (guest/paired-chat.ps1).
# This PC only starts things and reads what the VM reports. Checks: the
# window runs as the user and waits minimized, messages both ways, it comes
# up for a message without taking the keyboard, files both ways (the
# viewer's kept in Downloads\Rider), and it closes when the session ends.
#
#   cargo build --release --workspace
#   cd test/vm && vagrant up
#   powershell -ExecutionPolicy Bypass -File test\vm\paired-chat.ps1 -Server rdm.example.com -Fingerprint <hex>
#
# Exit code 0: every check passed.
param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Pin = "rider-test-pin"
)
$guest = Join-Path $PSScriptRoot "guest"
. "$PSScriptRoot\vmlib.ps1"   # Guest: run a guest/ script in the VM
$outFile = Join-Path $guest "paired-chat.out"
function Wait-File([string]$file, [string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = Get-Content (Join-Path $guest $file) -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 500
    }
    return $null
}
function Seen([string]$prefix) {
    Get-Content $outFile -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ -like "$prefix*" } | Select-Object -Last 1
}
$results = @()
function Check([string]$what, $ok) {
    $ok = [bool]$ok   # a -match on nothing gives an empty array
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}

$pairJob = $null
try {
    Guest "update-host.ps1"
    Remove-Item (Join-Path $guest "pair.out"), $outFile, (Join-Path $guest "paired-chat.args") -ErrorAction SilentlyContinue
    # The user-side script first: it waits for the args, and the VM's helper
    # runs one script at a time (pair-auto.ps1 holds it until paired).
    Guest "run-as-user.ps1 paired-chat.ps1"
    $pairJob = Guest-Start "pair-auto.ps1"
    $line = Wait-File "pair.out" '"code":"[A-Z0-9]{3}-[A-Z0-9]{3}"' 120
    if (-not $line) { throw "no pairing code from the VM" }
    $code = ($line | ConvertFrom-Json).code
    Write-Host "pairing code $code"
    Set-Content (Join-Path $guest "paired-chat.args") "$Server $Fingerprint $code $Pin" -NoNewline
    if (-not (Wait-File "paired-chat.out" '^done' 300)) { throw "the VM's script did not finish" }
    Get-Content $outFile -Encoding UTF8 | ForEach-Object { Write-Host "  vm: $_" }

    Check "the VM's viewer paired with its service" ((Seen "paired") -eq "paired 0")
    Check "the viewer connected" ($null -ne (Seen "window"))
    $panel = Seen "panel "
    Check "the chat window runs as the signed-in user, minimized ('$panel')" ($panel -eq "panel vagrant True")
    Check "a message from the chat window reaches the viewer" ((Seen "title") -like "*Salam from the host panel*")
    Check "a message from the viewer brings the window up, keys stay with the viewer ('$(Seen "panel-up")')" ((Seen "panel-up") -eq "panel-up False True")
    $saved = Seen "saved"
    Check "a file from the viewer is kept in Downloads\Rider, owned by the user ('$saved')" ($saved -match '^saved pv-\w+\.txt 20 \S*vagrant$')
    Check "a file from the chat window reaches the viewer ('$(Seen "got")')" ((Seen "got") -match '^got ph-\w+\.txt 18$')
    $gone = Seen "panel-gone"
    Check "the chat window closes when the session ends ('$gone')" ($gone -match '^panel-gone \d+$')
    Check "no errors in the VM" ($null -eq (Seen "error"))
} finally {
    if ($pairJob) { Remove-Job $pairJob -Force -ErrorAction SilentlyContinue }
}
$failed = @($results | Where-Object Result -eq "FAIL").Count
if ($failed -or $results.Count -ne 9) { Write-Host "FAIL: $failed of $($results.Count) checks"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
exit 0
