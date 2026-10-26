# The loopback relay on Windows: the owner's PC (ADR 0020 task 5)

test/relay-windows-check, from `origin/main` at 99b5d18 (PRs 78 and 79). Checked live on 2026-10-26 by the owner with Claude, on the
owner's Windows 11 PC, against the owner's own Jellyfin 12.3.0 server and the gateways in front of it. The app ran on the owner's
normal data folder (the owner's choice). Gateways are described by kind only (ADR 0017): a **header gateway** (extra headers, a
service token), a **Basic gateway**, and a **cookie gateway** (a web sign-in portal, signed in through Current's sign-in window).

## Build and tests on this PC
- libmpv: the D6 build (`libmpv-windows-x64`, mpv v0.41.0, FFmpeg 9.0.2, from CI artifact 12448213025, 2026-10-22), downloaded with the
  owner's agreement. The libmpv folder first named was a third-party build (one static DLL, FFmpeg 9.0.1, no `components.json`), so
  `build.ps1` stopped at its licence step; it was not used.
- `bindings/csharp/build.ps1`: ok. `bindings/csharp/test.ps1`: 57 passed. `apps/windows/build.ps1 -Mpv <D6>`: ok. `smoke.ps1` with the
  stand-in: passed.
- `cargo test -p current-mpv --features libmpv`: 46 passed, 1 ignored (the child-process test, as on CI).
- `cargo test -p current-app --features libmpv -- --test-threads=1`: the library's 130 tests (the libmpv live tests through the relay
  included) and 89 in the other targets passed.
- The final runs before the PR are in the PR description.

## How it was watched
- **The process:** a read-only PowerShell probe of `Current.exe`: its listening sockets, its loopback connections, the number of its
  other connections (no remote address printed), its CPU over a few seconds, and the Windows Firewall rules naming it.
- **mpv's log:** a temporary switch in the local build only (never committed, removed before the PR) wrote mpv's log lines, already
  redacted by `current-mpv`, at level `v` to a file on the PC. Only error lines, the URLs' shape and counts were read.
- **The owner:** what the screen showed, and the Jellyfin dashboard.

## Results
Each case: what was done, what was expected, what happened.

### 1. Direct play, remux and transcode through the gateways
Media: the owner's own items for direct play; for remux and transcode, a generated test file (15 min of H.264 at 4 Mbit/s and AAC in
FLV, with the time drawn on the picture and a sidecar SubRip file). FLV is not in Current's direct-play containers, so Jellyfin copies
both streams into HLS fMP4 (shown "Remux · HLS · mp4"); a lower quality makes it transcode (shown "Transcode · HLS · mp4", HEVC).
Expected: each plays through the relay, seeks forward and back (beyond the loaded part too) land where asked, a pause of several minutes
then play goes on, playing again resumes, and the dashboard shows the session and its progress.

- **Header gateway: passed** after two fixes below. Direct play: the relay listened on `127.0.0.1` only, on a port of the OS's choice,
  only while something played (nothing listened at idle); libmpv held one loopback connection, and after a seek a new one with the old
  one closed. Seeks, a pause of about 5 minutes, resume and the dashboard's progress: passed. Remux and transcode: passed, including a
  pause of about 5 minutes on the transcode and its resume.
- **Basic gateway: passed.** Direct play (quality at "Maximum"), remux and transcode, with seeks, a pause of about 5 minutes, resume and
  the dashboard. mpv's log showed the direct stream and every HLS load on `127.0.0.1`.
