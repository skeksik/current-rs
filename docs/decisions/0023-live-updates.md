# 0023: Live updates of user state from Jellyfin

Status: Accepted (owner, 2026-10-27), from feat/live-user-data. The owner approved the two crates and this record on 2026-10-27.

## Context
ADR 0018 section 5 (amended 2026-10-27) keeps one store of each item's user state per session, which every page shows. A change
made on another device, by playback elsewhere, or by the server reached it only on a read: a refresh, the foreground, a page shown
again (the resync on focus). Jellyfin pushes such changes on its WebSocket.

What Jellyfin 12.3 does (tag v12.3, 06de749, source read 2026-10-27; not checked on a server):
- **Endpoint.** Any request that asks for an upgrade opens a socket: `WebSocketHandlerMiddleware.Invoke` checks only
  `IsWebSocketRequest`, no path. Clients use `/socket` (by convention; not verified). The session comes from the request's
  `Authorization` (`AuthorizationContext`, `RequestHelpers.GetSession`), as for any request; a token it does not take is refused
  with 403 (`WebSocketManager` throws a `SecurityException`, which `ExceptionMiddleware` answers with 403). No message subscribes
  to anything: `SessionWebSocketListener` registers the socket as the session's controller on connect.
- **Messages.** `{ "MessageType", "Data", "MessageId" }` (`OutboundWebSocketMessage`). `UserDataChanged` carries
  `UserDataChangeInfo { UserId, UserDataList }`, each entry a `UserItemDataDto` with its `ItemId` ("`dto.ItemId = i.Id`"), built
  when sent, for every save of the user's data but a progress report's, with each item's parent ("`// Go up one level`"), batched
  per user for 500 ms (`UserDataChangeNotifier`). The client's own saves come back too.
- **Keep-alive.** On connect the server sends `ForceKeepAlive` with the seconds it waits (`WebSocketLostTimeout = 60`), and drops a
  socket that sends no `KeepAlive` message in that time; a ping frame does not count (`WebSocketConnection`: `LastKeepAliveDate`).
  It answers each `KeepAlive` with one (`SendKeepAliveResponse`).
- **Remote control.** A socket is a controller that supports media control (`WebSocketController.SupportsMediaControl =>
  HasOpenSockets`), but the session supports it only if the client's capabilities say so too: `SessionInfo.SupportsMediaControl`
  is "`if (Capabilities is null || !Capabilities.SupportsMediaControl) return false;`", and a new session's capabilities are the
  device's saved ones or empty (`DeviceManager.GetCapabilities`: "`: new()`", `SupportsMediaControl` false).

## Decision
1. **One socket per browse session** (`current-app`, `browse/live.rs`): opened with the session's first `Browse`, closed when its
   last list is dropped or the session ends (sign-out, removal, a new sign-in).
   Amended 2026-10-28 (owner): closed after the window has been minimized for a minute, reopened on restore. The platform reports
   the window's minimized state (`App::window_minimized`, `Core::window_minimized` in current-ffi) and the core keeps the rule: the
   socket closes once the window has been minimized for `LIVE_PAUSE_AFTER` (60 s), counted from the minimize, and no attempt is
   made while it stays minimized; a restore opens it again at once, followed by the usual resync (decision 6). A quick minimize and
   restore changes nothing, and a window that is behind other apps or not focused, but not minimized, keeps its socket. Playback
   does not depend on the socket and goes on while minimized. Windows reports it from the window's presenter state
   (`OverlappedPresenter.State`, read on every `AppWindow.Changed` and activation; `IsIconic` under the player's full-screen
   presenter); a restore also activates the window, which is the foreground return of decision 5. The macOS app will report
   miniaturize the same way (M3).
2. **What it carries.** `UserDataChanged` goes into the store as a server's answer that arrived now (newest wins, wanted changes on
   top), so every page shows it; the lists chosen by marks read again a moment later, as after a mark. Everything else is ignored:
   remote control (`Play`, `GeneralCommand`, `Playstate`), sessions, library changes, and kinds not known yet.
3. **Not controllable.** Current posts no capabilities, so the session is never offered to other clients as one to control (the
   default above); remote control messages are ignored if they come. A test checks that no capabilities are posted.
4. **The handshake** (`current-net`, `ServerClient::open_socket`) is a GET through the stream client (HTTP/1.1 only: an HTTP/2
   connection cannot upgrade; no timeout for the whole request; no redirect), with the service's credentials and the gateway's
   (ADR 0017) added as for every request, and its `Set-Cookie` taken as any answer's. So it uses the same TLS (reqwest's, ring and
   the platform verifier), system proxies, and plain-HTTP rule (ADR 0012, ADR 0014): `ws://` only where plain HTTP is allowed. The
   `Sec-WebSocket-Accept` is checked; any other answer than 101 is an error, a redirect never followed. `tokio-tungstenite` frames
   the upgraded connection and opens nothing itself. The request log gets one line per opening, like a request's.
5. **Keep-alive and reconnect.** A `KeepAlive` at once on `ForceKeepAlive`, then every half of its time; a socket that hears
   nothing for twice that time is taken for lost. After a broken or failed connection it opens again after a backoff of 1 s
   doubling to 60 s, with full jitter, back to 1 s once a socket lived 60 s; at once when the app returns to the foreground or the
   server's status changes (another address, reached again, a gateway signed in to). A refusal (Jellyfin's 403, a gateway's
   challenge or sign-in page, no token) is not retried until then: the session's own requests and the sign-in and gateway flows deal
   with it.
6. **Resync.** After each opening, the items the lists hold (at most `RESYNC_LIMIT`) read their state again (`GET Items?ids=`), for
   what changed while no socket was open. The resync on focus (ADR 0018 section 5, point 7) stays.
7. **Crates** (owner, 2026-10-27; ADR 0003): `tokio-tungstenite =0.30.0` (no default features, `handshake`) and `futures-util
   =0.3.34` (`sink`), in `current-net` and in the stand-in. New in `Cargo.lock`: tokio-tungstenite 0.30.0 (MIT), tungstenite 0.30.0,
   sha1 0.11.0, digest 0.11.3, block-buffer 0.12.1, crypto-common 0.2.2, const-oid 0.10.2, hybrid-array 0.4.15, typenum 1.20.1,
   cpufeatures 0.3.1, rand 0.10.3, rand_core 0.10.1, chacha20 0.10.2 (each MIT OR Apache-2.0), data-encoding 2.11.1 (MIT).

## Not verified
On Windows, which `AppWindow.Changed` flags or activation a minimize and a restore raise, and what a minimize from the player's
full screen does to the presenter (the owner checks both on their PC). A real server (the endpoint path, the batching, the echo of our own saves, the 403); upgrades through an HTTP proxy; how other
clients show our session. A push built before a later save of ours but delivered after its answer would show the older state until
the next read (the HTTP answer and the socket are separate connections); Jellyfin builds each batch when it sends it, which makes
this unlikely.

## Consequences
A mark made anywhere shows on every page within about half a second while the app runs. One socket and a keep-alive every 30 s per
signed-in server. The stand-in models the socket (`tools/jellyfin-stand-in`), and `crates/app/tests/live.rs` tests it.
