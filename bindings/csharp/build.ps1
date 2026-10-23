# Builds current-ffi and the Jellyfin stand-in for x64 Windows, generates the C# bindings and builds the class library and its tests
# (docs/notes/uniffi-bridge.md). Needs Rust (rustup), the .NET 10 SDK and uniffi-bindgen-cs:
#   cargo install --locked --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.1+v0.31.0 uniffi-bindgen-cs
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$root = Resolve-Path "$PSScriptRoot/../.."
$target = 'x86_64-pc-windows-msvc'
$lib = Join-Path $root "target/$target/release"
$generated = Join-Path $PSScriptRoot 'Current.Bindings/Generated'

Push-Location $root
try {
    rustup target add $target
    cargo build --release --locked -p current-ffi --target $target
    # The Jellyfin stand-in the adapter tests run against (tools/jellyfin-stand-in). Test code only: nothing ships it.
    cargo build --release --locked -p jellyfin-stand-in --target $target

    if (Test-Path $generated) { Remove-Item -Recurse -Force $generated }
    # --no-format: otherwise it tries `dotnet csharpier` and prints a backtrace when that is missing.
    uniffi-bindgen-cs --library --no-format (Join-Path $lib 'current_ffi.dll') --out-dir $generated

    dotnet build (Join-Path $PSScriptRoot 'Current.Bindings.Tests/Current.Bindings.Tests.csproj') -c Release "-p:CoreLibDir=$lib"
}
finally {
    Pop-Location
}
