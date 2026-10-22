# D1 spike: builds the Rust library, generates the C# bindings and publishes the WinUI 3 app (docs/notes/uniffi-bridge.md).
# Needs: the Rust toolchain (rust-toolchain.toml), uniffi-bindgen-cs v0.11.1+v0.31.0 on PATH, the .NET 10 SDK.
# Usage: pwsh spikes/uniffi-bridge/windows/build.ps1 [-Platform x64|ARM64]
param([ValidateSet('x64', 'ARM64')] [string] $Platform = 'x64')
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$spike = Split-Path -Parent $PSScriptRoot
$rustTarget = @{ x64 = 'x86_64-pc-windows-msvc'; ARM64 = 'aarch64-pc-windows-msvc' }[$Platform]
$rid = @{ x64 = 'win-x64'; ARM64 = 'win-arm64' }[$Platform]

Push-Location $spike
try {
    rustup target add $rustTarget
    cargo build --release --locked -p bridge-spike --target $rustTarget
    $libDir = Join-Path $spike "target\$rustTarget\release"
    uniffi-bindgen-cs --library --no-format "$libDir\bridge_spike.dll" --out-dir "$PSScriptRoot\Generated"
} finally {
    Pop-Location
}

Push-Location $PSScriptRoot
try {
    dotnet --version
    dotnet publish BridgeSpike.csproj -c Release -r $rid -p:Platform=$Platform "-p:BridgeLibDir=$libDir"
    Write-Host "App: $PSScriptRoot\bin\$Platform\Release\net10.0-windows10.0.26100.0\$rid\publish\BridgeSpike.exe"
} finally {
    Pop-Location
}