- **Cookie gateway: passed.** The same set, signed in through the sign-in window. In that app run (which also held case 4's re-check),
  mpv's log had 12 direct-play loads and 30 HLS loads, all on `127.0.0.1`.
- **Found and fixed (b46f545): the sound went on for 1 to 2 s after Back** on a remux through a gateway. Not the relay: the controller
  sent Jellyfin's stop report and waited for its answer before unloading the engine (ADR 0019, "Stopping"), and that answer took a
  second or two there. Stop and playing another item now unload first, then report; ADR 0019 amended (owner). Test:
  `stop_and_another_item_unload_the_engine_before_the_stop_report` (current-playback), red before the fix. Rechecked live: passed.
- **Found and fixed (34f0db7): no HLS stream played behind a gateway.** Jellyfin 12.3.0 wrote the item id in the `TranscodingUrl` as a
  GUID with dashes (`/videos/8-4-4-4-12/master.m3u8`); the relay's route allowed only the 32-hex form, so it answered 404 before
  anything went upstream, and the player showed "The player couldn't open the stream" (`Engine`). Direct play worked because the core
  builds that URL. The route now allows both spellings of the id (`path_ids` in `crates/app/src/player/server.rs`), and the stand-in
  spells its HLS paths with dashes (`TRANSCODE_ITEM_PATH_ID`), so the bridge and libmpv live tests cover it; the bridge test
  `hls_is_rewritten_through_the_relay_and_a_playlist_that_points_elsewhere_fails` was red before the fix. Rechecked live: passed.
- Seen, not changed here: the info panel's "The player can't read this container" for the FLV is Jellyfin's `ContainerNotSupported`;
  FFmpeg can read FLV, Current's profile does not list it. One play also listed `AudioCodecNotSupported`, another did not (not
  looked into).
- Seen, by design: a lower quality on a direct-play item transcodes it, and the quality is remembered per kind of address (ADR 0019
  choice 10), so it stays until set back to "Maximum".

### 2. External subtitles on a transcode through a gateway
Done: the test file's sidecar SubRip on its transcode, through the header gateway, then the cookie gateway. Expected: the track is
listed, can be selected and shows. **Passed**: listed, selected, and "External subtitle line N" showed and changed every 3 s; mpv's log
showed the core-fetched subtitle loaded from memory.

### 3. Network switch mid-play
Done: playing through a gateway, the PC was moved to another network (a phone hotspot) and back, three times.
Expected: playback continues at the next range or segment, or fails with a clear message.
- Tries 1 and 2: the hotspot could not reach the gateway's address at the time (a problem on the gateway provider's side, the owner
  says). FFmpeg reconnected at the next byte at once; the relay's upstream request found no address. Try 1: the relay answered 502 and
  mpv ended the file; the app ended on the item's page with "Can't reach the server". Try 2: "Loading", then after the 60 s stall rule
  "The server can't be reached. Check your connection and try again." (`Network`). Playing again on a working network resumed.
  **Passed** (fails with a clear message), noting that it took the stall rule's 60 s.
- Try 3, the gateway reachable after the switch: **passed** (continues). Direct play went on after FFmpeg's reconnect at the next byte;
  HLS went on at the next segment after one lost keep-alive connection; no loading and no message within 30 s; a seek just after the
  switch loaded for a few seconds, then played.
- Not determined: whether another saved address was selected after the switch or the same gateway address on both networks. A change of
  address mid-play is covered by the bridge test with the test engine (PR 78). `send_stream` retries once, and only on a different
  address (ADR 0014); a retry on the same address after a network change would be a change to ADR 0014 (not proposed here).

### 4. Gateway expiry
Done: on the header gateway, play, pause, the service token revoked at the gateway, then a seek. Expected: "Sign in to the gateway
again", the Jellyfin session kept, playback again after the gateway's sign-in.
- First try: revoking changed nothing; playback went on with no message (the gateway did not refuse yet; why is not verified).
- Second try: **passed.** The player showed "Sign in to your server's gateway again in Settings, then play again."
  (`GatewaySignInNeeded`); the user stayed signed in to Jellyfin; after signing in to the gateway again, playing resumed where it was
  paused. The seek's target was not saved: nothing reaches Jellyfin while the gateway refuses, the stop report included (expected).
