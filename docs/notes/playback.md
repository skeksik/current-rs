# Playback spike notes (M2): current-playback and current-mpv on Windows (2026-10-25)

Decision: `docs/decisions/0019-playback-design.md`. This file has the evidence: what Jellyfin 12.2 does with a device profile, what
the playstate and segment endpoints expect, what libmpv 0.41.0 and FFmpeg 9.0.2 do with URLs, headers, cookies, redirects and TLS,
and what the core must hand the Windows UI. No code was written or run; everything below is read from source or documentation.

Sources, all read 2026-10-25:
- Jellyfin server, tag `v12.2` (commit `ca0f16eb7c195f720a1493ed469a27c4657db0c6`, 2026-10-05), the same commit ADR 0017 read.
- mpv, tag `v0.41.0` (commit `41f6a645068483470267271e1d09966ca3b9f413`), the release our Windows build pins
  (`build/mpv-windows/sources.toml`).
- FFmpeg 9.0.2, `https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz`, SHA-256 `8c385028...a96e002e`, the same tarball and checksum
  `build/mpv-windows/sources.toml` pins.
- Microsoft Learn, "Manual control of the System Media Transport Controls"
  (https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/system-media-transport-controls, updated 2026-08-27).

Paths below are relative to each source tree. "Read from code" means the behaviour was traced in the source, not observed.

## 1. Jellyfin 12.2: PlaybackInfo

### The request
`POST /Items/{itemId}/PlaybackInfo` (`Jellyfin.Api/Controllers/MediaInfoController.cs`, `GetPostedPlaybackInfo`, class `[Authorize]`).
Its query parameters are all marked `ParameterObsolete`; the body is `PlaybackInfoDto`
(`Jellyfin.Api/Models/MediaInfoDtos/PlaybackInfoDto.cs`): `UserId`, `MaxStreamingBitrate`, `StartTimeTicks`, `AudioStreamIndex`,
`SubtitleStreamIndex`, `MaxAudioChannels`, `MediaSourceId`, `LiveStreamId`, `DeviceProfile`, `EnableDirectPlay`,
`EnableDirectStream`, `EnableTranscoding`, `AllowVideoStreamCopy`, `AllowAudioStreamCopy`, `AutoOpenLiveStream`,
`AlwaysBurnInSubtitleWhenTranscoding`. Defaults when absent: the four `Enable*`/`Allow*` flags `true`, `AutoOpenLiveStream` `false`.

Without a `DeviceProfile` in the body the server falls back to the capabilities the device posted earlier:
```csharp
if (profile is null)
{
    var caps = _deviceManager.GetCapabilities(User.GetDeviceId());
```
and without any profile it does not compute routes at all (the `SetDeviceSpecificData` loop runs only `if (profile is not null)`).
So Current always sends its profile in the body.

### The profile (`MediaBrowser.Model/Dlna/DeviceProfile.cs` and siblings)
| Field | Type | Notes |
|---|---|---|
| `MaxStreamingBitrate` | `int?`, **default `8000000`** | Used when the body's `MaxStreamingBitrate` is absent: `maxStreamingBitrate ?? profile.MaxStreamingBitrate` (MediaInfoController). An unset value means 8 Mbit/s, which blocks direct play of most files |
| `MaxStaticBitrate` | `int?`, default `8000000` | Only for `EncodingContext.Static` (`MediaOptions.GetMaxBitrate`); playback uses `Streaming` |
| `DirectPlayProfiles[]` | `Container`, `VideoCodec`, `AudioCodec` (comma lists), `Type` (Video, Audio, Photo) | |
| `TranscodingProfiles[]` | `Container`, `Type`, `VideoCodec`, `AudioCodec`, `Protocol` (`http` or `hls`), `Context`, `MaxAudioChannels`, `MinSegments`, `SegmentLength`, `CopyTimestamps`, `EnableSubtitlesInManifest`, `EnableAudioVbrEncoding`, `Conditions[]` | `BreakOnNonKeyFrames` is `[Obsolete("This is always false")]` |
| `CodecProfiles[]` | `Type` (Video, VideoAudio, Audio), `Codec`, `Container`, `SubContainer`, `Conditions[]`, `ApplyConditions[]` | |
| `ContainerProfiles[]` | conditions on the container | |
| `SubtitleProfiles[]` | `Format`, `Method` (Encode, Embed, External, Hls, Drop), `Language`, `Container` | |

