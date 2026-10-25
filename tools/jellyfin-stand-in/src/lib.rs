//! A hand-written Jellyfin stand-in on 127.0.0.1, for the bridge tests (Rust, C# and Swift). It answers only what server setup,
//! sign-in, restoring a session, sign-out, images, browsing a small fake library (see [`library`]) and a large generated one (see
//! [`paging`]) and playback (PlaybackInfo, playstate reports, media segments, the session list, HLS playlists) ask, in Jellyfin
//! 12.2's shapes as `current-jellyfin` reads them. Every name, id and credential in it is fake. Test code only: nothing shipped
//! depends on it.

pub mod library;
pub mod paging;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

/// The stand-in's server id.
pub const SERVER_ID: &str = "57a2d1e0000000000000000000000001";
/// The stand-in's server name.
pub const SERVER_NAME: &str = "Stand-in Server";
/// The one user that can sign in.
pub const USER_NAME: &str = "stand-in-user";
pub const USER_ID: &str = "57a2d1e0000000000000000000000002";
/// The one password that works. Obviously not a real one.
pub const PASSWORD: &str = "not-a-real-password";
/// The Quick Connect code the stand-in hands out. Its requests are approved at the first check.
pub const QUICK_CONNECT_CODE: &str = "123456";
/// The gateway's user name and password in [`Gate::Basic`].
pub const GATE_USER: &str = "stand-in-gate-user";
pub const GATE_PASSWORD: &str = "not-a-real-gate-password";
/// `stand-in-gate-user:not-a-real-gate-password` in base64.
const GATE_BASIC: &str = "c3RhbmQtaW4tZ2F0ZS11c2VyOm5vdC1hLXJlYWwtZ2F0ZS1wYXNzd29yZA==";
/// The realm of [`Gate::Basic`]'s challenge.
pub const GATE_REALM: &str = "Stand-in Gate";
/// The cookie [`Gate::Portal`] wants, and the value it takes until [`Controls::set_gate_cookie`] changes it.
pub const GATE_COOKIE: &str = "stand_in_gate";
pub const GATE_COOKIE_VALUE: &str = "not-a-real-gate-cookie";
/// Where [`Gate::Portal`]'s sign-in link goes: it sets the cookie.
pub const GATE_SIGN_IN_PATH: &str = "/gate/sign-in";
/// The header [`Gate::Header`] wants.
pub const GATE_HEADER: &str = "x-stand-in-gate";
pub const GATE_HEADER_VALUE: &str = "not-a-real-gate-header";
/// An item PlaybackInfo answers with direct play. It has an intro and an outro segment. The library's episodes play directly too.
pub const DIRECT_PLAY_ITEM: &str = "57a2d1e0000000000000000000000010";
/// An item PlaybackInfo answers with an HLS remux (`ContainerNotSupported`).
pub const TRANSCODE_ITEM: &str = "57a2d1e0000000000000000000000011";
/// The one media source of either item.
pub const MEDIA_SOURCE: &str = "57a2d1e0000000000000000000000012";
/// The play session id PlaybackInfo hands out.
pub const PLAY_SESSION: &str = "57a2d1e0000000000000000000000013";
/// The name of [`DIRECT_PLAY_ITEM`] and [`TRANSCODE_ITEM`] in their details (`/Items/{id}`), a film each.
pub const DIRECT_PLAY_NAME: &str = "Stand-in Direct Play";
pub const TRANSCODE_NAME: &str = "Stand-in Remux";
/// What every subtitle request (`/Videos/{item}/{source}/Subtitles/{index}/Stream.{format}`) of the two items gets: a SubRip file.
pub const SUBTITLE: &str = "1\r\n00:00:01,000 --> 00:00:02,000\r\nStand-in subtitle\r\n";
/// The user's `RemoteClientBitrateLimit` in bits per second.
pub const REMOTE_BITRATE_LIMIT: u32 = 8_000_000;
/// What every `/Items/{id}/Images/...` request gets: a 1 by 1 PNG. Jellyfin serves images without a token, and so does this.
pub const IMAGE: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53,
    0xde, 0x00, 0x00, 0x00, 0x0c, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x30, 0x48, 0x98, 0x00,
    0x00, 0x01, 0xe4, 0x01, 0x21, 0xca, 0x6c, 0xd2, 0x32, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
    0x44, 0xae, 0x42, 0x60, 0x82,
];
const QUICK_CONNECT_SECRET: &str =
    "57a2d1e0000000000000000000000000000000000000000000000000000000ff";

