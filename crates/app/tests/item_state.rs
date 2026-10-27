//! One source of truth for each item's user state on every page (owner, 2026-10-27), through `App` against the Jellyfin stand-in:
//! whichever page read an item last, or whichever change came last, every page shows the same marks. Each test names the desync it
//! covers (D1 to D5 of the research for fix/item-state-single-source).

use std::time::Duration;

use current_app::{AddServerState, SignInState};
use current_app::{App, Browse, DetailsView, HomeFeed, PagedItems, Slot};
use current_model::{ItemId, ItemQuery, ItemRef, ItemSummary, Library, ServerRef, UserState};
use jellyfin_stand_in::library::{
    BOX_SET_ID, BOX_SET_MEMBERS, EPISODE_IDS, FILM_IDS, FILMS_LIBRARY_ID, SERIES_ID,
};
use jellyfin_stand_in::{PASSWORD, StandIn, USER_NAME};

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

/// Lets the answers and the dependent reload (a second after the last change) arrive.
async fn settle() {
    tokio::time::sleep(current_app::DEPENDENT_RELOAD_DELAY + Duration::from_millis(700)).await;
}

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
    let mut capabilities = app.capabilities();
    tokio::time::timeout(
        Duration::from_secs(5),
        capabilities.wait_for(|capabilities| capabilities.servers[0].address.is_some()),
    )
    .await
    .unwrap()
    .unwrap();
    app.capabilities().borrow().servers[0].server.clone()
}

struct Session {
    server: ServerRef,
    browse: Browse,
}

impl Session {
    async fn open(stand_in: &StandIn, app: &App) -> Self {
        let server = signed_in(stand_in, app).await;
        let browse = app.browse(&server).unwrap();
        Self { server, browse }
    }

    fn item(&self, id: &str) -> ItemRef {
        ItemRef::new(self.server.clone(), ItemId::new(id))
    }

    async fn details(&self, id: &str) -> DetailsView {
        let page = self.browse.details(&self.item(id));
        eventually("the page", || loaded(&page)).await;
        page
    }

    async fn home(&self) -> HomeFeed {
        let home = self.browse.home();
        eventually("Home", || {
            !home.state().borrow().loading && latest_films(&home, &self.item(FILM_IDS[0])).is_some()
        })
        .await;
        home
    }

    fn library(&self, id: &str) -> Library {
        Library {
            item: self.item(id),
            name: "Example Library".into(),
            collection: None,
            artwork: Vec::new(),
        }
    }

    /// A grid of the items below `id` (a library, or the collection).
    async fn grid(&self, id: &str, expect: usize) -> PagedItems {
        let grid = self
            .browse
            .open_library(&self.library(id), ItemQuery::default());
        eventually("the grid", || {
            grid.slots()
                .snapshot()
                .iter()
                .filter(|slot| matches!(slot, Slot::Item(_)))
                .count()
                == expect
        })
        .await;
        grid
    }
}

fn loaded(page: &DetailsView) -> bool {
    page.details().borrow().is_some() && !page.state().borrow().loading
}

fn on_page(page: &DetailsView) -> Option<UserState> {
    page.details()
        .borrow()
        .as_ref()
        .map(|details| details.summary.user.clone())
}

fn latest_films(home: &HomeFeed, item: &ItemRef) -> Option<UserState> {
    home.row(&format!("latest:{FILMS_LIBRARY_ID}"))?
        .snapshot()
        .into_iter()
        .find(|summary| &summary.item == item)
        .map(|summary| summary.user)
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

fn in_list(list: &[ItemSummary], item: &ItemRef) -> Option<UserState> {
    list.iter()
        .find(|summary| &summary.item == item)
        .map(|summary| summary.user.clone())
}

fn played(state: Option<UserState>) -> Option<bool> {
    state.map(|state| state.played)
}

/// D1: an episode's page, then its series' page read after the episode was marked on another device. The episode's page shows
/// what the series' page read: before, it kept its own first read (`the_copy_read_last_answers_the_played_button` asserted that).
#[tokio::test]
async fn a_page_read_later_shows_on_a_page_read_before() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let first = session.item(EPISODE_IDS[0]);
    let episode = session.details(EPISODE_IDS[0]).await;
    assert_eq!(played(on_page(&episode)), Some(false));

    stand_in.set_played_on_server(EPISODE_IDS[0], true);
    let series = session.browse.details(&session.item(SERIES_ID));
    eventually("the series' page", || {
        series.episodes().len() == 2 && loaded(&series)
    })
    .await;
    assert_eq!(
        played(in_list(&series.episodes().snapshot(), &first)),
        Some(true)
    );
    eventually("the episode's page follows the series' page", || {
        played(on_page(&episode)) == Some(true)
    })
    .await;
}

