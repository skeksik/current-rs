# Publishes the Windows app self-contained for x64 into apps/windows/out/app (docs/notes/windows-shell.md). Run
# bindings/csharp/build.ps1 first: it builds current_ffi.dll and generates the C# bindings this app compiles against.
param(
    # The app's version; defaults to the workspace version in Cargo.toml.
    [string]$Version
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$root = Resolve-Path "$PSScriptRoot/../.."
$lib = Join-Path $root 'target/x86_64-pc-windows-msvc/release'
$out = Join-Path $PSScriptRoot 'out/app'

if (-not $Version) {
    $match = Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version = "([^"]+)"' | Select-Object -First 1
    if (-not $match) { throw 'no workspace version in Cargo.toml' }
    $Version = $match.Matches[0].Groups[1].Value
}
if (-not (Test-Path (Join-Path $lib 'current_ffi.dll'))) { throw "current_ffi.dll is missing in ${lib}: run bindings/csharp/build.ps1" }

if (Test-Path $out) { Remove-Item -Recurse -Force $out }
# From this folder, so that its global.json picks the SDK.
Push-Location $PSScriptRoot
try {
    dotnet publish 'Current.Shell/Current.Shell.csproj' -c Release -r win-x64 -p:Platform=x64 `
        "-p:CoreLibDir=$lib" "-p:Version=$Version" -o $out
}
finally {
    Pop-Location
}
Write-Host "Published Current $Version to $out"