/// A running stand-in. It stops when dropped. Its controls ([`Controls`]) are reachable through it.
pub struct StandIn {
    controls: Controls,
    task: JoinHandle<()>,
}

impl std::fmt::Debug for StandIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StandIn").finish_non_exhaustive()
    }
}

/// An image request as the stand-in received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSeen {
    /// The path and query, e.g. `/Items/{id}/Images/Primary?fillWidth=160&...`.
    pub target: String,
    /// The `Accept` header.
    pub accept: String,
    /// The [`GATE_HEADER`] header, empty without it.
    pub gate_header: String,
}

/// A playlist request as the stand-in received it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaylistSeen {
    /// The path and query.
    pub target: String,
    /// The [`GATE_HEADER`] header, empty without it.
    pub gate_header: String,
}

/// The path of [`TRANSCODE_ITEM`]'s master playlist (its `TranscodingUrl`'s path).
pub fn master_playlist_path() -> String {
    format!("/videos/{TRANSCODE_ITEM}/master.m3u8")
}

/// The path of the media playlist [`TRANSCODE_ITEM`]'s master playlist names by default.
pub fn media_playlist_path() -> String {
    format!("/videos/{TRANSCODE_ITEM}/main.m3u8")
}

/// [`TRANSCODE_ITEM`]'s master playlist until [`Controls::set_playlist`] changes it: one variant, relative, in the shape of Jellyfin's
/// (not verified against a real server's bytes).
pub const MASTER_PLAYLIST: &str = "#EXTM3U\n\
#EXT-X-STREAM-INF:BANDWIDTH=8000000,AVERAGE-BANDWIDTH=8000000,CODECS=\"hvc1.1.4.L120,mp4a.40.2\",RESOLUTION=1920x1080\n\
main.m3u8?DeviceId=d&MediaSourceId=57a2d1e0000000000000000000000012&PlaySessionId=57a2d1e0000000000000000000000013&ApiKey=not-a-real-variant-key\n";

/// The media playlist until [`Controls::set_playlist`] changes it: fMP4 segments, relative.
pub const MEDIA_PLAYLIST: &str = "#EXTM3U\n\
#EXT-X-PLAYLIST-TYPE:VOD\n\
#EXT-X-VERSION:7\n\
#EXT-X-TARGETDURATION:6\n\
#EXT-X-MEDIA-SEQUENCE:0\n\
#EXT-X-MAP:URI=\"hls1/main/-1.mp4?PlaySessionId=57a2d1e0000000000000000000000013\"\n\
#EXTINF:6.0, nodesc\n\
hls1/main/0.mp4?PlaySessionId=57a2d1e0000000000000000000000013&runtimeTicks=0\n\
#EXTINF:4.0, nodesc\n\
hls1/main/1.mp4?PlaySessionId=57a2d1e0000000000000000000000013&runtimeTicks=60000000\n\
#EXT-X-ENDLIST\n";

/// An authentication gateway in front of the stand-in (ADR 0017), as a reverse proxy would answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Gate {
    /// None.
    #[default]
    Off,
    /// HTTP Basic: 401 with a challenge without [`GATE_USER`]'s password. It does not pass its `Authorization` on, so Jellyfin's
    /// fields then come from `X-Emby-Authorization`.
    Basic,
    /// A portal: an HTML sign-in page without the [`GATE_COOKIE`] cookie. Its link, [`GATE_SIGN_IN_PATH`], sets the cookie and
    /// redirects back to `/`, as a portal does after a sign-in: for the sign-in window, by hand or in the Windows smoke run.
    Portal,
    /// 403 without the [`GATE_HEADER`] header.
    Header,
}