- **Found and fixed (f0fdce4): Home showed "The server answered with an error (HTTP 302)"** until the gateway's sign-in. The Jellyfin
  client started ADR 0017's check on the gateway's redirect but gave the request the redirect itself (a sign-in page gave `Decode`). A
  request that waits now waits for that check and answers the gateway's sign-in when the probe found the gateway (`send_checked` in
  `crates/jellyfin/src/client.rs`); playstate reports and images, which do not wait, are unchanged. Tests (current-app,
  `tests/gateway.rs`): `an_expired_gateway_session_keeps_the_jellyfin_session_until_signed_in_again` now expects the gateway's answer
  for a sign-in page, and `an_expired_gateway_session_that_redirects_answers_the_gateway_sign_in` is new; both red before the fix.
  Rechecked live: Home showed "The gateway in front of the server wants you to sign in again. Sign in to the gateway in Settings.":
  passed.
- Cookie gateway: **not checked** live: its session could not be ended from outside Current. Covered on the stand-in by the bridge test
  `an_expired_gateway_cookie_mid_play_asks_for_the_gateway_and_keeps_the_session` (PR 78) and the tests above.
- Basic password change: **not checked** (not possible in the owner's setup).

### 5. Revoked session
Done: Current's device revoked in the Jellyfin dashboard mid-play. Expected: signed out, once, with no loop. **Passed**: the player
showed "The server ended your session. Sign in again in Settings."; Home showed the server signed out with "Sign in"; nothing repeated.

### 6. No firewall prompt
Done: every play of the day, with Windows Defender Firewall on for the domain, private and public profiles, each set to notify when a
new app listens (`Get-NetFirewallProfile`: `Enabled` and `NotifyOnListen` true). This build's `Current.exe` ran from a new folder, so no
firewall rule named it before the first play. Expected: no Windows Security Alert. **Passed**: none appeared (owner), and
`Get-NetFirewallApplicationFilter` listed no rule for `Current.exe` before or after all the plays. Not a fresh user profile: the
owner's own account, an administrator; whether that changes when the alert appears is not verified.

### 7. Credentials stay out of the player
- **mpv's log** (level `v`, already redacted; with the owner's agreement), over every gateway play of the day (up to 32,315 lines per
  run): `ApiKey`, `api_key`, `Cookie`, `Authorization`, `X-Emby`, `MediaBrowser`, `Token=` and `Basic `: **absent**. URLs not on
  `127.0.0.1`: **absent**. Every URL mpv opened was `http://127.0.0.1:<port>/<route>/...`. **Passed.**
- Limit: mpv logs the URLs it opens, not its request headers. That FFmpeg's requests to the relay carry no cookie or header is shown by
  CI's recording tap (PR 79), not by this log.
- **The app's log:** not checked. The request log has no reader in the Windows app (diagnostics are read by the smoke run only), and
  mpv's log lines have no reader in the app (`MpvEvent::Log` is dropped).

### 8. Not on a gateway
Done: a LAN address with no gateway selected, then a direct-play item. Expected: the direct URL, no relay. **Passed**: nothing
listened and nothing connected on loopback; libmpv opened the server's address directly, with `ApiKey` in the URL (redacted in the log),
as ADR 0020 keeps for unrelayed streams; all 8 of the process's other connections went to private addresses; playback and seeks as
before. The first try still went through the relay because the address in use then had a gateway.

### 9. Resources
**Not checked** as a long run (the owner skipped it). Spot readings with Task Manager's scale (all 32 logical processors), a few
seconds each: relayed direct play 0.1 % to 0.7 %, unrelayed direct play 0.2 %; working set 580 to 900 MB in both. No stutter noticed
during the day's plays.

## Not verified / follow-ups
- A long relayed play (case 9) and a fresh Windows user profile (case 6).
- The cookie gateway's expiry live, and a Basic password change (case 4).
- Roaming to another saved address mid-play on libmpv (case 3); the retry on the same address after a network change (ADR 0014).
- FLV is not in Current's direct-play profile, and the panel's wording says the player cannot read it.
- The core's request log and mpv's log have no reader in the Windows app.
