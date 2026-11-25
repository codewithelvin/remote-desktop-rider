# Shared by the Enterprise mode tests (test/vm/enterprise*.ps1; dot-source
# it after setting $Work): the company server (test/server/server.sh), the
# admin on this PC (`rider-client company ...` with a scratch identity in
# $Work), sessions answered in the VM (guest/ent-answer.ps1), and checks.
. "$PSScriptRoot\..\local-session\lib.ps1"
. "$PSScriptRoot\vmlib.ps1"
$guest = Join-Path $PSScriptRoot "guest"
$client = Join-Path $repo "target\release\rider-client.exe"
$serverAddr = "192.168.56.1:47900"
# Git's bash (not WSL's, which `bash` may find first).
$bash = $null
for ($d = Split-Path (Get-Command git).Source; $d -and -not $bash; $d = Split-Path $d) {
    if (Test-Path "$d\bin\bash.exe") { $bash = "$d\bin\bash.exe" }
}
if (-not $bash) { throw "Git's bash.exe not found" }
$serverSh = Join-Path $repo "test\server\server.sh"
function Server([string]$arguments) {
    $out = & $bash -c "'$($serverSh -replace '\\', '/')' $arguments" 2>&1
    if ($LASTEXITCODE) { throw "server.sh $arguments failed:`n$($out -join "`n")" }
    $out
}

$results = @()
function Check([string]$what, $ok) {
    $ok = [bool]$ok
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}
function GuestOut([string]$file) { Get-Content (Join-Path $guest $file) -Encoding UTF8 -ErrorAction SilentlyContinue }
function Wait-GuestLine([string]$file, [string]$pattern, [int]$seconds) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) {
        $hit = GuestOut $file | Where-Object { $_ -match $pattern } | Select-Object -Last 1
        if ($hit) { return $hit }
        Start-Sleep -Milliseconds 400
    }
    return $null
}
$env1 = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$env1["APPDATA"] = $Work; $env1["RIDER_LANG"] = "en"
function Admin([string]$arguments, [int]$seconds = 30) {
    $p = New-Object Rdr+Proc $client, $arguments, $env1
    $p.P.WaitForExit($seconds * 1000) | Out-Null
    $p.Stop()
    Start-Sleep -Milliseconds 200
    [pscustomobject]@{ Code = $p.P.ExitCode; Text = $p.All() }
}
function Computers { try { (Admin "company list").Text | Where-Object { $_ } | ConvertFrom-Json } catch { @() } }
# Start a session; answer in the VM with $Answer ("" = nobody answers).
# -GiveUpAfter: the admin stops waiting after that many seconds.
# -Env: whose identity asks (default: the admin's).
function Try-Connect([string]$fp, [string]$Answer, [int]$GiveUpAfter = 0, $Env = $env1) {
    Remove-Item (Join-Path $guest "ent-answer.out") -ErrorAction SilentlyContinue
    if ($Answer) { Guest "run-as-user.ps1 ent-answer.ps1 $Answer" }
    $v = New-Object Rdr+Proc $client, "company-connect $fp", $Env
    $started = Get-Date
    $end = $started.AddSeconds($(if ($GiveUpAfter) { $GiveUpAfter } else { 90 }))
    while ((Get-Date) -lt $end -and -not $v.P.HasExited -and -not $v.Find("Connected to")) { Start-Sleep -Milliseconds 300 }
    $seconds = [math]::Round(((Get-Date) - $started).TotalSeconds)
    $connected = [bool]$v.Find("Connected to")
    if (-not $connected -and -not $v.P.HasExited) {
        # Giving up like the app's "Cancel request": the client closes the
        # connection properly, so the person's window goes away at once.
        $v.Send("cancel")
        $v.P.WaitForExit(5000) | Out-Null
    }
    if ($connected) {
        Start-Sleep 4
        # End it like the window's close button would (no input on this PC).
        $w = [Rdr]::WindowOf($v.P.Id)
        if ($w -ne [IntPtr]::Zero) { [Rdr]::Post($w, 0x0010, 0, [IntPtr]::Zero) }
        $v.P.WaitForExit(10000) | Out-Null
    }
    $v.Stop()
    Start-Sleep 3   # the host is free again
    [pscustomobject]@{ Connected = $connected; Text = $v.All(); Seconds = $seconds }
}

# A second device on this PC that is not the company's admin.
$envStranger = New-Object 'System.Collections.Generic.Dictionary[string,string]'
$envStranger["APPDATA"] = Join-Path $Work "stranger"; $envStranger["RIDER_LANG"] = "en"
function Stranger([string]$arguments, [int]$seconds = 30) {
    $p = New-Object Rdr+Proc $client, $arguments, $envStranger
    $p.P.WaitForExit($seconds * 1000) | Out-Null
    $p.Stop()
    [pscustomobject]@{ Code = $p.P.ExitCode; Text = $p.All() }
}

# A fresh company server with an admin signed in on this PC; the join token
# goes to guest\ent-token.txt. Returns the sign-in results (first, again).
function New-Company {
    if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
    New-Item -ItemType Directory -Force $Work | Out-Null
    Server "up" | Out-Null
    $init = Server "sh rider-server --state-dir /var/lib/rider-server company init --name TestCo"
    $code = ($init | Select-String 'Admin setup code: (\S+)').Matches[0].Groups[1].Value
    $sfp = (Server "fp" | Select-Object -Last 1).ToString().Trim()
    Write-Host "test server $serverAddr (Ubuntu, systemd), setup code $code"
    Admin "set-server $serverAddr $sfp" | Out-Null
    New-Item -ItemType Directory -Force $envStranger["APPDATA"] | Out-Null
    Stranger "set-server $serverAddr $sfp" | Out-Null
    $script:serverFp = $sfp
    $first = Admin "company sign-in $code"
    $again = Admin "company sign-in $code"
    Set-Content (Join-Path $guest "ent-token.txt") ((Admin "company token").Text.Trim()) -NoNewline
    [pscustomobject]@{ First = $first; Again = $again }
}

# The first company computer online in the admin's list (or $null).
function Wait-Online([int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    do {
        $pc = Computers | Where-Object { $_.online } | Select-Object -First 1
        if ($pc) { return $pc }
        Start-Sleep 2
    } while ((Get-Date) -lt $end)
    $null
}

function Finish([int]$expected) {
    $failed = @($script:results | Where-Object Result -eq "FAIL").Count
    if ($failed -or $script:results.Count -ne $expected) { Write-Host "FAIL: $failed of $($script:results.Count) checks"; exit 1 }
    Write-Host "PASS: all $($script:results.Count) checks"
    exit 0
}