#[derive(Default)]
struct State {
    gate: Mutex<Gate>,
    /// The cookie value [`Gate::Portal`] takes; [`GATE_COOKIE_VALUE`] when unset.
    gate_cookie: Mutex<Option<String>>,
    /// Live tokens.
    tokens: Mutex<Vec<String>>,
    issued: AtomicUsize,
    quick_connect_off: AtomicBool,
    /// Quick Connect requests stay unapproved.
    quick_connect_pending: AtomicBool,
    /// Paths whose requests wait until released.
    held: Mutex<HashSet<String>>,
    released: Notify,
    /// The path of every request, in order.
    paths: Mutex<Vec<String>>,
    /// Playstate reports received: path and body.
    reports: Mutex<Vec<(String, String)>>,
    /// Every image request that the gateway let through, in order.
    images: Mutex<Vec<ImageSeen>>,
    /// Playlists by path, over the defaults ([`MASTER_PLAYLIST`], [`MEDIA_PLAYLIST`]).
    playlists: Mutex<HashMap<String, String>>,
    /// Every playlist request that the gateway let through, in order.
    playlists_seen: Mutex<Vec<PlaylistSeen>>,
    /// The path of every held request whose client closed the connection before it was released.
    cancelled: Mutex<Vec<String>>,
    /// What the user marked played or favourite.
    marks: Mutex<library::Marks>,
    /// The user's libraries.
    views: Mutex<library::Views>,
    /// The generated library and its controls.
    paging: paging::Paging,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Each change is one push or one replacement: a poisoned lock still holds a usable value.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StandIn {
    /// Starts listening on a free port of 127.0.0.1, on the current tokio runtime.
    ///
    /// # Errors
    /// The port could not be bound.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
        let state = Arc::new(State::default());
        let serving = state.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, serving.clone()));
            }
        });
        Ok(Self {
            controls: Controls { state, address },
            task,
        })
    }

    /// A handle that changes the stand-in from another thread.
    pub fn controls(&self) -> Controls {
        self.controls.clone()
    }
}

impl std::ops::Deref for StandIn {
    type Target = Controls;

    fn deref(&self) -> &Controls {
        &self.controls
    }
}

impl Drop for StandIn {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Changes a running stand-in and reads what it received, from any thread.
#[derive(Clone)]
pub struct Controls {
    state: Arc<State>,
    address: String,
}

impl std::fmt::Debug for Controls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Controls").finish_non_exhaustive()
    }
}

impl Controls {
    /// Where it listens, e.g. `http://127.0.0.1:49152`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Ends every session, as an administrator revoking the device would: the next authenticated request gets 401.
    pub fn revoke_all(&self) {
        revoke_all(&self.state);
    }

    /// Turns Quick Connect off or on.
    pub fn set_quick_connect(&self, enabled: bool) {
        self.state
            .quick_connect_off
            .store(!enabled, Ordering::SeqCst);
    }

    /// While set, Quick Connect requests stay unapproved: the app keeps waiting and polling.
    pub fn set_quick_connect_pending(&self, pending: bool) {
        self.state
            .quick_connect_pending
            .store(pending, Ordering::SeqCst);
    }

    /// Requests to `path` (e.g. `/Users/AuthenticateByName`, or a [`paging::page_key`]) wait, unanswered, until
    /// [`Controls::release`].
    pub fn hold(&self, path: &str) {
        lock(&self.state.held).insert(path.to_owned());
    }

    /// Lets held requests to `path` answer.
    pub fn release(&self, path: &str) {
        lock(&self.state.held).remove(path);
        self.state.released.notify_waiters();
    }

    /// The paths of the requests received so far, oldest first.
    pub fn paths(&self) -> Vec<String> {
        lock(&self.state.paths).clone()
    }

    /// How many requests to `path` arrived so far.
    pub fn count(&self, path: &str) -> usize {
        lock(&self.state.paths)
            .iter()
            .filter(|seen| *seen == path)
            .count()
    }

    /// Puts `gate` in front of the stand-in.
    pub fn set_gate(&self, gate: Gate) {
        *lock(&self.state.gate) = gate;
    }

    /// The cookie value [`Gate::Portal`] takes from now on (the gateway's session expired and a new one began).
    pub fn set_gate_cookie(&self, value: &str) {
        *lock(&self.state.gate_cookie) = Some(value.to_owned());
    }

    /// The playstate reports received so far (`/Sessions/Playing`, `/Progress`, `/Stopped`), oldest first: path and JSON body.
    pub fn reports(&self) -> Vec<(String, String)> {
        lock(&self.state.reports).clone()
    }

