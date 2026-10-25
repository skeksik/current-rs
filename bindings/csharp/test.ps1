# Runs the adapter tests that build.ps1 built. The runner exits non-zero on any failure and prints "<n> passed"; this script also
# fails if that line is missing or n is 0, so the tests cannot pass by not running.
# The tests start the Jellyfin stand-in that build.ps1 built; it exits when the runner does (its stdin closes), so a failing or
# crashing run leaves nothing behind.
$ErrorActionPreference = 'Stop'

$standIn = Resolve-Path "$PSScriptRoot/../../target/x86_64-pc-windows-msvc/release/jellyfin-stand-in.exe"

# The runner must load the core built with the test engine (build.ps1): its library holds the engine's marker
# (crates/app/src/player/test_engine.rs). apps/windows/build.ps1 checks that the published app's does not.
$runnerCores = @(Get-ChildItem (Join-Path $PSScriptRoot 'Current.Bindings.Tests/bin/Release') -Recurse -Filter 'current_ffi.dll')
if ($runnerCores.Count -eq 0) { throw 'the test runner has no current_ffi.dll: run build.ps1' }
foreach ($core in $runnerCores) {
    $text = [System.Text.Encoding]::Latin1.GetString([System.IO.File]::ReadAllBytes($core.FullName))
    if (-not $text.Contains('current-test-engine')) { throw "$($core.FullName) is not the test-engine core" }
}
$env:CURRENT_STAND_IN = $standIn.Path

# Stand-ins already running belong to someone else: only those this run started are stopped below.
$before = @(Get-Process -Name 'jellyfin-stand-in' -ErrorAction SilentlyContinue | ForEach-Object { $_.Id })
try {
    $output = dotnet run --no-build -c Release --project (Join-Path $PSScriptRoot 'Current.Bindings.Tests/Current.Bindings.Tests.csproj')
    $code = $LASTEXITCODE
}
finally {
    # A backstop: a stand-in of this run still running here would keep the CI step from ending.
    Get-Process -Name 'jellyfin-stand-in' -ErrorAction SilentlyContinue |
        Where-Object { $before -notcontains $_.Id -and $_.Path -eq $standIn.Path } |
        Stop-Process -Force
}
$output | ForEach-Object { Write-Host $_ }
if ($code -ne 0) { throw "adapter tests failed (exit code $code)" }
$summary = $output | Select-String -Pattern '^(\d+) passed, 0 failed$' | Select-Object -Last 1
if (-not $summary -or [int]$summary.Matches[0].Groups[1].Value -eq 0) { throw 'adapter tests did not report any passing test' }