`ProfileCondition`: `Condition` (Equals, NotEquals, LessThanEqual, GreaterThanEqual, EqualsAny), `Property`, `Value`, `IsRequired`.
`ProfileConditionValue` includes `AudioChannels`, `AudioBitrate`, `AudioProfile`, `Width`, `Height`, `VideoBitDepth`,
`VideoBitrate`, `VideoFramerate`, `VideoLevel`, `VideoProfile`, `IsAnamorphic`, `RefFrames`, `NumAudioStreams`, `VideoCodecTag`,
`IsAvc`, `IsInterlaced`, `AudioSampleRate`, `AudioBitDepth`, `VideoRangeType`, `NumStreams`, `VideoRotation`.
`VideoRangeType` (`Jellyfin.Data.Enums`): `Unknown`, `SDR`, `HDR10`, `HLG`, `DOVI`, `DOVIWithHDR10`, `DOVIWithHLG`, `DOVIWithSDR`,
`DOVIWithEL`, `DOVIWithHDR10Plus`, `DOVIWithELHDR10Plus`, `DOVIInvalid`, `HDR10Plus`.
Enums are serialised as strings (`src/Jellyfin.Extensions/Json/JsonDefaults.cs` adds `new JsonStringEnumConverter()`).

### How the server decides (`Jellyfin.Api/Helpers/MediaInfoHelper.cs`, `MediaBrowser.Model/Dlna/StreamBuilder.cs`)
- **Direct stream is switched off for every client:**
  ```csharp
  if (!options.ForceDirectStream)
  {
      // direct-stream http streaming is currently broken
      options.EnableDirectStream = false;
  }
  ```
  `ForceDirectStream` is never set from the PlaybackInfo request, so `StreamBuilder` returns `DirectPlay` or `Transcode`, and
  `SupportsDirectStream` ends up equal to `SupportsDirectPlay` ("Players do not handle this being set according to PlayMethod").
  **Remux therefore arrives as a transcode**: a `TranscodingUrl` whose job copies the video and/or audio stream
  (`AllowVideoStreamCopy`/`AllowAudioStreamCopy` default true).
