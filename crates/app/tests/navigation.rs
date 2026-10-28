//! The user's arrangement of the pane and Home (ADR 0024), end to end through `App` against the Jellyfin stand-in: the pane's
//! layout starts in the server's order, follows a move, a hide and a reset; a library kept out of "My media" shows hidden until the
//! user shows it; libraries the user never moved follow the server's order; Home's rows take the order and lose the hidden rows,
//! and a row shown again loads.

use std::time::Duration;

use current_app::{AddServerState, App, Browse, HomeFeed, NavGroup, PaneLayout, SignInState};
use current_model::ServerRef;
use jellyfin_stand_in::library::{FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID};
use jellyfin_stand_in::{PASSWORD, StandIn, USER_NAME};

/// A library the tests add, kept out of "My media".
const HIDDEN_LIBRARY_ID: &str = "57a2d1e0000000000000000000001004";

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

fn layout(browse: &Browse) -> Option<PaneLayout> {
    browse.pane().borrow().clone()
}

/// The library ids of the pane, in order, `!` before a hidden one.
fn libraries(browse: &Browse) -> Vec<String> {
    layout(browse)
        .map(|layout| {
            layout
                .libraries
                .iter()
                .map(|entry| {
                    let id = entry.library.item.id.as_str();
                    if entry.hidden {
                        format!("!{id}")
                    } else {
                        id.to_owned()
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The keys of a group of the pane, `!` before a hidden one.
fn keys(entries: &[current_app::PaneEntry]) -> Vec<String> {
    entries
        .iter()
        .map(|entry| {
            if entry.hidden {
                format!("!{}", entry.key)
            } else {
                entry.key.clone()
            }
        })
        .collect()
}

fn rows(home: &HomeFeed) -> Vec<String> {
    home.rows()
        .snapshot()
        .into_iter()
        .map(|row| row.key)
        .collect()
}

#[tokio::test]
async fn the_pane_follows_the_users_arrangement() {
    let stand_in = StandIn::start().await.unwrap();
    // Read at sign-in, as "exclude from latest" is.
    stand_in.set_my_media_excludes(&[HIDDEN_LIBRARY_ID]);
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();

    // The defaults: the server's order, the app's tabs and views.
    eventually(|| libraries(&browse) == [FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]).await;
    let pane = layout(&browse).unwrap();
    assert_eq!(keys(&pane.tabs), ["movie", "series"]);
    assert_eq!(keys(&pane.views), ["favourites", "recently_added"]);

    // Moved and hidden.
    browse
        .move_item(NavGroup::Libraries, SHOWS_LIBRARY_ID, 0)
        .unwrap();
    eventually(|| libraries(&browse) == [SHOWS_LIBRARY_ID, FILMS_LIBRARY_ID]).await;
    browse.set_hidden(NavGroup::Tabs, "series", true).unwrap();
    browse
        .move_item(NavGroup::Views, "recently_added", 0)
        .unwrap();
    eventually(|| {
        layout(&browse).is_some_and(|pane| {
            keys(&pane.tabs) == ["movie", "!series"]
                && keys(&pane.views) == ["recently_added", "favourites"]
        })
    })
    .await;

    // A library kept out of "My media" comes hidden, last (the server lists it after those in its order), until shown here.
    stand_in.add_library(HIDDEN_LIBRARY_ID, "homevideos", "Example Hidden");
    browse.refresh();
    let hidden = format!("!{HIDDEN_LIBRARY_ID}");
    eventually(|| libraries(&browse) == [SHOWS_LIBRARY_ID, FILMS_LIBRARY_ID, hidden.as_str()])
        .await;
    assert!(
        layout(&browse).unwrap().libraries[2].hidden_on_server,
        "the editor says why it starts hidden"
    );
    browse
        .set_hidden(NavGroup::Libraries, HIDDEN_LIBRARY_ID, false)
        .unwrap();
    eventually(|| libraries(&browse)[2] == HIDDEN_LIBRARY_ID).await;

    // The server's order changes: the unmoved libraries follow it, the moved one keeps its place.
    stand_in.order_libraries(&[HIDDEN_LIBRARY_ID, FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]);
    browse.refresh();
    eventually(|| libraries(&browse) == [SHOWS_LIBRARY_ID, HIDDEN_LIBRARY_ID, FILMS_LIBRARY_ID])
        .await;

    // Reset: the server's order and hiding again.
    browse.reset_group(NavGroup::Libraries).unwrap();
    eventually(|| libraries(&browse) == [hidden.as_str(), FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID])
        .await;
    // Another session of the same user sees the same arrangement.
    assert_eq!(
        keys(&layout(&app.browse(&server).unwrap()).unwrap().tabs),
        ["movie", "!series"]
    );
}

#[tokio::test]
async fn homes_rows_follow_the_users_arrangement() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let home = browse.home();
    let films = format!("latest:{FILMS_LIBRARY_ID}");
    let shows = format!("latest:{SHOWS_LIBRARY_ID}");
    eventually(|| {
        let rows = rows(&home);
        rows.contains(&films) && rows.contains(&shows) && !home.state().borrow().loading
    })
    .await;
    let before = rows(&home);
    let at = |rows: &[String], key: &str| rows.iter().position(|row| row == key).unwrap();
    assert!(at(&before, &films) < at(&before, &shows));

    // The editor lists every row, in order.
    let layout: Vec<String> = browse
        .home_layout()
        .into_iter()
        .map(|row| row.key)
        .collect();
    assert_eq!(
        layout,
        [
            "resume",
            "next_up",
            "favourites",
            "recently_added",
            films.as_str(),
            shows.as_str(),
            "seen_recently"
        ]
    );

    // Each Latest row moves on its own: Latest in Shows first.
    browse.move_item(NavGroup::HomeRows, &shows, 0).unwrap();
    eventually(|| rows(&home).first() == Some(&shows)).await;
    assert!(at(&rows(&home), &films) > 0);

    // Hidden: gone from Home, still in the editor.
    let views = stand_in.count("/Items/Latest");
    browse.set_hidden(NavGroup::HomeRows, &films, true).unwrap();
    eventually(|| !rows(&home).contains(&films)).await;
    assert!(
        browse
            .home_layout()
            .iter()
            .any(|row| row.key == films && row.hidden)
    );
    // A reload does not ask for it.
    home.refresh();
    eventually(|| !home.state().borrow().loading).await;
    assert_eq!(
        stand_in.count("/Items/Latest"),
        views + 1,
        "only Latest in Shows"
    );

    // Shown again: it loads, in its place.
    browse
        .set_hidden(NavGroup::HomeRows, &films, false)
        .unwrap();
    eventually(|| rows(&home).contains(&films)).await;
    assert_eq!(rows(&home).first(), Some(&shows));
}
