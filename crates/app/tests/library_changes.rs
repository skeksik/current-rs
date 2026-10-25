//! Libraries that change on the server while the app runs (found on the owner's run, 2026-10-25), end to end through `App` against
//! the Jellyfin stand-in: the navigation's library list follows a library added, renamed, reordered and removed; a library request
//! that gets 401 with a token that still works answers "not available" and the session stays (ADR 0015 as amended 2026-10-25),
//! while a revoked token still signs out and a gateway's answer still asks for the gateway sign-in; while the token is checked,
//! one probe runs and other requests are held back.

use std::time::Duration;

use current_app::{AddServerState, App, Browse, PagedItems, SignInState, Slot};
use current_jellyfin::AuthError;
use current_model::{ItemQuery, Library, ServerRef};
use jellyfin_stand_in::library::{FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID};
use jellyfin_stand_in::{Gate, PASSWORD, StandIn, USER_NAME};

/// A library the tests add. Its kind is one Current does not know.
const ADDED_LIBRARY_ID: &str = "57a2d1e0000000000000000000001003";

/// Waits (at most 5 s) until `check` passes.
async fn eventually(mut check: impl FnMut() -> bool) {
    for _ in 0..500 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not met within 5 s");
}

/// Sets up and signs in to the stand-in; returns the server.
async fn signed_in(stand_in: &StandIn, app: &App) -> ServerRef {
    let setup = app.add_server();
    setup.probe(stand_in.address()).await;
    let found = match setup.state().borrow().clone() {
        AddServerState::Ready { server } => server,
        state => panic!("{state:?}"),
    };
    let flow = app.sign_in(found).unwrap();
    flow.with_password(USER_NAME, PASSWORD).await;
    let state = flow.state().borrow().clone();
    assert!(matches!(state, SignInState::SignedIn { .. }), "{state:?}");
    eventually(|| app.capabilities().borrow().servers[0].address.is_some()).await;
    app.capabilities().borrow().servers[0].server.clone()
}

/// The ids in the browse's library list, in order; empty until loaded.
fn listed(browse: &Browse) -> Vec<String> {
    browse
        .library_list()
        .borrow()
        .iter()
        .flatten()
        .map(|library| library.item.id.as_str().to_owned())
        .collect()
}

fn library(browse: &Browse, id: &str) -> Library {
    browse
        .library_list()
        .borrow()
        .iter()
        .flatten()
        .find(|library| library.item.id.as_str() == id)
        .cloned()
        .unwrap()
}

/// A grid of `id`'s library whose first page has loaded.
async fn loaded_grid(browse: &Browse, id: &str) -> PagedItems {
    let grid = browse.open_library(&library(browse, id), ItemQuery::default());
    let state = grid.state();
    eventually(|| !state.borrow().loading && state.borrow().failed.is_none()).await;
    assert!(matches!(grid.slots().get(0), Some(Slot::Item(_))));
    grid
}

async fn failed_with(grid: &PagedItems, problem: AuthError) {
    let state = grid.state();
    eventually(|| state.borrow().failed.as_ref() == Some(&problem)).await;
}

fn signed_in_now(app: &App) -> bool {
    app.capabilities().borrow().servers[0].signed_in
}

#[tokio::test]
async fn the_library_list_follows_the_server() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    // Loaded at once, in the user's order.
    eventually(|| listed(&browse) == [FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]).await;

    // Added on the server: there after a refresh, its unknown kind passed through.
    stand_in.add_library(ADDED_LIBRARY_ID, "examplekind", "Example Added");
    browse.refresh();
    eventually(|| listed(&browse).len() == 3).await;
    let added = library(&browse, ADDED_LIBRARY_ID);
    assert_eq!(added.name, "Example Added");
    assert_eq!(
        added.collection.as_ref().map(|kind| kind.as_str()),
        Some("examplekind")
    );

    // Renamed and reordered: there on return to the foreground.
    stand_in.rename_library(SHOWS_LIBRARY_ID, "Example Shows Renamed");
    stand_in.order_libraries(&[SHOWS_LIBRARY_ID, ADDED_LIBRARY_ID, FILMS_LIBRARY_ID]);
    app.returned_to_foreground();
    eventually(|| listed(&browse) == [SHOWS_LIBRARY_ID, ADDED_LIBRARY_ID, FILMS_LIBRARY_ID]).await;
    assert_eq!(
        library(&browse, SHOWS_LIBRARY_ID).name,
        "Example Shows Renamed"
    );

    // Removed: gone after Home reloads, with one request for Home's rows and the list together.
    stand_in.remove_library(ADDED_LIBRARY_ID);
    let home = browse.home();
    eventually(|| listed(&browse) == [SHOWS_LIBRARY_ID, FILMS_LIBRARY_ID]).await;
    let views = stand_in.count("/UserViews");
    home.refresh();
    eventually(|| !home.state().borrow().loading).await;
    assert_eq!(stand_in.count("/UserViews"), views + 1);
    assert!(signed_in_now(&app));
}