/// D1: Home, then a film's page read after the film was marked on another device. Home's tile shows what the page read.
#[tokio::test]
async fn home_shows_what_a_details_page_read_later() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let film = session.item(FILM_IDS[0]);
    let home = session.home().await;
    assert_eq!(played(latest_films(&home, &film)), Some(false));

    stand_in.set_played_on_server(FILM_IDS[0], true);
    let page = session.details(FILM_IDS[0]).await;
    assert_eq!(played(on_page(&page)), Some(true));
    eventually("Home's tile follows the page", || {
        played(latest_films(&home, &film)) == Some(true)
    })
    .await;
}

/// D1: a film's page kept for Back, then Home refreshed after the film was marked on another device. The kept page shows what
/// Home read when the user goes back to it.
#[tokio::test]
async fn a_page_kept_for_back_shows_what_home_read_later() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let film = session.item(FILM_IDS[0]);
    let kept = session.details(FILM_IDS[0]).await;
    let home = session.home().await;

    stand_in.set_played_on_server(FILM_IDS[0], true);
    home.refresh();
    eventually("Home read again", || {
        played(latest_films(&home, &film)) == Some(true)
    })
    .await;
    eventually("the kept page follows Home", || {
        played(on_page(&kept)) == Some(true)
    })
    .await;
}

/// D1: Home, then a library grid opened (its page loaded) after the film was made a favourite on another device. Home's tile
/// shows what the grid read.
#[tokio::test]
async fn home_shows_what_a_grid_page_read_later() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let film = session.item(FILM_IDS[0]);
    let home = session.home().await;

    stand_in.set_played_on_server(FILM_IDS[0], true);
    let grid = session.grid(FILMS_LIBRARY_ID, FILM_IDS.len()).await;
    assert_eq!(played(in_grid(&grid, &film)), Some(true));
    eventually("Home's tile follows the grid", || {
        played(latest_films(&home, &film)) == Some(true)
    })
    .await;
}

/// D2: Home's rows were read before the user made a film a favourite on its page, and answer only after the server confirmed it.
/// The Latest row does not reload on a mark: it must not keep the answer read before the change.
#[tokio::test]
async fn an_answer_read_before_a_confirmed_change_does_not_undo_it() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let film = session.item(FILM_IDS[0]);
    let page = session.details(FILM_IDS[0]).await;

    stand_in.hold_answer("/Items/Latest");
    let home = session.browse.home();
    eventually("Home's Latest requests arrived", || {
        stand_in.count("/Items/Latest") >= 2
    })
    .await;
    session.browse.set_favourite(&film, true);
    settle().await;
    assert_eq!(on_page(&page).map(|state| state.favourite), Some(true));
    stand_in.release("/Items/Latest");
    eventually("Home's Latest row", || latest_films(&home, &film).is_some()).await;
    settle().await;
    assert_eq!(
        latest_films(&home, &film).map(|state| state.favourite),
        Some(true),
        "the row keeps the confirmed change"
    );
    assert_eq!(
        session
            .browse
            .user_state(&film)
            .map(|state| state.favourite),
        Some(true)
    );
}

/// D3: a collection marked played marks its films on the server (`BoxSet.MarkPlayed`); every list showing them follows: the
/// collection's own grid, the films library's grid and Home.
#[tokio::test]
async fn a_collection_marked_played_marks_its_films_everywhere() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let home = session.home().await;
    let films = session.grid(FILMS_LIBRARY_ID, FILM_IDS.len()).await;
    let collection = session.grid(BOX_SET_ID, BOX_SET_MEMBERS.len()).await;
    let page = session.details(BOX_SET_ID).await;
    assert_eq!(played(on_page(&page)), Some(false));

    session.browse.set_played(&session.item(BOX_SET_ID), true);
    settle().await;
    assert_eq!(played(on_page(&page)), Some(true), "the collection's page");
    for id in BOX_SET_MEMBERS {
        let film = session.item(id);
        assert_eq!(
            played(in_grid(&collection, &film)),
            Some(true),
            "the collection's grid"
        );
        assert_eq!(played(in_grid(&films, &film)), Some(true), "the films grid");
        assert_eq!(played(latest_films(&home, &film)), Some(true), "Home");
    }
    // The film outside the collection is untouched.
    assert_eq!(
        played(in_grid(&films, &session.item(FILM_IDS[1]))),
        Some(false)
    );
}

/// D5: a change for an item no live list holds is still sent, whatever the state it asks for.
#[tokio::test]
async fn a_change_for_an_item_no_list_holds_is_sent() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let session = Session::open(&stand_in, &app).await;
    let film = session.item(FILM_IDS[0]);
    let path = format!("/UserFavoriteItems/{}", FILM_IDS[0]);
    {
        let _page = session.details(FILM_IDS[0]).await;
        session.browse.set_favourite(&film, true);
        settle().await;
    }
    assert_eq!(stand_in.count(&path), 1);
    // No list holds the film now: unmarking it must reach the server.
    session.browse.set_favourite(&film, false);
    settle().await;
    assert_eq!(stand_in.count(&path), 2, "the unmark was sent");
    let page = session.details(FILM_IDS[0]).await;
    assert_eq!(on_page(&page).map(|state| state.favourite), Some(false));
}
