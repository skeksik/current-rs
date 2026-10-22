# Case 1 harness: runs the prototype on each file, lets it measure itself for 60 s (mpv drop counters, process CPU),
# and samples per-process GPU engine utilisation (3D, VideoDecode) for the app and for dwm.exe meanwhile.
# Usage: pwsh measure.ps1 -Exe <VideoSpike.exe> -Media <dir> [-FullScreen] [-Size 1600x1000] [-Pos 100,100] [-Files a.mkv,b.mkv]
param(
    [Parameter(Mandatory)] [string] $Exe,
    [Parameter(Mandatory)] [string] $Media,
    [string] $Out = 'C:\crs2-data\logs',
    [switch] $FullScreen,
    [string] $Size = '1600x1000',
    [string] $Pos = '100,100',
    [string] $Colour = 'auto',
    [string[]] $Files = @('sdr-1080p60.mkv', 'hdr10-1080p60.mkv', 'sdr-2160p60.mkv', 'hdr10-2160p60.mkv'),
    [int] $Seconds = 60
)
$ErrorActionPreference = 'Stop'
$results = @()
foreach ($f in $Files) {
    $args = @('--file', (Join-Path $Media $f), '--measure', $Seconds, '--exit', '--size', $Size, '--pos', $Pos, '--colour', $Colour, '--data', $Out)
    if ($FullScreen) { $args += '--fullscreen' }
    $before = Get-ChildItem $Out -Filter 'measure-*.json' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName
    $p = Start-Process $Exe -ArgumentList $args -PassThru
    Start-Sleep -Seconds 5 # load + the app's own 3 s warm-up, roughly aligned with its window
    $dwm = (Get-Process dwm).Id
    $samples = @()
    for ($i = 0; $i -lt $Seconds - 2 -and -not $p.HasExited; $i++) {
        $c = (Get-Counter "\GPU Engine(pid_$($p.Id)_*)\Utilization Percentage", "\GPU Engine(pid_$($dwm)_*)\Utilization Percentage" -ErrorAction SilentlyContinue).CounterSamples
        $row = @{}
        foreach ($s in $c) {
            $who = if ($s.InstanceName -like "pid_$($p.Id)_*") { 'app' } else { 'dwm' }
            $eng = ($s.InstanceName -split 'engtype_')[1]
            $key = "$who-$eng"
            $row[$key] = ($row[$key] ?? 0) + $s.CookedValue
        }
        $samples += $row
    }
    $p.WaitForExit(120000) | Out-Null
    $json = Get-ChildItem $Out -Filter 'measure-*.json' | Where-Object { $before -notcontains $_.FullName } | Sort-Object LastWriteTime | Select-Object -Last 1
    $m = Get-Content $json.FullName -Raw | ConvertFrom-Json
    $gpu = [ordered]@{}
    foreach ($k in ($samples | ForEach-Object { $_.Keys } | Sort-Object -Unique)) {
        $vals = $samples | ForEach-Object { $_[$k] ?? 0 }
        $gpu[$k] = '{0:0.0} avg / {1:0.0} max' -f ($vals | Measure-Object -Average).Average, ($vals | Measure-Object -Maximum).Maximum
    }
    $r = [ordered]@{
        file = $f; fullscreen = [bool]$FullScreen; composition = $m.composition_size; hwdec = $m.hwdec
        played_s = $m.played_seconds; vo_drops = $m.'frame-drop-count'; decoder_drops = $m.'decoder-frame-drop-count'
        delayed = $m.'vo-delayed-frame-count'; mistimed = $m.'mistimed-frame-count'; cpu_pct = $m.cpu_percent_of_machine
        gpu = $gpu; swap_chain = $m.swap_chain; target = $m.video_target_params
    }
    $results += [pscustomobject]$r
    $r | ConvertTo-Json -Depth 4 | Write-Output
}
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$results | ConvertTo-Json -Depth 4 | Set-Content (Join-Path $Out "case1-$stamp.json")
