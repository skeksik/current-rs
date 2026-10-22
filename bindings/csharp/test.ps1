# Runs the adapter tests that build.ps1 built. The runner exits non-zero on any failure and prints "<n> passed"; this script also
# fails if that line is missing or n is 0, so the tests cannot pass by not running.
$ErrorActionPreference = 'Stop'

$output = dotnet run --no-build -c Release --project (Join-Path $PSScriptRoot 'Current.Bindings.Tests/Current.Bindings.Tests.csproj')
$code = $LASTEXITCODE
$output | ForEach-Object { Write-Host $_ }
if ($code -ne 0) { throw "adapter tests failed (exit code $code)" }
$summary = $output | Select-String -Pattern '^(\d+) passed, 0 failed$' | Select-Object -Last 1
if (-not $summary -or [int]$summary.Matches[0].Groups[1].Value -eq 0) { throw 'adapter tests did not report any passing test' }