- **Bitrate gate before anything else** (`BuildVideoItem`): `IsBitrateLimitExceeded(item, options.GetMaxBitrate(false))`; a source
  without a known bitrate counts as 40 Mbit/s ("If we don't know the item bitrate, then force a transcode if requested max bitrate is
  under 40 mbps"); remote sources (`IsRemote`) are never limited. Exceeding it sets `ContainerBitrateExceedsLimit` and makes the
  source ineligible for direct play.
- **The effective limit** (`GetMaxBitrate`): the client's value, lowered to the user's `RemoteClientBitrateLimit` (else the server's)
  when the client's IP is not in the server's local network.
- **Never direct play:** DVD and Blu-ray folders, and manifest containers (`"hls,applehttp,dash"`: "A manifest is not a byte stream").
- **Direct play check** (`GetVideoDirectPlayProfile`): container conditions, video codec conditions, audio candidates, then the
  subtitle: a selected subtitle whose profile method is not `Drop`, `External` or `Embed` (that is, `Encode` or `Hls`) adds
  `SubtitleCodecNotSupported` and blocks direct play. Each `DirectPlayProfile` of type Video is tried; failures are ranked
  (`VideoCodecNotSupported`, video codec conditions, `AudioCodecNotSupported`, audio conditions, container) and the best failure is
  kept; no match and no reason gives `DirectPlayError`.
- **Audio choice inside a source:** the default audio stream comes from the user's settings (below); unless the client asked for an
  index or the user's choice is remembered, any candidate audio stream that the profile can play may be picked.
- **Permissions** (user policy, `MediaBrowser.Model/Users/UserPolicy.cs`: `EnableAudioPlaybackTranscoding`,
  `EnableVideoPlaybackTranscoding`, `EnablePlaybackRemuxing`, `ForceRemoteSourceTranscoding`, all default true except the last):
  a video with all three of audio transcoding, video transcoding and remuxing off gets `SupportsTranscoding = false`;
  `ForceRemoteSourceTranscoding` on a remote source turns off direct play and adds `&allowVideoStreamCopy=false&allowAudioStreamCopy=false`.
- **Sorting** (`SortMediaSources`): the queried item's own source first ("must stay the default the client plays"), then direct-play
  files, then direct play or stream, then local files, then bitrate under the limit.

### The answer
`PlaybackInfoResponse`: `MediaSources[]`, `PlaySessionId`, `ErrorCode`. Each `MediaSourceInfo` (`MediaBrowser.Model/Dto/MediaSourceInfo.cs`)
carries among others `Id`, `Protocol`, `Container`, `Size`, `Name`, `IsRemote`, `ETag`, `RunTimeTicks`, `SupportsDirectPlay`,
`SupportsDirectStream`, `SupportsTranscoding`, `IsInfiniteStream`, `RequiresOpening`, `LiveStreamId`, `VideoType`, `MediaStreams[]`,
`MediaAttachments[]`, `Bitrate`, `RequiredHttpHeaders`, `TranscodingUrl`, `TranscodingSubProtocol`, `TranscodingContainer`,
`DefaultAudioStreamIndex`, `DefaultSubtitleStreamIndex`, `HasSegments`.

**`TranscodeReasons` is `[JsonIgnore]` on `MediaSourceInfo`.** The reasons reach the client only inside `TranscodingUrl`:
`StreamInfo.ToUrl` appends `&TranscodeReasons=` with the flag names (`GetUniqueFlags()`, joined with `,`) when the route is not
direct. They also appear on the session's `TranscodingInfo.TranscodeReasons` (`MediaBrowser.Model/Session/TranscodingInfo.cs`), with
`IsVideoDirect`, `IsAudioDirect`, codecs, bitrate, `CompletionPercentage` and `HardwareAccelerationType`.
`TranscodeReason` (flags, `MediaBrowser.Model/Session/TranscodeReason.cs`): `ContainerNotSupported`, `VideoCodecNotSupported`,
`AudioCodecNotSupported`, `SubtitleCodecNotSupported`, `AudioIsExternal`, `SecondaryAudioNotSupported`, `StreamCountExceedsLimit`,
`VideoProfileNotSupported`, `VideoRangeTypeNotSupported`, `VideoCodecTagNotSupported`, `VideoLevelNotSupported`,
`VideoResolutionNotSupported`, `VideoBitDepthNotSupported`, `VideoFramerateNotSupported`, `VideoRotationNotSupported`,
`RefFramesNotSupported`, `AnamorphicVideoNotSupported`, `InterlacedVideoNotSupported`, `AudioChannelsNotSupported`,
`AudioProfileNotSupported`, `AudioSampleRateNotSupported`, `AudioBitDepthNotSupported`, `ContainerBitrateExceedsLimit`,
`VideoBitrateNotSupported`, `AudioBitrateNotSupported`, `UnknownVideoStreamInfo`, `UnknownAudioStreamInfo`, `DirectPlayError`.

### URLs and the token
- **`TranscodingUrl`** is relative (`StreamInfo.ToUrl(null, token, ...)`): `/videos/{itemId}/master.m3u8?...` for HLS or
  `/videos/{itemId}/stream.{container}?...` for progressive, with `DeviceId`, `MediaSourceId`, codecs, stream indexes, bitrates,
  `PlaySessionId`, **`&ApiKey=<the caller's token>`**, `Tag`, `TranscodeReasons` and the profile's options.
- **Direct play** has no URL in the answer; the client builds the static stream URL: `GET /Videos/{itemId}/stream` (or
  `stream.{container}`) with `static=true`, `mediaSourceId`, `playSessionId`, `tag` (`Jellyfin.Api/Controllers/VideosController.cs`
  `GetVideoStream`). A static remote source (`InputProtocol == Http`) is proxied by the server
  (`GetStaticRemoteStreamResult`), not redirected. `GetVideoStream` carries no `[Authorize]` attribute and `VideosController` has no
  class-level one; whether the server requires a token for it is **not verified** (Current sends the token anyway).
- **HLS** playlists copy the request's query string into every variant and segment URL (`DynamicHlsHelper.cs`:
  `var queryString = _httpContextAccessor.HttpContext.Request.QueryString.ToString();`; `DynamicHlsController.cs` passes
  `Request.QueryString.ToString()` to `CreateMainPlaylistRequest`), so an `ApiKey` in the master URL is repeated in every segment URL.
  Subtitle playlists and trickplay in a manifest add `&ApiKey=` themselves (`DynamicHlsHelper.cs`).
- **External subtitles:** the delivery URL is `/Videos/{id}/{mediaSourceId}/Subtitles/{index}/{startTicks}/Stream.{format}` plus
  `?ApiKey=<token>` (`StreamInfo.GetSubtitleStreamInfo`), except when the subtitle is an external file whose path is an absolute
  http(s) URL: then `info.Url = stream.Path; info.IsExternalUrl = true;` and no token is added. **That URL points to another host.**
- **How the token is read** (`Jellyfin.Server.Implementations/Security/AuthorizationContext.cs`): the `Token` field of
  `Authorization: MediaBrowser ...`; with `EnableLegacyAuthorization` also `X-Emby-Token`, `X-MediaBrowser-Token`; then always
  `queryString["ApiKey"]`; `api_key` only with legacy authorization. So the `ApiKey` query value works on every 12.2 server.

### Other endpoints
- `GET /Playback/BitrateTest?size=<bytes>` (`MediaInfoController`, `[Range(1, 100_000_000)]`, default 102400): bytes for measuring
  throughput.
- `POST /LiveStreams/Open`, `/LiveStreams/Close`: live TV, out of scope (roadmap: not planned).

## 2. Jellyfin 12.2: playstate (`Jellyfin.Api/Controllers/PlaystateController.cs`)
| Endpoint | Body | Server side |
|---|---|---|
| `POST /Sessions/Playing` | `PlaybackStartInfo` (= `PlaybackProgressInfo`) | `OnPlaybackStart` |
| `POST /Sessions/Playing/Progress` | `PlaybackProgressInfo`: `ItemId`, `MediaSourceId`, `AudioStreamIndex`, `SubtitleStreamIndex`, `IsPaused`, `IsMuted`, `PositionTicks`, `PlaybackStartTimeTicks`, `VolumeLevel`, `PlayMethod`, `LiveStreamId`, `PlaySessionId`, `RepeatMode`, `PlaybackOrder`, `CanSeek`, `NowPlayingQueue`, `PlaylistItemId` | `OnPlaybackProgress`; also pings the transcode job |
| `POST /Sessions/Playing/Ping?playSessionId=` | none | `PingTranscodingJob` only |
| `POST /Sessions/Playing/Stopped` | `PlaybackStopInfo`: `ItemId`, `MediaSourceId`, `PositionTicks`, `LiveStreamId`, `PlaySessionId`, `Failed`, `NextMediaType`, `PlaylistItemId`, `NowPlayingQueue` | kills the play session's transcodes, then `OnPlaybackStopped` |

- `PlayMethod` is validated: a reported `Transcode` with no transcoding job for the `PlaySessionId` is recorded as `DirectPlay`
  (`ValidatePlayMethod`).
- **Transcode keep-alive** (`MediaBrowser.MediaEncoding/Transcoding/TranscodeManager.cs`): `TranscodeManager` subscribes to
  `PlaybackProgress` and pings the job with the play session id. The kill timer is 10 s for progressive jobs and 60 s for HLS
  (`timerDuration = 10000; if (job.Type != TranscodingJobType.Progressive) { timerDuration = 60000; }`). HLS segment requests also
  keep it alive. **A paused HLS player that reports nothing for 60 s loses its transcode.**
- **Idle sessions** (`Emby.Server.Implementations/Session/SessionManager.cs`): every 5 minutes the server stops playback for sessions
  whose `LastPlaybackCheckIn` is more than 5 minutes old, recording the last checked-in position. With `InactiveSessionThreshold > 0`
  (server setting, default 0) a session paused longer than that many minutes is stopped.
- **Played and resume** (`Emby.Server.Implementations/Library/UserDataManager.cs`, `UpdatePlayState`; defaults in
  `ServerConfiguration.cs`): under `MinResumePct` (5) the position is dropped; over `MaxResumePct` (90), or within one second of the
  end, the item is marked played and the position reset; items shorter than `MinResumeDurationSeconds` (300) are marked played. So the
  server decides "played" from the reported position; the client only reports.
- **Remembered tracks** (`SessionManager.UpdatePlaybackSettings`): on each progress or stop report, when the user's
  `RememberAudioSelections` / `RememberSubtitleSelections` is on, the reported `AudioStreamIndex` / `SubtitleStreamIndex` is saved in
  the user's data **for that item**; when off, it is cleared.

## 3. Jellyfin 12.2: the user's track preferences
- Where they live: `UserDto.Configuration` (`MediaBrowser.Model/Configuration/UserConfiguration.cs`), returned with the user
  (`GET /Users/Me`): `AudioLanguagePreference`, `PlayDefaultAudioTrack` (default true), `SubtitleLanguagePreference`, `SubtitleMode`,
  `RememberAudioSelections` (default true), `RememberSubtitleSelections` (default true), `EnableNextEpisodeAutoPlay` (default true).
  `SubtitlePlaybackMode` (`src/Jellyfin.Database/.../Enums/SubtitlePlaybackMode.cs`): `Default`, `Always`, `OnlyForced`, `None`,
  `Smart` ("Only show subtitles when the current audio stream is in a different language").
- How the server applies them (`Emby.Server.Implementations/Library/MediaSourceManager.cs`, `SetDefaultAudioAndSubtitleStreamIndices`):
  for video items, a remembered index from the user's item data wins when remembering is on and the item allows it
  (`EnableRememberingTrackSelections`, true except live TV channels); otherwise `MediaStreamSelector` picks by the language
  preference, the original language and `PlayDefaultAudioTrack`. The result is the answer's `DefaultAudioStreamIndex` and
  `DefaultSubtitleStreamIndex`.
- So following the Jellyfin user's own preferences needs no logic in Current beyond using the two defaults and reporting the indexes the
  user picks; the same choices then follow the user to every other client.

## 4. Jellyfin 12.2: media segments
- `GET /MediaSegments/{itemId}?includeSegmentTypes=...` (`Jellyfin.Api/Controllers/MediaSegmentsController.cs`, `[Authorize]`,
  route from `BaseJellyfinApiController`'s `[Route("[controller]")]`): `QueryResult<MediaSegmentDto>` with `Id`, `ItemId`, `Type`,
  `StartTicks`, `EndTicks`.
- `MediaSegmentType`: `Unknown`, `Commercial`, `Preview`, `Recap`, `Outro`, `Intro`.
- `MediaSourceInfo.HasSegments` says whether to ask (`BaseItem.cs`: `MediaSegmentManager.IsTypeSupported(item) &&
  MediaSegmentManager.HasSegments(item.Id)`).
- **Segments come from providers** (`MediaBrowser.Controller/MediaSegments/IMediaSegmentProvider.cs`); no class in the 12.2 server
  implements the interface (searched the tree), so segments exist only when a plugin provides them. Intro and credits skip is therefore
  optional: shown only when `HasSegments` is true and a segment covers the position.

## 5. Next episode
- `GET /Shows/{seriesId}/Episodes?startItemId=<current>&...` ("Skip through the list until a given item is found",
  `Jellyfin.Api/Controllers/TvShowsController.cs`) gives the current episode followed by the next ones in order; `GET /Shows/NextUp`
  is the home row. The browse spike (ADR 0018) owns these item queries; playback only needs "the episode after this one".
- `EnableNextEpisodeAutoPlay` is the Jellyfin user's own setting for continuing automatically.

## 6. libmpv 0.41.0
### Client API (`include/mpv/client.h`, `MPV_CLIENT_API_VERSION` 2.5)
- Threads: "The client API is generally fully thread-safe, unless otherwise noted. Currently, there is no real advantage in using more
  than 1 thread to access the client API, since everything is serialized through a single lock in the playback core."
- Wakeup callback (`mpv_set_wakeup_callback`): "You are not allowed to call any client API functions inside of the callback. In
  particular, you should not do any processing in the callback, but wake up another thread that does all the work."
- Property observation: "Property changes are coalesced: the change events are returned only once the event queue becomes empty
  (e.g. mpv_wait_event() would block or return MPV_EVENT_NONE), and then only one event per changed property is returned." and "You
  always get an initial change notification."
- End of file: `MPV_EVENT_END_FILE` with `MPV_END_FILE_REASON_EOF`, `STOP`, `QUIT`, `ERROR` (with an mpv error code), `REDIRECT`.
  `MPV_EVENT_START_FILE`, `MPV_EVENT_FILE_LOADED`, `MPV_EVENT_PLAYBACK_RESTART` (after a seek or start).
- Environment notes in the header that matter on Windows: "On Windows, mpv will call timeBeginPeriod(1)"; "mpv uses some other
  libraries that are not library-safe, such as Fribidi (used through libass), ALSA, FFmpeg".

### Commands and properties used (`DOCS/man/input.rst`)
- `loadfile <url> [<flags> [<index> [<options>]]]`: the fourth argument sets options "during playback, and restored to the previous
  value at end of playback"; through the client API it "can be a `MPV_FORMAT_NODE_MAP`", with string values. Since 0.38.0 the third
  argument (index) must be `-1` when options are given.
- `sub-add <url> [<flags> [<title> [<lang>]]]` for external subtitles; `memory://data` (`DOCS/man/mpv.rst`) plays a string as a file.
- `display-swapchain`: "Read-only - Direct3D 11 swapchain address. Returns an int64 type value representing the memory address of the
  D3D11 swapchain."
- `time-pos` (RW, seconds; `time-pos/full` with milliseconds), `pause`, `duration`, `track-list`, `aid`, `sid`, `seekable`,
  `paused-for-cache`, `demuxer-cache-state`, `eof-reached`, `video-params`, `hwdec-current`.

### Options (`DOCS/man/options.rst`)
- `--d3d11-output-mode=composition`: "'composition' will not create a window. If you want to use the D3D11 GPU backend in WinUI
  applications, you need to set this to 'composition'." `--d3d11-composition-size=<WxH>`: "When use composition mode, there is no
  window, must set the output size by the embedding application."
- `--display-fps-override=<fps>`: "Set the display FPS used with the `--video-sync=display-*` modes." (present in 0.41.0; D6's
  `check.py` sets it on our build).
- `--target-colorspace-hint-mode=<target|source|source-dynamic>`: `source-dynamic` "does not send full HDR10+ or Dolby Vision
  metadata, but uses that information to produce HDR10 with per-scene luminance values"; "Default is `target`."
- Network: `--user-agent` (default `libmpv`, `stream/stream_lavf.c`), `--cookies=<yes|no>` ("Disabled by default"),
  `--cookies-file=<filename>` ("assumed to be in Netscape format"), `--http-header-fields`, `--tls-verify` ("Verify peer
  certificates when using TLS"), `--tls-ca-file`, `--network-timeout` ("default: 60 seconds"), `--stream-lavf-o` ("Set AVOptions on
  streams opened with libavformat. Unknown or misspelled options are silently ignored."), `--demuxer-lavf-propagate-opts` ("Propagate
  FFmpeg-level options to recursively opened connections (default: yes)").
- Key/value list options (`DOCS/man/mpv.rst`): "When using libmpv, prefer using `MPV_FORMAT_NODE_MAP`".
- Logging: `--msg-level=<module=level,...>` "also affects `--log-file`, and in certain cases libmpv API logging"; `--log-file`
  writes "at least `-v -v`".

### What mpv does with network options (`stream/stream_lavf.c`, `mp_setup_av_network_options`)
```c
if (opts->cookies_enabled) {
    ...
    char *cookies = cookies_lavf(temp, global, log, file);
    if (cookies && cookies[0])
        av_dict_set(dict, "cookies", cookies, 0);
}
av_dict_set(dict, "tls_verify", opts->tls_verify ? "1" : "0", 0);
...
if (strlen(cust_headers))
    av_dict_set(dict, "headers", cust_headers, 0);
...
mp_set_avdict(dict, opts->avopts);
```
- **Cookies come only from a file.** `cookies_lavf` (`stream/cookies.c`) reads the Netscape file and turns each cookie into FFmpeg's
  `name=value; path=...; domain=...; secure` line. ADR 0017 said "libmpv gets gateway cookies through its `cookies` option"; read
  exactly, mpv's `cookies` is the on/off switch for `cookies-file`. A cookie file would put gateway secrets on disk.
- **`stream-lavf-o` is applied last** (`mp_set_avdict(dict, opts->avopts)`), so FFmpeg's own `cookies`, `headers` and
  `max_redirects` options can be given directly, without a file, and they override mpv's.
- **mpv turns TLS verification off by default:** `tls_verify` is set to `"0"` unless `--tls-verify=yes`, although FFmpeg's own
  default is on (below). Read from code.
- `stream_lavf` also sets `reconnect=1`, `reconnect_delay_max=7` for the files it opens.
- For HLS and DASH mpv lets FFmpeg open the URL itself (`demux/demux_lavf.c` format hack `{"hls", .no_stream = true, ...}`), with the
  same network options (`mp_setup_av_network_options(&dopts, ...)`), and `nested_io_open` copies those options into every nested open
  (playlists, segments, keys) for keys the nested open does not already have, while `demuxer-lavf-propagate-opts` is on.

### Logging that carries URLs
- `player/loadfile.c`: `MP_INFO(mpctx, "Playing: %s\n", mpctx->filename);` at info level; `MP_VERBOSE` "Opening done: %s" and
  `stream/stream.c` "Opening %s" at verbose. FFmpeg's http errors are logged through mpv's lavf log bridge (wording not checked).
  So any log level from info up carries the URL, `ApiKey` included.

## 7. FFmpeg 9.0.2 (`libavformat/`)
- `http.c` options: `headers` ("set custom HTTP headers, can override built in default headers"), `cookies` ("set cookies to be sent
  in applicable future requests, use newline delimited Set-Cookie HTTP field value syntax"), `max_redirects` ("Maximum number of
  redirects", default `MAX_REDIRECTS` = 8, range 0 to `INT_MAX`), `reconnect*`, `user_agent`.
- Redirects (`http_open_cnx`): on 301, 302, 303, 307, 308 with a `Location`, `if (redirects++ >= s->max_redirects) return
  AVERROR(EIO);`, then the new location is opened with the same `s->headers`; the auth state is reset, the headers are not. A cached
  redirect is counted the same way. So **`max_redirects=0` makes any redirect an error** (read from code, not tested).
- Cookies are sent only where they apply (domain and `secure` matching in `http.c`, already noted in `docs/notes/gateway-auth.md`).
- TLS (`tls.h`): `tls_verify` / `verify` default `1`. With SChannel (`tls_schannel.c`) verification uses
  `SCH_CRED_AUTO_CRED_VALIDATION | SCH_CRED_REVOCATION_CHECK_CHAIN`, that is Windows' own certificate validation with a revocation
  check of the chain; without it, `SCH_CRED_MANUAL_CRED_VALIDATION` and no check. Whether an unreachable revocation server fails the
  connection is **not verified**.
- HLS (`hls.c`): every playlist, segment and key request goes through `open_url`, which allows only `http(s)`, `file` (by extension)
  and `data`; it uses the options from `ffio_copy_url_options` (`aviobuf.c`: `"headers", "user_agent", "cookies", "http_proxy",
  "referer", "rw_timeout", "icy"`) and updates `cookies` from each response. `max_redirects` and `tls_verify` are not in that list; with
  mpv they reach segments through `nested_io_open` (above). Read from code, not tested.

## 8. Microsoft Learn: system media transport controls
"Manual control of the System Media Transport Controls" (updated 2026-08-27):
- "There are a few scenarios where you may need to implement manual control of the SMTC" (our case: no `MediaPlayer`).
- "Enable the buttons that your app will use by setting the corresponding 'is enabled' property ... such as IsPlayEnabled,
  IsPauseEnabled, IsNextEnabled, and IsPreviousEnabled." "Register a handler for the ButtonPressed event".
- "Apps should set a value for the SystemMediaTransportControlsDisplayUpdater.Type property even if they aren't supplying other media
  metadata ... This value helps the system handle your media content correctly, including preventing the screen saver from activating
  during playback."
- "set the PlaybackStatus property to the appropriate MediaPlaybackStatus value" when the state changes; timeline through
  `SystemMediaTransportControlsTimelineProperties` and `UpdateTimelineProperties`.
- For a desktop window the controls come from `SystemMediaTransportControlsInterop.GetForWindow` (ADR 0007's source, read
  2026-10-22). D2 found that a seek from the flyout leaves a stale position until the timeline is updated (`docs/notes/windows-video.md`).

## 9. What D6 already settled (not reopened)
- mpv **0.41.0**: `docs/notes/windows-mpv-build.md` (2026-10-22), "mpv | 0.41.0 | LGPL-2.1-or-later (`-Dgpl=false`) | DLL | The engine.
  0.41.0 has `--d3d11-output-mode=composition`, `--d3d11-composition-size` and `--display-fps-override` (its `DOCS/man/options.rst`)";
  its licence check makes libmpv accept `display-fps-override` and the ADR 0007 colour options. Confirmed in the v0.41.0 tree today
  (section 6).
- **uchardet**: ADR 0003 (2026-10-22), "We use it under LGPL-2.1-or-later (owner's approval of uchardet, 2026-10-22)."
- **LAME**: ADR 0003, "LAME is left out (owner, 2026-10-22: a player needs no MP3 encoder)."
- Roadmap D6 (2026-10-22, PR 9): "no LAME; uchardet allowed (owner, 2026-10-22)".
- Still open from the roadmap and out of scope here: the Visual C++ runtime under Microsoft's redistributable terms, to be recorded in
  ADR 0003 with the owner when `current-mpv` first ships libmpv.

## 10. The browse spike's media summary (ADR 0018)
The browse spike (spike/browse-model) puts a thin media summary on its details type: container, stream counts and labels. The
playback types in ADR 0019 (`MediaSource` with its `MediaStream`s) are a superset: the summary is derived from the item's first media
source (its container, the number of video, audio and subtitle streams, and each stream's display label), so the details screen does
not need a second wire type. Which of the two crates converts is settled when both merge; the ADR 0019 types live in `current-model`.

## Not verified
- Every behaviour above is read from source or documentation; nothing was run against a Jellyfin 12.2 server, libmpv or FFmpeg.
- Whether `GET /Videos/{id}/stream` needs a token at all on 12.2.
- `max_redirects=0` reaching HLS segment requests through mpv's option propagation, and the error mpv reports when a redirect is
  refused (expected `MPV_END_FILE_REASON_ERROR` with a loading error; not checked).
- SChannel's revocation check when the revocation server is unreachable.
- Dolby Vision (profiles 5, 7, 8) and HDR10+ through libplacebo in our build; 23.976 and 24 fps on 60, 120 and 144 Hz displays; and
  `video-sync=display-resample` (ADR 0007's list, still open).
- How often mpv's `time-pos` change notices arrive in practice (the coalescing rule above bounds them by the event loop, not by a rate).
