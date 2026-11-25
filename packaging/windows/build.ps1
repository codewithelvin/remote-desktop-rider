param(
    [string]$Version,
    [string]$Profile = "release"
)
$ErrorActionPreference = "Stop"
$root = Resolve-Path "$PSScriptRoot\..\.."
Set-Location $root

if (-not $Version) {
    $m = Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    $Version = $m.Matches[0].Groups[1].Value
}

cargo build --profile $Profile -p rider-app -p rider-client -p rider-host
if ($LASTEXITCODE) { throw "cargo build failed" }

$bin = Join-Path $root "target\$Profile"
$icon = Get-ChildItem "$bin\build\rider-app-*\out\app.ico" -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1

$iscc = @(
    (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source,
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }

$out = Join-Path $root "target\installer"
$defs = @("/DAppVersion=$Version", "/DBinDir=$bin", "/DOutDir=$out")
if ($icon) { $defs += "/DIconFile=$($icon.FullName)" }
& $iscc /Q @defs "$PSScriptRoot\RemoteDesktopRider.iss"
if ($LASTEXITCODE) { throw "Inno Setup failed" }
Write-Host "$out\rdr-setup-$Version.exe"
