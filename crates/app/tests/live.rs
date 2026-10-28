//! Live updates of user state (ADR 0023) through `App` against the Jellyfin stand-in's WebSocket: a mark made on another device
//! shows on every page without a refresh; the socket opens again after it drops and reads what changed meanwhile; it answers the
//! server's keep-alive, ignores remote control, never declares the session controllable, carries the gateway's header, and does
//! not retry a refusal in a loop; it closes once the window has been minimized for a minute and opens again on its restore.

use std::time::Duration;

use current_app::{
    AddServerState, App, Browse, HeaderInput, LIVE_PAUSE_AFTER, PagedItems, SignInState, Slot,
};
use current_model::{ItemId, ItemQuery, ItemRef, Library, ServerRef, UserState};
use jellyfin_stand_in::library::{FILM_IDS, FILMS_LIBRARY_ID};
use jellyfin_stand_in::{GATE_HEADER, GATE_HEADER_VALUE, Gate, PASSWORD, StandIn, USER_NAME};

/// Waits (at most 5 s) until `check` passes.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        real_wait(Duration::from_millis(10)).await;
    }
    panic!("not within 5 s: {what}");
}

/// Waits `time` of real time, also while tokio's clock is stopped ([`tokio::time::pause`]): a blocking task holds the stopped clock
/// where it is, while the sockets' traffic, some of which runs on other threads, goes on.
async fn real_wait(time: Duration) {
    tokio::task::spawn_blocking(move || std::thread::sleep(time))
        .await
        .unwrap();
}

