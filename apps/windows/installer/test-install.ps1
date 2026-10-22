# Install, upgrade and uninstall test for the installer (docs/notes/windows-shell.md). Installs -Old, checks the app and its shortcut
# and runs the smoke run; writes a marker file into the data folder; installs -New over it and checks again; uninstalls and checks that
# the program folder and shortcut are gone and the marker is still there. Runs as the current user, silently. CI runs it on a clean
# runner; on a PC with Current installed it would upgrade and then remove that install (the data folder stays). Before the final
# uninstall it starts the app and uninstalls once with it running, which must either stop with nothing removed or remove everything.
param(
    [Parameter(Mandatory)] [string]$Old,
    [Parameter(Mandatory)] [string]$OldVersion,
    [Parameter(Mandatory)] [string]$New,
    [Parameter(Mandatory)] [string]$NewVersion
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$programDir = Join-Path $env:LOCALAPPDATA 'Programs\Current'
$exe = Join-Path $programDir 'Current.exe'
$dataDir = Join-Path $env:LOCALAPPDATA 'Current'
$marker = Join-Path $dataDir 'install-test-marker.txt'
$shortcutDir = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
$shortcut = Join-Path $shortcutDir 'Current.lnk'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{21A6CF25-84E6-463B-84C5-304469A6490C}_is1'
$smoke = Join-Path $PSScriptRoot '../smoke.ps1'

function Assert([bool]$condition, [string]$what) {
    if (-not $condition) { throw "FAIL: $what" }
    Write-Host "ok   $what"
}

function Install([string]$setup) {
    $log = Join-Path $env:RUNNER_TEMP "$(Split-Path -Leaf $setup).log"
    $p = Start-Process -FilePath $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"" -Wait -PassThru
    if ($p.ExitCode -ne 0) {
        if (Test-Path $log) { Get-Content $log | Write-Host }
        throw "$setup exited with $($p.ExitCode)"
    }
}

function Check-Installed([string]$version) {
    Assert (Test-Path $exe) "Current.exe is in $programDir"
    Assert ((Get-ItemProperty $uninstallKey).DisplayVersion -eq $version) "Apps lists version $version"
    Assert (Test-Path $shortcut) 'the Start menu shortcut exists'
    $folder = (New-Object -ComObject Shell.Application).NameSpace($shortcutDir)
    $id = $folder.ParseName('Current.lnk').ExtendedProperty('System.AppUserModel.ID')
    Assert ($id -eq 'Skeksik.Current') "the shortcut's AppUserModelID is '$id'"
    & $smoke -Exe $exe
    Write-Host 'ok   the installed app passes the smoke run'
}

Assert (-not (Test-Path $programDir)) 'no earlier install'

Install $Old
Check-Installed $OldVersion

New-Item -ItemType Directory -Force $dataDir | Out-Null
Set-Content -Path $marker -Value 'written by test-install.ps1'

Install $New
Check-Installed $NewVersion
Assert (Test-Path $marker) 'the upgrade kept the data folder'

$uninstaller = (Get-ItemProperty $uninstallKey).UninstallString.Trim('"')

# Uninstall while the app runs. Either outcome is acceptable, a partial removal is not: Inno detects the app through its AppMutex
# and stops with everything in place, or it closes the app and removes everything. Report which one it is.
$before = (Get-ChildItem $programDir -Recurse -File).Count
$app = Start-Process -FilePath $exe -PassThru
Start-Sleep -Seconds 5
Assert (-not $app.HasExited) 'the app is running'
$log = Join-Path $env:RUNNER_TEMP 'uninstall-while-running.log'
$u = Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=`"$log`"" -PassThru
# A message box that /SUPPRESSMSGBOXES does not answer would hang here: fail instead.
if (-not $u.WaitForExit(120000)) { $u.Kill(); throw 'FAIL: the uninstaller did not exit within 120 s with the app running' }
# The uninstaller can hand over to a copy of itself and return early: wait until the folder stops changing.
$last = -1; $stable = 0; $deadline = (Get-Date).AddSeconds(120)
while ($stable -lt 10) {
    if ((Get-Date) -gt $deadline) { throw 'FAIL: the uninstall with the app running did not settle within 120 s' }
    $now = if (Test-Path $programDir) { (Get-ChildItem $programDir -Recurse -File).Count } else { 0 }
    if ($now -eq $last) { $stable++ } else { $stable = 0; $last = $now }
    Start-Sleep -Milliseconds 500
}
if (Test-Path $log) { Get-Content $log | Write-Host }
$appRunning = -not $app.HasExited
if ($last -eq $before -and (Test-Path $uninstallKey)) {
    Assert $appRunning 'the app is still running'
    Write-Host 'ok   with the app running, uninstall stopped and removed nothing (AppMutex)'
    Stop-Process -Id $app.Id -Force
    $app.WaitForExit()
}
elseif ($last -eq 0 -and -not (Test-Path $uninstallKey)) {
    Assert (-not $appRunning) 'the app was closed'
    Write-Host 'ok   with the app running, uninstall closed it and removed everything'
}
else {
    throw "FAIL: partial removal with the app running: $last of $before files left, Apps entry present: $(Test-Path $uninstallKey)"
}

if (Test-Path $uninstallKey) {
    Start-Process -FilePath $uninstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait
}
# The uninstaller runs from a copy in the temp folder and can return before it has finished: wait for it.
$deadline = (Get-Date).AddSeconds(120)
while ((Test-Path $exe) -or (Test-Path $uninstallKey)) {
    if ((Get-Date) -gt $deadline) { throw 'FAIL: uninstall did not finish within 120 s' }
    Start-Sleep -Milliseconds 500
}
Assert (-not (Test-Path $programDir)) 'uninstall removed the program folder'
Assert (-not (Test-Path $shortcut)) 'uninstall removed the shortcut'
Assert (Test-Path $marker) 'uninstall kept the data folder'
Write-Host 'install test passed'