    /// Marks `id` played or not on the server only, as playback or another client would, without any client being told.
    pub fn set_played_on_server(&self, id: &str, played: bool) {
        let mut marks = lock(&self.state.marks);
        if played {
            marks.played.insert(id.to_owned());
        } else {
            marks.played.remove(id);
        }
    }

    /// The image requests that passed the gateway so far, oldest first.
    pub fn images(&self) -> Vec<ImageSeen> {
        lock(&self.state.images).clone()
    }

    /// Serves `body` as the HLS playlist at `path` (e.g. [`master_playlist_path`]), to a signed-in client.
    pub fn set_playlist(&self, path: &str, body: &str) {
        lock(&self.state.playlists).insert(path.to_owned(), body.to_owned());
    }

    /// The playlist requests that passed the gateway so far, oldest first.
    pub fn playlists(&self) -> Vec<PlaylistSeen> {
        lock(&self.state.playlists_seen).clone()
    }

    /// How many held requests to `path` (or a [`paging::page_key`]) were given up by their client (the connection closed) before [`Controls::release`].
    pub fn cancelled(&self, path: &str) -> usize {
        lock(&self.state.cancelled)
            .iter()
            .filter(|seen| *seen == path)
            .count()
    }

    /// How many sessions are live.
    pub fn live_sessions(&self) -> usize {
        lock(&self.state.tokens).len()
    }

    /// Adds a library at the end of the user's list, as an administrator adding one would. `collection` is its `CollectionType`,
    /// passed as given. It holds no items.
    pub fn add_library(&self, id: &str, collection: &str, name: &str) {
        let mut views = lock(&self.state.views);
        views.hidden.remove(id);
        views.forgotten.remove(id);
        views.listed.retain(|view| view.id != id);
        views.listed.push(library::View {
            id: id.to_owned(),
            name: name.to_owned(),
            collection: collection.to_owned(),
        });
    }

    /// Removes a library from the user's list while the server still knows it: `GET Items` under it answers 401 to a valid
    /// token from now on, as Jellyfin 12.2 does for a library the user cannot see.
    pub fn remove_library(&self, id: &str) {
        let mut views = lock(&self.state.views);
        views.listed.retain(|view| view.id != id);
        views.forgotten.remove(id);
        views.hidden.insert(id.to_owned());
    }

    /// Removes a library the server then forgets: `GET Items` under it answers 400, as Jellyfin 12.2 does for an unknown parent.
    pub fn forget_library(&self, id: &str) {
        let mut views = lock(&self.state.views);
        views.listed.retain(|view| view.id != id);
        views.hidden.remove(id);
        views.forgotten.insert(id.to_owned());
    }

    /// Renames a listed library.
    pub fn rename_library(&self, id: &str, name: &str) {
        if let Some(view) = lock(&self.state.views)
            .listed
            .iter_mut()
            .find(|view| view.id == id)
        {
            name.clone_into(&mut view.name);
        }
    }

    /// Puts the listed libraries in the order of `ids` (the user's order changed); those not named follow, in their order.
    pub fn order_libraries(&self, ids: &[&str]) {
        let rank = |id: &str| {
            ids.iter()
                .position(|named| *named == id)
                .unwrap_or(usize::MAX)
        };
        lock(&self.state.views)
            .listed
            .sort_by_key(|view| rank(&view.id));
    }

    /// The ids of the user's libraries, in the user's order.
    pub fn library_ids(&self) -> Vec<String> {
        lock(&self.state.views)
            .listed
            .iter()
            .map(|view| view.id.clone())
            .collect()
    }
}

/// The value of the query parameter `key` (any case), if present.
fn param<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn revoke_all(state: &State) {
    lock(&state.tokens).clear();
}

/// One request as the stand-in read it.
struct Received {
    method: String,
    path: String,
    /// The path and query, as received.
    target: String,
    accept: String,
    /// Query pairs, decoded.
    query: Vec<(String, String)>,
    authorization: String,
    emby_authorization: String,
    cookie: String,
    gate_header: String,
    body: String,
}

