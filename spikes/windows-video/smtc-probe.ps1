# Case 6 probe (Windows PowerShell 5.1, which still projects WinRT): reads the system media sessions the way the media
# flyout does, and can toggle play/pause through the same path. Usage: powershell -File smtc-probe.ps1 [-Toggle]
param([switch] $Toggle, [double] $SeekTo = -1)
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object {
    $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } |
    Select-Object -First 1
function Await($op, [Type] $t) { $task = $asTask.MakeGenericMethod($t).Invoke($null, @($op)); $task.Wait(5000) | Out-Null; $task.Result }
$null = [Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager, Windows.Media.Control, ContentType = WindowsRuntime]
$mgr = Await ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager]::RequestAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager])
foreach ($s in $mgr.GetSessions()) {
    if ($s.SourceAppUserModelId -notmatch 'VideoSpike') { continue }
    $p = Await ($s.TryGetMediaPropertiesAsync()) ([Windows.Media.Control.GlobalSystemMediaTransportControlsSessionMediaProperties])
    $i = $s.GetPlaybackInfo(); $t = $s.GetTimelineProperties()
    [pscustomobject]@{
        App = $s.SourceAppUserModelId; Title = $p.Title; Subtitle = $p.Subtitle; Type = $p.PlaybackType
        HasArtwork = $null -ne $p.Thumbnail; Status = $i.PlaybackStatus
        PlayPause = $i.Controls.IsPlayPauseToggleEnabled; Seek = $i.Controls.IsPlaybackPositionEnabled
        Position = $t.Position; End = $t.EndTime; Updated = $t.LastUpdatedTime
    } | Format-List
    if ($SeekTo -ge 0) {
        Await ($s.TryChangePlaybackPositionAsync([TimeSpan]::FromSeconds($SeekTo).Ticks)) ([bool]) | Out-Null
        Start-Sleep -Milliseconds 800; "after seek: $($s.GetTimelineProperties().Position)"
    }
    if ($Toggle) { Await ($s.TryTogglePlayPauseAsync()) ([bool]) | Out-Null; Start-Sleep -Milliseconds 500; "after toggle: $($s.GetPlaybackInfo().PlaybackStatus)" }
}
