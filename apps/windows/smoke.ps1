# Runs `Current.exe --smoke` (Current.Shell/SmokeRun.cs) and fails unless it prints "smoke run passed" and exits 0 in time.
# -StandIn names the built Jellyfin stand-in (bindings/csharp/build.ps1 builds it): the run then also opens the sign-in window on its
# portal (ADR 0017). Without it that check is skipped, and the run says so.
param(
    [Parameter(Mandatory)] [string]$Exe,
    [string]$StandIn,
    [int]$TimeoutSeconds = 120
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Exe)) { throw "$Exe does not exist" }
if ($StandIn) {
    if (-not (Test-Path $StandIn)) { throw "$StandIn does not exist" }
    $env:CURRENT_STAND_IN = (Resolve-Path $StandIn).Path
}
$log = New-TemporaryFile
# Current.exe is a GUI executable, so it is started as a process with its standard output sent to a file.
$process = Start-Process -FilePath $Exe -ArgumentList '--smoke' -RedirectStandardOutput $log -PassThru -NoNewWindow
# Read the handle now, or ExitCode stays empty after the process ends.
$null = $process.Handle
if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
    $process.Kill()
    Get-Content $log | ForEach-Object { Write-Host $_ }
    throw "the smoke run did not exit within $TimeoutSeconds s"
}
$output = Get-Content $log
$output | ForEach-Object { Write-Host $_ }
if ($process.ExitCode -ne 0) { throw "the smoke run failed (exit code $($process.ExitCode))" }
if (-not ($output -contains 'smoke run passed')) { throw 'the smoke run did not report success' }
