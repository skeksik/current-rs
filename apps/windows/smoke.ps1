# Runs `Current.exe --smoke` (Current.Shell/SmokeRun.cs) and fails unless it prints "smoke run passed" and exits 0 in time.
param(
    [Parameter(Mandatory)] [string]$Exe,
    [int]$TimeoutSeconds = 120
)
$ErrorActionPreference = 'Stop'

if (-not (Test-Path $Exe)) { throw "$Exe does not exist" }
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
