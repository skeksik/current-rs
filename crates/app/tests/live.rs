//! Live updates of user state (ADR 0023) through `App` against the Jellyfin stand-in's WebSocket: a mark made on another device
//! shows on every page without a refresh; the socket opens again after it drops and reads what changed meanwhile; it answers the
//! server's keep-alive, ignores remote control, never declares the session controllable, carries the gateway's header, and does
//! not retry a refusal in a loop.

use std::time::Duration;

use current_app::{AddServerState, App, Browse, HeaderInput, PagedItems, SignInState, Slot};
use current_model::{ItemId, ItemQuery, ItemRef, Library, ServerRef, UserState};
use jellyfin_stand_in::library::{FILM_IDS, FILMS_LIBRARY_ID};
use jellyfin_stand_in::{GATE_HEADER, GATE_HEADER_VALUE, Gate, PASSWORD, StandIn, USER_NAME};

/// Waits (at most 5 s) until `check` passes.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("not within 5 s: {what}");
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