/// Sets up and signs in to the stand-in (with the gateway's header when it asks for one), and waits until the session has an
/// address.
async fn signed_in(stand_in: &StandIn, app: &App) -> ServerRef {
    let setup = app.add_server();
    setup.probe(stand_in.address()).await;
    if matches!(
        *setup.state().borrow(),
        AddServerState::GatewayRefused { .. }
    ) {
        setup
            .submit_headers(
                &[HeaderInput {
                    name: GATE_HEADER.into(),
                    value: GATE_HEADER_VALUE.into(),
                }],
                false,
            )
            .await
            .unwrap();
    }
    let found = match setup.state().borrow().clone() {
        AddServerState::Ready { server } => server,
        state => panic!("{state:?}"),
    };
    let flow = app.sign_in(found).unwrap();
    flow.with_password(USER_NAME, PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(matches!(state, SignInState::SignedIn { .. }), "{state:?}");
    eventually("an address", || {
        app.capabilities().borrow().servers[0].address.is_some()
    })
    .await;
    app.capabilities().borrow().servers[0].server.clone()
}

fn item(server: &ServerRef, id: &str) -> ItemRef {
    ItemRef::new(server.clone(), ItemId::new(id))
}

fn films(browse: &Browse, server: &ServerRef) -> PagedItems {
    let library = Library {
        item: item(server, FILMS_LIBRARY_ID),
        name: "Example Library".into(),
        collection: None,
        artwork: Vec::new(),
    };
    browse.open_library(&library, ItemQuery::default())
}

fn in_grid(grid: &PagedItems, item: &ItemRef) -> Option<UserState> {
    grid.slots()
        .snapshot()
        .into_iter()
        .find_map(|slot| match slot {
            Slot::Item(summary) if &summary.item == item => Some(summary.user),
            _ => None,
        })
}

fn latest(home: &current_app::HomeFeed, item: &ItemRef) -> Option<UserState> {
    home.row(&format!("latest:{FILMS_LIBRARY_ID}"))?
        .snapshot()
        .into_iter()
        .find(|summary| &summary.item == item)
        .map(|summary| summary.user)
}

#[tokio::test]
async fn a_mark_made_elsewhere_shows_on_every_page_without_a_refresh() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let film = item(&server, FILM_IDS[0]);
    let home = browse.home();
    let grid = films(&browse, &server);
    let page = browse.details(&film);
    eventually("every page", || {
        latest(&home, &film).is_some()
            && in_grid(&grid, &film).is_some()
            && page.details().borrow().is_some()
    })
    .await;
    eventually("the socket", || stand_in.socket_count() == 1).await;
    // The server answers the keep-alive the client sends at once.
    eventually("a keep-alive", || {
        stand_in
            .socket_texts()
            .iter()
            .any(|text| text.contains(r#""MessageType":"KeepAlive""#))
    })
    .await;
    // Remote control commands are ignored: the next message still applies.
    stand_in.send_socket(r#"{"MessageType":"Play","Data":{"ItemIds":["57a2d1e0000000000000000000002001"],"PlayCommand":"PlayNow"}}"#);
    stand_in.send_socket(
        r#"{"MessageType":"GeneralCommand","Data":{"Name":"DisplayMessage","Arguments":{}}}"#,
    );

    stand_in.set_played_on_server(FILM_IDS[0], true);
    let played = |state: Option<UserState>| state.is_some_and(|state| state.played);
    eventually("every page shows the mark", || {
        played(latest(&home, &film))
            && played(in_grid(&grid, &film))
            && played(
                page.details()
                    .borrow()
                    .as_ref()
                    .map(|details| details.summary.user.clone()),
            )
    })
    .await;
    assert_eq!(
        browse.user_state(&film).map(|state| state.played),
        Some(true)
    );
    // The session never says it can be controlled (ADR 0023): no capabilities are posted.
    assert!(
        !stand_in
            .paths()
            .iter()
            .any(|path| path.starts_with("/Sessions/Capabilities")),
        "{:?}",
        stand_in.paths()
    );
}

#[tokio::test]
async fn a_dropped_socket_opens_again_and_reads_what_changed_meanwhile() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let film = item(&server, FILM_IDS[2]);
    let grid = films(&browse, &server);
    eventually("the grid", || in_grid(&grid, &film).is_some()).await;
    eventually("the socket", || stand_in.socket_count() == 1).await;

    // Dropped, and the film marked while no socket is open: no push reaches the client.
    stand_in.drop_sockets();
    stand_in.set_played_on_server(FILM_IDS[2], true);
    eventually("the socket again", || stand_in.socket_count() == 1).await;
    eventually("the grid shows the mark", || {
        in_grid(&grid, &film).is_some_and(|state| state.played)
    })
    .await;
}

#[tokio::test]
async fn a_refused_socket_waits_for_the_foreground() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let _grid = films(&browse, &server);
    eventually("the socket", || stand_in.socket_count() == 1).await;
    let opened = || {
        stand_in
            .paths()
            .iter()
            .filter(|path| *path == "/socket")
            .count()
    };
    assert_eq!(opened(), 1);

    // The token no longer works: the socket that drops is refused (403) when it opens again, and not asked for again by itself.
    stand_in.revoke_all();
    stand_in.drop_sockets();
    eventually("the refused attempt", || opened() == 2).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(opened(), 2, "no retry loop");
    assert_eq!(stand_in.socket_count(), 0);
}

#[tokio::test]
async fn the_socket_carries_the_gateway_s_header() {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_gate(Gate::Header);
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let film = item(&server, FILM_IDS[0]);
    let grid = films(&browse, &server);
    eventually("the grid", || in_grid(&grid, &film).is_some()).await;
    // The gate answers 403 without the header: the socket opened through it.
    eventually("the socket", || stand_in.socket_count() == 1).await;
    stand_in.set_played_on_server(FILM_IDS[0], true);
    eventually("the push", || {
        in_grid(&grid, &film).is_some_and(|state| state.played)
    })
    .await;
}

/// The live updates' first wait before a reconnect (ADR 0023, decision 5).
const MIN_BACKOFF: Duration = Duration::from_secs(1);

/// How many times the client asked the stand-in for a socket.
fn socket_requests(stand_in: &StandIn) -> usize {
    stand_in
        .paths()
        .iter()
        .filter(|path| *path == "/socket")
        .count()
}

/// Lets `time` pass on tokio's stopped clock a second at a time, with the sockets' traffic (the keep-alives) in between: one jump
/// would outrun the server's answers and the socket would be taken for lost.
async fn pass(time: Duration) {
    let mut left = time;
    while !left.is_zero() {
        let step = left.min(Duration::from_secs(1));
        tokio::time::advance(step).await;
        left -= step;
        real_wait(Duration::from_millis(1)).await;
    }
}

/// Signs in, opens a grid holding `film`, and waits for its socket; then stops tokio's clock, so the minute the window stays
/// minimized is not a real one ([`tokio::time::advance`]).
async fn minimizable(stand_in: &StandIn, app: &App, film: &str) -> (Browse, PagedItems, ItemRef) {
    let server = signed_in(stand_in, app).await;
    let browse = app.browse(&server).unwrap();
    let film = item(&server, film);
    let grid = films(&browse, &server);
    eventually("the grid", || in_grid(&grid, &film).is_some()).await;
    eventually("the socket", || stand_in.socket_count() == 1).await;
    assert_eq!(socket_requests(stand_in), 1);
    tokio::time::pause();
    (browse, grid, film)
}

#[tokio::test]
async fn a_window_minimized_for_a_minute_closes_the_socket_and_its_restore_opens_it_again() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let (_browse, grid, film) = minimizable(&stand_in, &app, FILM_IDS[1]).await;

    app.window_minimized(true);
    pass(LIVE_PAUSE_AFTER - Duration::from_secs(1)).await;
    assert_eq!(stand_in.socket_count(), 1, "still open within the delay");
    pass(Duration::from_secs(2)).await;
    eventually("the socket closes", || stand_in.socket_count() == 0).await;

    // Marked while it is closed: no push reaches the client, and no attempt is made however long the window stays minimized.
    stand_in.set_played_on_server(FILM_IDS[1], true);
    pass(Duration::from_secs(30 * 60)).await;
    assert_eq!(socket_requests(&stand_in), 1, "no attempt while minimized");
    assert_eq!(stand_in.socket_count(), 0);
    assert!(in_grid(&grid, &film).is_some_and(|state| !state.played));

    // Restored: it opens at once, and the resync shows the mark.
    app.window_minimized(false);
    eventually("the socket again", || stand_in.socket_count() == 1).await;
    assert_eq!(socket_requests(&stand_in), 2);
    eventually("the grid shows the mark", || {
        in_grid(&grid, &film).is_some_and(|state| state.played)
    })
    .await;
}

