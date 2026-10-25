using System.Diagnostics;
using Current.Bindings;
using Microsoft.UI.Dispatching;
using Windows.Media;
using Windows.Storage.Streams;

namespace Current.Shell.Player;

/// <summary>
/// The media keys and Windows' media flyout for the player (ADR 0007 item 3, ADR 0019 "Media keys, the media flyout and full screen"):
/// <c>SystemMediaTransportControlsInterop.GetForWindow</c> for the main window ("Use WinRT COM interop classes in .NET",
/// learn.microsoft.com, updated 2026-09-10, read 2026-10-25: (Windows.Media) SystemMediaTransportControlsInterop, ADR 0007's source),
/// filled from the core's NowPlaying: title and subtitle, artwork, video type (what keeps the screen
/// saver off, ADR 0019), status, enabled buttons, and the timeline whenever its revision changes (after every seek, so the flyout's
/// position is never stale; the spike's case 6). Button presses and position requests become player actions on the UI thread.
/// </summary>
internal sealed class MediaControls : IDisposable
{
    private readonly SystemMediaTransportControls _controls;
    private readonly DispatcherQueue _ui;
    private readonly PlayerSession _player;
    private readonly Action _stop;
    private ulong _timelineRevision = ulong.MaxValue;
    private (ItemRef? Item, string? Title, int ArtworkLength) _shown;
    private bool _disposed;

    public MediaControls(IntPtr window, DispatcherQueue ui, PlayerSession player, Action stop)
    {
        _ui = ui;
        _player = player;
        _stop = stop;
        _controls = SystemMediaTransportControlsInterop.GetForWindow(window);
        _controls.IsEnabled = true;
        _controls.PlaybackStatus = MediaPlaybackStatus.Changing;
        _controls.ButtonPressed += OnButtonPressed;
        _controls.PlaybackPositionChangeRequested += OnPositionRequested;
    }

    /// <summary>Copies <paramref name="now"/> into the controls. UI thread.</summary>
    public void Show(NowPlaying now)
    {
        if (_disposed)
        {
            return;
        }
        _controls.IsPlayEnabled = now.CanPlay;
        _controls.IsPauseEnabled = now.CanPause;
        _controls.IsStopEnabled = now.CanStop;
        _controls.IsNextEnabled = now.CanNext;
        _controls.IsPreviousEnabled = now.CanPrevious;
        _controls.PlaybackStatus = now.Status switch
        {
            MediaStatusKind.Playing => MediaPlaybackStatus.Playing,
            MediaStatusKind.Paused => MediaPlaybackStatus.Paused,
            MediaStatusKind.Changing => MediaPlaybackStatus.Changing,
            _ => MediaPlaybackStatus.Stopped,
        };
        var shown = (now.Item, now.Title, now.Artwork?.Length ?? 0);
        if (shown != _shown)
        {
            _shown = shown;
            _ = ShowDisplayAsync(now, shown);
        }
        if (now.TimelineRevision != _timelineRevision)
        {
            _timelineRevision = now.TimelineRevision;
            var end = now.Duration ?? TimeSpan.Zero;
            _controls.UpdateTimelineProperties(new SystemMediaTransportControlsTimelineProperties
            {
                StartTime = TimeSpan.Zero,
                EndTime = end,
                MinSeekTime = TimeSpan.Zero,
                MaxSeekTime = now.CanSeek ? now.SeekableEnd ?? end : TimeSpan.Zero,
                Position = now.Position,
            });
        }
    }

    /// <summary>The title, subtitle and artwork; dropped when newer ones (<paramref name="shown"/> no longer current) or the close
    /// came while the artwork was written.</summary>
    private async Task ShowDisplayAsync(NowPlaying now, (ItemRef?, string?, int) shown)
    {
        try
        {
            RandomAccessStreamReference? thumbnail = null;
            if (now.Artwork is { Length: > 0 } artwork)
            {
                var stream = new InMemoryRandomAccessStream();
                using (var writer = new DataWriter(stream.GetOutputStreamAt(0)))
                {
                    writer.WriteBytes(artwork);
                    await writer.StoreAsync();
                    writer.DetachStream();
                }
                thumbnail = RandomAccessStreamReference.CreateFromStream(stream);
            }
            if (_disposed || shown != _shown)
            {
                return;
            }
            var display = _controls.DisplayUpdater;
            display.ClearAll();
            display.Type = MediaPlaybackType.Video;
            display.VideoProperties.Title = now.Title ?? "";
            display.VideoProperties.Subtitle = now.Subtitle ?? "";
            display.Thumbnail = thumbnail;
            display.Update();
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the media controls' display failed: {error.GetType().Name}");
        }
    }

    // On a system thread: queued to the UI thread, where the player's actions are called.
    private void OnButtonPressed(SystemMediaTransportControls sender, SystemMediaTransportControlsButtonPressedEventArgs args)
    {
        var button = args.Button;
        _ui.TryEnqueue(() => Act(() =>
        {
            switch (button)
            {
                case SystemMediaTransportControlsButton.Play:
                    _player.Resume();
                    break;
                case SystemMediaTransportControlsButton.Pause:
                    _player.Pause();
                    break;
                case SystemMediaTransportControlsButton.Stop:
                    _stop();
                    break;
                case SystemMediaTransportControlsButton.Next:
                    _ = PlayNextAsync();
                    break;
                case SystemMediaTransportControlsButton.Previous:
                    // Previous restarts the item (ADR 0019, "NowPlaying").
                    _player.SeekTo(TimeSpan.Zero);
                    break;
            }
        }));
    }

    private void OnPositionRequested(SystemMediaTransportControls sender, PlaybackPositionChangeRequestedEventArgs args)
    {
        var position = args.RequestedPlaybackPosition;
        _ui.TryEnqueue(() => Act(() => _player.SeekTo(position)));
    }

    private async Task PlayNextAsync()
    {
        try
        {
            await _player.PlayNextAsync();
        }
        catch (Exception error) when (error is PlayerException or OperationCanceledException)
        {
            // The state shows any problem; a replaced or closed player has nothing to say here.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: the next episode from the media keys failed: {error.GetType().Name}");
        }
    }

    private void Act(Action action)
    {
        if (_disposed)
        {
            return;
        }
        try
        {
            action();
        }
        catch (PlayerException)
        {
            // Closed meanwhile.
        }
        catch (Exception error)
        {
            Trace.TraceError($"Current: a media key action failed: {error.GetType().Name}");
        }
    }

    /// <summary>Off: the flyout drops the session and the media keys go elsewhere.</summary>
    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }
        _disposed = true;
        _controls.ButtonPressed -= OnButtonPressed;
        _controls.PlaybackPositionChangeRequested -= OnPositionRequested;
        _controls.PlaybackStatus = MediaPlaybackStatus.Closed;
        _controls.DisplayUpdater.ClearAll();
        _controls.DisplayUpdater.Update();
        _controls.IsEnabled = false;
    }
}
