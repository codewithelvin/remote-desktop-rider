param(
    [Parameter(Mandatory)][string]$Server,
    [Parameter(Mandatory)][string]$Fingerprint,
    [string]$Work = (Join-Path $env:TEMP "rdr-vm-chat")
)
. "$PSScriptRoot\..\local-session\lib.ps1"
$guest = Join-Path $PSScriptRoot "guest"
$vm = $PSScriptRoot
$hostExe = Join-Path $repo "target\release\rider-host.exe"

. "$PSScriptRoot\vmlib.ps1"
function Seen([string]$pattern) {
    Get-Content (Join-Path $guest "chat-viewer.out") -Encoding UTF8 -ErrorAction SilentlyContinue | Where-Object { $_ -like $pattern } | Select-Object -Last 1
}
function Wait-Seen([string]$pattern, [int]$seconds = 60) {
    $end = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $end) { $hit = Seen $pattern; if ($hit) { return $hit }; Start-Sleep -Milliseconds 500 }
    return $null
}
function Try-Wait($proc, $text, $seconds) { try { Wait-For $proc $text $seconds } catch { $null } }

$results = @()
function Check([string]$what, [bool]$ok) {
    $script:results += [pscustomobject]@{ Check = $what; Result = $(if ($ok) { "PASS" } else { "FAIL" }) }
    Write-Host ("  {0}  {1}" -f $(if ($ok) { "PASS" } else { "FAIL" }), $what)
}

$hostProc = $null
try {
    if (Test-Path $Work) { Remove-Item -Recurse -Force $Work }
    New-Item -ItemType Directory -Force "$Work\host-pd", "$Work\host-ad" | Out-Null
    $hostEnv = New-Object 'System.Collections.Generic.Dictionary[string,string]'
    $hostEnv["ProgramData"] = "$Work\host-pd"; $hostEnv["APPDATA"] = "$Work\host-ad"; $hostEnv["RIDER_LANG"] = "en"
    $hostProc = New-Object Rdr+Proc $hostExe, "--json support $Server $Fingerprint", $hostEnv
    $code = ((Wait-For $hostProc '"registered"' 30) | ConvertFrom-Json).code
    Write-Host "one-time code $code"
    Set-Content (Join-Path $guest "chat-viewer.args") "$Server $Fingerprint $code" -NoNewline
    Guest "start-chat-viewer.ps1"
    Wait-For $hostProc '"support_request"' 90 | Out-Null
    $hostProc.Send("y")
    Wait-For $hostProc '"support_started"' 30 | Out-Null
    Check "the VM's viewer connected with the code" ($null -ne (Wait-Seen "window" 30))

    $bs = [char]92
    $hostProc.Send('chat "Salam ' + $bs + 'u0259 from the host"')
    $title = Wait-Seen "title *from the host*" 20
    Check "a message from this PC shows in the VM's viewer ('$title')" ($title -like "*Salam $([char]0x259) from the host*")

    $msg = Try-Wait $hostProc '"event":"chat"' 60
    $text = if ($msg) { ($msg | ConvertFrom-Json).text } else { "" }
    Check "a message typed in the viewer's chat arrives ('$text')" ($text -eq "Hello from the viewer")
    $in = Try-Wait $hostProc '"path":' 60
    $kept = if ($in) { ($in | ConvertFrom-Json).path } else { "" }
    $sentFile = Join-Path $guest "from-viewer.txt"
    Check "a file attached in the viewer arrives on this PC" ($kept -and (Test-Path $kept) -and ((Get-Content $kept -Raw) -eq (Get-Content $sentFile -Raw)))

    $src = Join-Path $Work "from-host.txt"
    Set-Content -Path $src -Value ("host file " + [guid]::NewGuid()) -NoNewline
    $hostProc.Send("send-file " + (ConvertTo-Json $src -Compress))
    $got = Wait-Seen "got from-host.txt*" 30
    $copy = Join-Path $guest "received-from-host.txt"
    Check "a file sent from this PC arrives in the VM's viewer" ($got -and (Test-Path $copy) -and ((Get-Content $copy -Raw) -eq (Get-Content $src -Raw)))
    Check "it is marked as from the internet there" ($got -like "*marked")

    $hostProc.Send("access standard")
    Wait-For $hostProc '"access":"standard"' 10 | Out-Null
    $hostProc.Send("send-file " + (ConvertTo-Json $src -Compress))
    Check "Standard access: files are refused" ($null -ne (Try-Wait $hostProc '"event":"file_failed"' 15))

    $hostProc.Send("quit")
    Check "the viewer closed when the session ended" ($null -ne (Wait-Seen "exit" 30))
    Check "the viewer's received file is deleted" ($null -ne (Wait-Seen "gone from-host.txt" 15))
    Start-Sleep 2
    Check "this PC's received file is deleted" (-not $kept -or -not (Test-Path $kept))
} finally {
    if ($hostProc) { $hostProc.Stop() }
    Remove-Item (Join-Path $guest "chat-viewer.args"), (Join-Path $guest "received-*"), (Join-Path $guest "from-viewer.txt") -ErrorAction SilentlyContinue
}

$failed = @($results | Where-Object Result -eq "FAIL").Count
Write-Host ""
$results | Format-Table -AutoSize | Out-String | Write-Host
if ($failed) { Write-Host "FAIL: $failed check(s)"; exit 1 }
Write-Host "PASS: all $($results.Count) checks"
