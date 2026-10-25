# Builds the installer from the published app (apps/windows/build.ps1) into apps/windows/out/installer. Needs Inno Setup 7's ISCC.exe:
# pass -Iscc, or have it on PATH.
param(
    # The installer's version; defaults to the workspace version in Cargo.toml.
    [string]$Version,
    [string]$Iscc = 'ISCC.exe'
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$root = Resolve-Path "$PSScriptRoot/../../.."
$app = Join-Path $PSScriptRoot '../out/app'
$out = Join-Path $PSScriptRoot '../out/installer'

if (-not $Version) {
    $match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $match) { throw 'no workspace version in Cargo.toml' }
    $Version = $match.Matches[0].Groups[1].Value
}
if (-not (Test-Path (Join-Path $app 'Current.exe'))) { throw 'the published app is missing: run apps/windows/build.ps1' }
# The player's libmpv DLLs and their licence texts ship with the app (ADR 0019 task 5, ADR 0003).
if (-not (Test-Path (Join-Path $app 'mpv-2.dll'))) { throw 'the published app has no libmpv DLLs: run apps/windows/build.ps1 with -Mpv' }
if (-not (Test-Path (Join-Path $app 'licences/libmpv/components.md'))) { throw 'the published app has no libmpv licence texts' }

$isccPath = (Get-Command $Iscc).Source
$innoLicense = Join-Path (Split-Path $isccPath) 'license.txt'
if (-not (Test-Path $innoLicense)) { throw "Inno Setup's license.txt is not next to $isccPath" }

New-Item -ItemType Directory -Force $out | Out-Null
& $isccPath "/DAppVersion=$Version" "/DInnoLicense=$innoLicense" "/DSourceDir=$(Resolve-Path $app)" "/DOutputDir=$(Resolve-Path $out)" (Join-Path $PSScriptRoot 'Current.iss')
$setup = Join-Path (Resolve-Path $out) "Current-$Version-x64-setup.exe"
if (-not (Test-Path $setup)) { throw "ISCC did not write $setup" }
Write-Host "Built $setup"