#[tokio::test]
async fn a_removed_library_answers_not_available_and_the_session_stays() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    eventually(|| listed(&browse).len() == 2).await;
    let films = library(&browse, FILMS_LIBRARY_ID);
    let grid = loaded_grid(&browse, FILMS_LIBRARY_ID).await;
    let probes = stand_in.count("/System/Info/Public");
    let checks = stand_in.count("/Users/Me");

    // The server still knows the library but no longer lists it: `GET Items` under it answers 401 to a valid token.
    stand_in.remove_library(FILMS_LIBRARY_ID);
    grid.refresh();
    failed_with(&grid, AuthError::NotAvailable).await;
    // The identity probe found the server, and Jellyfin said who the token belongs to: the session stays.
    assert_eq!(stand_in.count("/System/Info/Public"), probes + 1);
    assert_eq!(stand_in.count("/Users/Me"), checks + 1);
    assert!(signed_in_now(&app));
    assert_eq!(stand_in.live_sessions(), 1);
    // The libraries were read again before the failure showed.
    assert_eq!(listed(&browse), [SHOWS_LIBRARY_ID]);
    // Loaded items stay; requests go out again.
    assert!(matches!(grid.slots().get(0), Some(Slot::Item(_))));
    let shows = loaded_grid(&browse, SHOWS_LIBRARY_ID).await;
    assert_eq!(shows.state().borrow().failed, None);

    // Its stale entry opened before any refresh: the same, and still signed in.
    let stale = browse.open_library(&films, ItemQuery::default());
    failed_with(&stale, AuthError::NotAvailable).await;
    assert!(signed_in_now(&app));
    assert!(app.browse(&server).is_some());
}

#[tokio::test]
async fn a_forgotten_library_answers_not_available_without_a_probe() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    eventually(|| listed(&browse).len() == 2).await;
    let grid = loaded_grid(&browse, SHOWS_LIBRARY_ID).await;
    let probes = stand_in.count("/System/Info/Public");

    // Jellyfin answers 400 for a parent it does not know: the libraries are read again, and the grid says the same.
    stand_in.forget_library(SHOWS_LIBRARY_ID);
    grid.refresh();
    failed_with(&grid, AuthError::NotAvailable).await;
    assert_eq!(listed(&browse), [FILMS_LIBRARY_ID]);
    assert_eq!(stand_in.count("/System/Info/Public"), probes);
    assert!(signed_in_now(&app));
}

#[tokio::test]
async fn a_revoked_token_still_signs_out() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    eventually(|| listed(&browse).len() == 2).await;
    let grid = loaded_grid(&browse, FILMS_LIBRARY_ID).await;

    // An administrator revoked the device's session: the token check gets 401 too.
    stand_in.revoke_all();
    grid.refresh();
    failed_with(&grid, AuthError::SessionRevoked).await;
    eventually(|| !signed_in_now(&app)).await;
    assert!(app.browse(&server).is_none());
}

#[tokio::test]
async fn a_gateway_s_answer_still_asks_for_the_gateway_sign_in() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    eventually(|| listed(&browse).len() == 2).await;
    let grid = loaded_grid(&browse, FILMS_LIBRARY_ID).await;

    // A portal now answers every request, the probe's too, with its sign-in page.
    stand_in.set_gate(Gate::Portal);
    grid.refresh();
    eventually(|| app.capabilities().borrow().servers[0].gateway_sign_in_needed).await;
    assert!(signed_in_now(&app));
    assert_eq!(stand_in.live_sessions(), 1);
    // Not taken for a library that is gone.
    assert_eq!(listed(&browse), [FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]);
}

#[tokio::test]
async fn while_the_token_is_checked_one_probe_runs_and_other_requests_wait() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    eventually(|| listed(&browse).len() == 2).await;
    let films = loaded_grid(&browse, FILMS_LIBRARY_ID).await;
    let shows_library = library(&browse, SHOWS_LIBRARY_ID);
    let probes = stand_in.count("/System/Info/Public");
    let checks = stand_in.count("/Users/Me");

    stand_in.hold("/Users/Me");
    stand_in.remove_library(FILMS_LIBRARY_ID);
    films.refresh();
    eventually(|| stand_in.count("/Users/Me") == checks + 1).await;

    // Meanwhile another grid's request is held back: it never reaches the server.
    let items = stand_in.count("/Items");
    let shows = browse.open_library(&shows_library, ItemQuery::default());
    failed_with(&shows, AuthError::Unconfirmed).await;
    assert_eq!(stand_in.count("/Items"), items);
    assert_eq!(films.state().borrow().failed, None);

    stand_in.release("/Users/Me");
    failed_with(&films, AuthError::NotAvailable).await;
    assert_eq!(stand_in.count("/System/Info/Public"), probes + 1);
    assert!(signed_in_now(&app));
    // Requests go out again.
    shows.retry();
    let state = shows.state();
    eventually(|| !state.borrow().loading && state.borrow().failed.is_none()).await;
}
