# Runs the adapter tests that build.ps1 built. The runner exits non-zero on any failure and prints "<n> passed"; this script also
# fails if that line is missing or n is 0, so the tests cannot pass by not running.
# The tests start the Jellyfin stand-in that build.ps1 built; it exits when the runner does (its stdin closes), so a failing or
# crashing run leaves nothing behind.
$ErrorActionPreference = 'Stop'

$standIn = Resolve-Path "$PSScriptRoot/../../target/x86_64-pc-windows-msvc/release/jellyfin-stand-in.exe"
$env:CURRENT_STAND_IN = $standIn.Path

try {
    $output = dotnet run --no-build -c Release --project (Join-Path $PSScriptRoot 'Current.Bindings.Tests/Current.Bindings.Tests.csproj')
    $code = $LASTEXITCODE
}
finally {
    # A backstop: a stand-in still running here would keep the CI step from ending.
    Get-Process -Name 'jellyfin-stand-in' -ErrorAction SilentlyContinue | Stop-Process -Force
}
$output | ForEach-Object { Write-Host $_ }
if ($code -ne 0) { throw "adapter tests failed (exit code $code)" }
$summary = $output | Select-String -Pattern '^(\d+) passed, 0 failed$' | Select-Object -Last 1
if (-not $summary -or [int]$summary.Matches[0].Groups[1].Value -eq 0) { throw 'adapter tests did not report any passing test' }