#[tokio::test]
async fn a_quick_minimize_and_restore_keeps_the_socket() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let (_browse, grid, film) = minimizable(&stand_in, &app, FILM_IDS[0]).await;

    app.window_minimized(true);
    pass(LIVE_PAUSE_AFTER - Duration::from_secs(1)).await;
    app.window_minimized(false);
    // The minute counts from the minimize: once restored, the time passing changes nothing.
    pass(LIVE_PAUSE_AFTER * 3).await;
    // A second minimize counts afresh.
    app.window_minimized(true);
    pass(LIVE_PAUSE_AFTER / 2).await;
    app.window_minimized(false);
    pass(LIVE_PAUSE_AFTER).await;
    assert_eq!(stand_in.socket_count(), 1);
    assert_eq!(socket_requests(&stand_in), 1, "the same socket");

    // And it still carries pushes.
    stand_in.set_played_on_server(FILM_IDS[0], true);
    eventually("the push", || {
        in_grid(&grid, &film).is_some_and(|state| state.played)
    })
    .await;
    assert_eq!(socket_requests(&stand_in), 1);
}

#[tokio::test]
async fn a_socket_opened_again_while_minimized_closes_a_minute_after_the_minimize() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let (_browse, _grid, _film) = minimizable(&stand_in, &app, FILM_IDS[0]).await;

    // Dropped within the delay: it opens again after its backoff, as any time.
    app.window_minimized(true);
    pass(LIVE_PAUSE_AFTER / 2).await;
    stand_in.drop_sockets();
    // Its backoff is at most a second, once the client has seen it close.
    let mut waited = Duration::ZERO;
    while !(socket_requests(&stand_in) == 2 && stand_in.socket_count() == 1) {
        assert!(waited < MIN_BACKOFF * 5, "the socket again");
        real_wait(Duration::from_millis(10)).await;
        pass(Duration::from_millis(100)).await;
        waited += Duration::from_millis(100);
    }
    // The minute counts from the minimize, not from that opening.
    pass(LIVE_PAUSE_AFTER / 2 - waited - Duration::from_secs(1)).await;
    assert_eq!(stand_in.socket_count(), 1, "still open within the minute");
    pass(Duration::from_secs(2)).await;
    eventually("the socket closes", || stand_in.socket_count() == 0).await;
    pass(Duration::from_secs(30 * 60)).await;
    assert_eq!(socket_requests(&stand_in), 2, "no attempt while minimized");

    app.window_minimized(false);
    eventually("the socket again", || stand_in.socket_count() == 1).await;
    assert_eq!(socket_requests(&stand_in), 3);
}