async fn serve(mut socket: TcpStream, state: Arc<State>) {
    let Some(received) = read_request(&mut socket).await else {
        return;
    };
    lock(&state.paths).push(received.path.clone());
    let page = paging::key(&received.path, &received.query);
    if page.is_some() {
        paging::seen(&state.paging, &received.query);
    }
    loop {
        // Registered before the check, so a release between the two is not missed.
        let released = state.released.notified();
        let held = {
            let held = lock(&state.held);
            held.contains(&received.path) || page.as_ref().is_some_and(|page| held.contains(page))
        };
        if !held {
            break;
        }
        let mut probe = [0u8; 1];
        tokio::select! {
            () = released => {}
            // The client sent its whole request already, so a read that ends means it closed the connection: it gave up.
            read = socket.read(&mut probe) => {
                if matches!(read, Ok(0) | Err(_)) {
                    let mut cancelled = lock(&state.cancelled);
                    cancelled.push(received.path.clone());
                    cancelled.extend(page);
                    return;
                }
            }
        }
    }
    let response = match gate(&state, &received) {
        Some(blocked) => blocked,
        None if paging::failing(&state.paging, &received.path, page.as_deref()) => {
            "HTTP/1.1 500 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".to_owned()
        }
        None if is_image(&received.path) => {
            lock(&state.images).push(ImageSeen {
                target: received.target.clone(),
                accept: received.accept.clone(),
                gate_header: received.gate_header.clone(),
            });
            let head = format!(
                "HTTP/1.1 200 X\r\ncontent-type: image/png\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                IMAGE.len()
            );
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(IMAGE).await;
            let _ = socket.shutdown().await;
            return;
        }
        None if let Some(body) = playlist(&state, &received.path) => {
            lock(&state.playlists_seen).push(PlaylistSeen {
                target: received.target.clone(),
                gate_header: received.gate_header.clone(),
            });
            let (status, body) = if authorized(&state, &received) {
                (200, body)
            } else {
                (401, String::new())
            };
            format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/vnd.apple.mpegurl\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        }
        None => {
            let (status, body) = answer(&state, &received);
            format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            )
        }
    };
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// The gateway's answer when it does not let `received` through.
fn gate(state: &State, received: &Received) -> Option<String> {
    let reply = |status: u16, header: &str, body: &str| {
        format!(
            "HTTP/1.1 {status} X\r\n{header}content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    match *lock(&state.gate) {
        Gate::Off => None,
        Gate::Basic => (received.authorization != format!("Basic {GATE_BASIC}")).then(|| {
            reply(
                401,
                &format!("www-authenticate: Basic realm=\"{GATE_REALM}\"\r\n"),
                "",
            )
        }),
        Gate::Portal => {
            let value = lock(&state.gate_cookie)
                .clone()
                .unwrap_or_else(|| GATE_COOKIE_VALUE.to_owned());
            let wanted = format!("{GATE_COOKIE}={value}");
            if received.path == GATE_SIGN_IN_PATH {
                // The page's link: "signs in" by setting the cookie (HttpOnly, as portals do) and going back to the start.
                return Some(reply(
                    302,
                    &format!("set-cookie: {wanted}; Path=/; HttpOnly\r\nlocation: /\r\n"),
                    "",
                ));
            }
            (!received.cookie.split("; ").any(|pair| pair == wanted)).then(|| {
                reply(
                    200,
                    "content-type: text/html\r\n",
                    &format!(
                        "<!DOCTYPE html><html><body><a href=\"{GATE_SIGN_IN_PATH}\">Sign in</a></body></html>"
                    ),
                )
            })
        }
        Gate::Header => (received.gate_header != GATE_HEADER_VALUE).then(|| reply(403, "", "")),
    }
}

/// The playlist served at `path`, if any.
fn playlist(state: &State, path: &str) -> Option<String> {
    if let Some(body) = lock(&state.playlists).get(path) {
        return Some(body.clone());
    }
    if path == master_playlist_path() {
        Some(MASTER_PLAYLIST.to_owned())
    } else if path == media_playlist_path() {
        Some(MEDIA_PLAYLIST.to_owned())
    } else {
        None
    }
}

/// `/Items/{id}/Images/{type}` with an optional `/{index}`.
fn is_image(path: &str) -> bool {
    let parts: Vec<&str> = path.split('/').collect();
    matches!(
        parts.as_slice(),
        ["", "Items", _, "Images", _] | ["", "Items", _, "Images", _, _]
    )
}

fn answer(state: &State, received: &Received) -> (u16, String) {
    let quick_connect_on = !state.quick_connect_off.load(Ordering::SeqCst);
    match received.path.as_str() {
        "/System/Info/Public" => (200, info()),
        "/Users/AuthenticateByName"
            if received
                .body
                .contains(&format!(r#""Username":"{USER_NAME}""#))
                && received.body.contains(&format!(r#""Pw":"{PASSWORD}""#)) =>
        {
            (200, issue(state))
        }
        "/Users/AuthenticateByName" => (401, "Error processing request.".into()),
        "/QuickConnect/Enabled" => (200, quick_connect_on.to_string()),
        "/QuickConnect/Initiate" | "/QuickConnect/Connect" if !quick_connect_on => {
            (401, String::new())
        }
        "/QuickConnect/Initiate" => (200, quick_connect(false)),
        "/QuickConnect/Connect" => (
            200,
            quick_connect(!state.quick_connect_pending.load(Ordering::SeqCst)),
        ),
        "/Users/AuthenticateWithQuickConnect" if received.body.contains(QUICK_CONNECT_SECRET) => {
            (200, issue(state))
        }
        "/Users/Me" if authorized(state, received) => (200, user(state)),
        "/Sessions/Logout" if authorized(state, received) => {
            if let Some(token) = token(received) {
                lock(&state.tokens).retain(|live| *live != token);
            }
            (204, String::new())
        }
        "/Users/Me" | "/Sessions/Logout" | "/Users/AuthenticateWithQuickConnect" => {
            (401, String::new())
        }
        path if is_playback_path(path) => playback(state, received, path),
        path if library::is_browse_path(path) && !authorized(state, received) => {
            (401, String::new())
        }
        "/Items"
            if let Some(status) =
                lock(&state.views).refusal(param(&received.query, "parentId")) =>
        {
            (status, String::new())
        }
        "/Items" if paging::key(&received.path, &received.query).is_some() => {
            paging::answer(&state.paging, &received.query)
        }
        path => {
            let request = library::Browse {
                method: &received.method,
                path,
                query: &received.query,
            };
            library::answer(&request, &mut lock(&state.marks), &lock(&state.views))
                .unwrap_or((404, String::new()))
        }
    }
}

/// Whether `path` is one of playback's endpoints: PlaybackInfo, media segments, the session list and playstate reports.
fn is_playback_path(path: &str) -> bool {
    (path.starts_with("/Items/") && path.ends_with("/PlaybackInfo"))
        || path.starts_with("/MediaSegments/")
        || path == "/Sessions"
        || path.starts_with("/Sessions/Playing")
        || [DIRECT_PLAY_ITEM, TRANSCODE_ITEM]
            .iter()
            .any(|item| path == format!("/Items/{item}") || subtitle_item(path) == Some(*item))
}

/// The item of a subtitle request: `/Videos/{item}/{MEDIA_SOURCE}/Subtitles/{index}/Stream.{format}`.
fn subtitle_item(path: &str) -> Option<&str> {
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        ["", "Videos", item, source, "Subtitles", _, file]
            if *source == MEDIA_SOURCE && file.starts_with("Stream.") =>
        {
            Some(item)
        }
        _ => None,
    }
}

/// Playback's endpoints, for a signed-in caller.
fn playback(state: &State, received: &Received, path: &str) -> (u16, String) {
    if !authorized(state, received) {
        return (401, String::new());
    }
    match path {
        "/Sessions/Playing" | "/Sessions/Playing/Progress" | "/Sessions/Playing/Stopped" => {
            lock(&state.reports).push((path.to_owned(), received.body.clone()));
            (204, String::new())
        }
        "/Sessions" => (200, sessions(received)),
        // The library's episodes play directly too, for the next-episode autoplay.
        _ if path == format!("/Items/{DIRECT_PLAY_ITEM}/PlaybackInfo")
            || library::EPISODE_IDS
                .iter()
                .any(|episode| path == format!("/Items/{episode}/PlaybackInfo")) =>
        {
            (200, playback_info(r#""SupportsDirectPlay":true"#.into()))
        }
        _ if path == format!("/Items/{TRANSCODE_ITEM}/PlaybackInfo") => {
            let token = token(received).unwrap_or_default();
            (
                200,
                playback_info(format!(
                    r#""SupportsDirectPlay":false,"TranscodingUrl":"/videos/{TRANSCODE_ITEM}/master.m3u8?DeviceId=d&MediaSourceId={MEDIA_SOURCE}&VideoCodec=hevc,h264&AudioCodec=aac,ac3,eac3&AudioStreamIndex=1&PlaySessionId={PLAY_SESSION}&ApiKey={token}&SegmentContainer=mp4&TranscodeReasons=ContainerNotSupported","TranscodingSubProtocol":"hls","TranscodingContainer":"mp4""#
                )),
            )
        }
        _ if path == format!("/MediaSegments/{DIRECT_PLAY_ITEM}") => (200, segments()),
        _ if path == format!("/Items/{DIRECT_PLAY_ITEM}") => {
            (200, film(DIRECT_PLAY_ITEM, DIRECT_PLAY_NAME))
        }
        _ if path == format!("/Items/{TRANSCODE_ITEM}") => {
            (200, film(TRANSCODE_ITEM, TRANSCODE_NAME))
        }
        _ if subtitle_item(path).is_some() => (200, SUBTITLE.to_owned()),
        _ => (404, String::new()),
    }
}

/// A PlaybackInfo answer with one HEVC/AAC Matroska source; `route` sets how it can be played.
fn playback_info(route: String) -> String {
    format!(
        r#"{{"MediaSources":[{{"Protocol":"File","Id":"{MEDIA_SOURCE}","Path":"/media/stand-in/file.mkv","Type":"Default","Container":"mkv","Size":104857600,"Name":"Stand-in 1080p","IsRemote":false,"ETag":"5ad01e","RunTimeTicks":6000000000,"SupportsTranscoding":true,"SupportsDirectStream":false,{route},"MediaStreams":[{{"Codec":"hevc","DisplayTitle":"1080p HEVC SDR","BitDepth":8,"Height":1080,"Width":1920,"RealFrameRate":24,"VideoRange":"SDR","VideoRangeType":"SDR","Profile":"Main","Type":"Video","Index":0,"IsDefault":true,"IsForced":false,"IsHearingImpaired":false,"IsExternal":false,"Level":120}},{{"Codec":"aac","Language":"eng","DisplayTitle":"English - AAC - Stereo - Default","ChannelLayout":"stereo","Channels":2,"SampleRate":48000,"Type":"Audio","Index":1,"IsDefault":true,"IsForced":false,"IsHearingImpaired":false,"IsExternal":false}}],"Bitrate":1398101,"DefaultAudioStreamIndex":1,"HasSegments":true}}],"PlaySessionId":"{PLAY_SESSION}"}}"#
    )
}

/// A playback item's details: a film with a primary image.
fn film(id: &str, name: &str) -> String {
    format!(
        r#"{{"Name":"{name}","ServerId":"{SERVER_ID}","Id":"{id}","Type":"Movie","MediaType":"Video","RunTimeTicks":6000000000,"ProductionYear":2026,"ImageTags":{{"Primary":"5ad01e"}},"ImageBlurHashes":{{"Primary":{{"5ad01e":"LEHV6nWB2yk8pyo0adR*.7kCMdnj"}}}},"UserData":{{"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":false,"Played":false,"Key":"{id}","ItemId":"{id}"}}}}"#
    )
}

fn segments() -> String {
    format!(
        r#"{{"Items":[{{"Id":"57a2d1e0000000000000000000000020","ItemId":"{DIRECT_PLAY_ITEM}","Type":"Intro","StartTicks":50000000,"EndTicks":350000000}},{{"Id":"57a2d1e0000000000000000000000021","ItemId":"{DIRECT_PLAY_ITEM}","Type":"Outro","StartTicks":5500000000,"EndTicks":6000000000}}],"TotalRecordCount":2,"StartIndex":0}}"#
    )
}

/// The caller's session, with a transcode in progress: the device id comes from the caller's own authorization fields.
fn sessions(received: &Received) -> String {
    let fields = if received.authorization.starts_with("MediaBrowser") {
        &received.authorization
    } else {
        &received.emby_authorization
    };
    let device = fields
        .split_once("DeviceId=\"")
        .and_then(|(_, rest)| rest.split('"').next())
        .unwrap_or_default();
    format!(
        r#"[{{"Id":"57a2d1e0000000000000000000000030","UserId":"{USER_ID}","Client":"Current","DeviceId":"{device}","TranscodingInfo":{{"AudioCodec":"aac","VideoCodec":"hevc","Container":"mp4","IsVideoDirect":true,"IsAudioDirect":true,"Bitrate":1398101,"Width":1920,"Height":1080,"AudioChannels":2,"HardwareAccelerationType":"none","TranscodeReasons":["ContainerNotSupported"]}}}}]"#
    )
}

fn info() -> String {
    format!(
        r#"{{"ServerName":"{SERVER_NAME}","Version":"12.2.0","ProductName":"Jellyfin Server","Id":"{SERVER_ID}","StartupWizardCompleted":true}}"#
    )
}

fn user(state: &State) -> String {
    let excludes: Vec<String> = lock(&state.paging.latest_excludes)
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect();
    let excludes = excludes.join(",");
    format!(
        r#"{{"Name":"{USER_NAME}","Id":"{USER_ID}","Configuration":{{"LatestItemsExcludes":[{excludes}],"AudioLanguagePreference":"eng","PlayDefaultAudioTrack":true,"SubtitleLanguagePreference":"","SubtitleMode":"Default","RememberAudioSelections":true,"RememberSubtitleSelections":true,"EnableNextEpisodeAutoPlay":true}},"Policy":{{"IsAdministrator":false,"EnableMediaPlayback":true,"EnableVideoPlaybackTranscoding":true,"EnablePlaybackRemuxing":false,"ForceRemoteSourceTranscoding":false,"RemoteClientBitrateLimit":{REMOTE_BITRATE_LIMIT}}}}}"#
    )
}

/// A new token. As Jellyfin does for one device id, the earlier one is logged out.
fn issue(state: &State) -> String {
    let token = format!(
        "57a2d1e0t0ken{:019}",
        state.issued.fetch_add(1, Ordering::SeqCst)
    );
    *lock(&state.tokens) = vec![token.clone()];
    format!(
        r#"{{"User":{},"SessionInfo":{{}},"AccessToken":"{token}","ServerId":"{SERVER_ID}"}}"#,
        user(state)
    )
}

fn quick_connect(authenticated: bool) -> String {
    format!(
        r#"{{"Authenticated":{authenticated},"Secret":"{QUICK_CONNECT_SECRET}","Code":"{QUICK_CONNECT_CODE}","DeviceId":"d","DeviceName":"Example PC","AppName":"Current","AppVersion":"0.1.0","DateAdded":"2026-10-22T10:00:00Z"}}"#
    )
}

fn token(received: &Received) -> Option<String> {
    // Jellyfin reads `X-Emby-Authorization` when `Authorization` carries no fields of its own (legacy authorization on, and a Basic
    // gateway that does not pass its header on).
    let fields = if received.authorization.starts_with("MediaBrowser") {
        &received.authorization
    } else {
        &received.emby_authorization
    };
    let (_, rest) = fields.split_once("Token=\"")?;
    Some(rest.split('"').next()?.to_owned())
}

fn authorized(state: &State, received: &Received) -> bool {
    token(received).is_some_and(|token| lock(&state.tokens).contains(&token))
}

async fn read_request(socket: &mut TcpStream) -> Option<Received> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break end + 4;
        }
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => return None,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let header = |name: &str| {
        head.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_owned())
            })
            .unwrap_or_default()
    };
    let length: usize = header("content-length").parse().unwrap_or(0);
    while data.len() < head_end + length {
        match socket.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => data.extend_from_slice(&buffer[..read]),
        }
    }
    let mut first = head.lines().next()?.split(' ');
    let method = first.next()?.to_owned();
    let target = first.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    Some(Received {
        method,
        path: path.to_owned(),
        target: target.to_owned(),
        accept: header("accept"),
        query: query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
                (decode(key), decode(value))
            })
            .collect(),
        authorization: header("authorization"),
        emby_authorization: header("x-emby-authorization"),
        cookie: header("cookie"),
        gate_header: header(GATE_HEADER),
        body: String::from_utf8_lossy(&data[head_end..]).into_owned(),
    })
}

/// A query component with `%XX` and `+` decoded. A broken escape is kept as it is.
fn decode(component: &str) -> String {
    let bytes = component.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => decoded.push(b' '),
            b'%' => match component
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
            {
                Some(byte) => {
                    decoded.push(byte);
                    i += 2;
                }
                None => decoded.push(b'%'),
            },
            byte => decoded.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}
