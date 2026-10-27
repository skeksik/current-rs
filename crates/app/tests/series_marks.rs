//! Played marks between a series and its episodes (ADR 0018, section 5, the owner's run of 2026-10-26), through `App` against the
//! Jellyfin stand-in, which marks and derives them as Jellyfin 12.2 does: marking a series marks every episode below it, and a
//! series' played state, unplayed count and played percentage come from its episodes. Each test names the case it covers.

use std::time::Duration;

use current_app::{
    AddServerState, App, Browse, DetailsView, HomeFeed, PagedItems, SignInState, Slot,
};
use current_model::{ItemId, ItemRef, ItemSummary, ServerRef};
use jellyfin_stand_in::library::{EPISODE_IDS, SEASON_ID, SERIES_ID, SHOWS_LIBRARY_ID};
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

/// Every list the owner looked at: the shows library's grid and Home (the series' poster), the series' page (its season and
/// episodes) and the first episode's page.
struct Lists {
    server: ServerRef,
    browse: Browse,
    grid: PagedItems,
    home: HomeFeed,
    series: DetailsView,
    episode: DetailsView,
}

impl Lists {
    async fn open(stand_in: &StandIn, app: &App) -> Self {
        let server = signed_in(stand_in, app).await;
        let browse = app.browse(&server).unwrap();
        let libraries = browse.libraries().await.unwrap();
        let shows = libraries
            .iter()
            .find(|library| library.item.id.as_str() == SHOWS_LIBRARY_ID)
            .unwrap();
        let grid = browse.open_library(shows, current_app::library_query(shows));
        let home = browse.home();
        let item = |id: &str| ItemRef::new(server.clone(), ItemId::new(id));
        let series = browse.details(&item(SERIES_ID));
        let episode = browse.details(&item(EPISODE_IDS[0]));
        let lists = Self {
            server,
            browse,
            grid,
            home,
            series,
            episode,
        };
        eventually("every list loaded", || lists.loaded()).await;
        lists
    }

    fn loaded(&self) -> bool {
        self.grid_series().is_some()
            && self.latest_series().is_some()
            && !self.home.state().borrow().loading
            && self.series.episodes().len() == 2
            && self.series.seasons().len() == 1
            && !self.series.state().borrow().loading
            && self.episode.details().borrow().is_some()
            && !self.episode.state().borrow().loading
    }

    fn item(&self, id: &str) -> ItemRef {
        ItemRef::new(self.server.clone(), ItemId::new(id))
    }

    fn grid_series(&self) -> Option<ItemSummary> {
        let series = self.item(SERIES_ID);
        self.grid
            .slots()
            .snapshot()
            .into_iter()
            .find_map(|slot| match slot {
                Slot::Item(summary) if summary.item == series => Some(*summary),
                _ => None,
            })
    }

    fn latest_series(&self) -> Option<ItemSummary> {
        let series = self.item(SERIES_ID);
        self.home
            .row(&format!("latest:{SHOWS_LIBRARY_ID}"))?
            .snapshot()
            .into_iter()
            .find(|summary| summary.item == series)
    }

    /// The series as its page, its grid tile and its Home poster show it.
    fn series_everywhere(&self) -> Vec<ItemSummary> {
        let page = self
            .series
            .details()
            .borrow()
            .as_ref()
            .map(|details| details.summary.clone());
        [page, self.grid_series(), self.latest_series()]
            .into_iter()
            .map(|summary| summary.expect("the series in every list"))
            .collect()
    }

    fn series_played(&self, played: bool) -> bool {
        self.series_everywhere()
            .iter()
            .all(|summary| summary.user.played == played)
    }

    /// The posters' unplayed count and progress: the grid's and Home's.
    fn posters(&self, unplayed: u32, progress: Option<f32>) -> bool {
        [self.grid_series(), self.latest_series()]
            .into_iter()
            .flatten()
            .all(|summary| {
                summary.user.unplayed_count == Some(unplayed) && summary.user.progress == progress
            })
    }

    /// Every episode as the series' page lists it, and the first as its own page shows it.
    fn episodes_played(&self, played: bool) -> bool {
        let page = self
            .episode
            .details()
            .borrow()
            .as_ref()
            .is_some_and(|details| details.summary.user.played == played);
        page && self
            .series
            .episodes()
            .snapshot()
            .iter()
            .all(|episode| episode.user.played == played)
    }

    fn season_played(&self, played: bool) -> bool {
        self.series
            .seasons()
            .snapshot()
            .iter()
            .all(|season| season.user.played == played)
    }

    /// What the UI's Played button does: flips the mark every list shows.
    fn toggle(&self, id: &str) {
        let item = self.item(id);
        let state = self.browse.user_state(&item).expect("a loaded item");
        self.browse.set_played(&item, !state.played);
    }
}

/// Lets the answers and the dependent reload (a second after the last change) arrive.
async fn settle() {
    tokio::time::sleep(current_app::DEPENDENT_RELOAD_DELAY + Duration::from_millis(700)).await;
}

/// Cases 1 and 5: marking a series played marks its episodes and season at once, the series keeps its mark after the answer and the
/// reloads, and its posters lose their unplayed count and progress.
#[tokio::test]
async fn a_series_marked_played_keeps_its_mark_and_marks_its_episodes() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    assert!(lists.series_played(false) && lists.posters(2, Some(0.0)));

    let path = format!("/UserPlayedItems/{SERIES_ID}");
    stand_in.hold(&path);
    lists.toggle(SERIES_ID);
    assert!(lists.series_played(true), "the series at once");
    assert!(lists.episodes_played(true), "its episodes at once");
    assert!(lists.season_played(true), "its season at once");
    // An episode page opened while the request is held shows the mark too.
    let second = lists.browse.details(&lists.item(EPISODE_IDS[1]));
    eventually("the second episode's page", || {
        second
            .details()
            .borrow()
            .as_ref()
            .is_some_and(|details| details.summary.user.played)
    })
    .await;

    eventually("the request", || stand_in.count(&path) == 1).await;
    stand_in.release(&path);
    settle().await;
    assert!(
        lists.series_played(true),
        "case 1: the series keeps its mark"
    );
    assert!(lists.episodes_played(true) && lists.season_played(true));
    assert!(
        lists.posters(0, Some(1.0)),
        "case 5: {:?}",
        lists.series_everywhere()
    );
    // One request: the server marked the episodes itself.
    assert_eq!(stand_in.count(&path), 1);
    for episode in EPISODE_IDS {
        assert_eq!(stand_in.count(&format!("/UserPlayedItems/{episode}")), 0);
    }
}

/// Case 2, and 5 the other way: unmarking a series unmarks its episodes at once, and its posters show the server's unplayed count.
#[tokio::test]
async fn a_series_marked_unplayed_unmarks_its_episodes() {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_played_on_server(SERIES_ID, true);
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    assert!(lists.series_played(true) && lists.episodes_played(true));

    lists.toggle(SERIES_ID);
    assert!(lists.series_played(false), "the series at once");
    assert!(lists.episodes_played(false), "case 2: its episodes at once");
    assert!(lists.season_played(false));
    settle().await;
    assert!(lists.series_played(false) && lists.episodes_played(false));
    assert!(
        lists.posters(2, Some(0.0)),
        "case 5: {:?}",
        lists.series_everywhere()
    );
    let seasons = lists.series.seasons().snapshot();
    assert_eq!(
        seasons[0].user.unplayed_count,
        Some(2),
        "the season's count"
    );
}

/// Cases 3 and 5: marking every episode played marks the series at once, in every list, and the posters follow each episode.
#[tokio::test]
async fn every_episode_played_marks_the_series() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;

    lists.toggle(EPISODE_IDS[0]);
    assert!(lists.series_played(false));
    // One of two: half played, by the server's count.
    eventually("one unplayed, half played", || lists.posters(1, Some(0.5))).await;
    lists.toggle(EPISODE_IDS[1]);
    assert!(lists.series_played(true), "case 3: the series at once");
    assert!(lists.season_played(true));
    settle().await;
    assert!(lists.series_played(true), "case 3: after the answers");
    assert!(
        lists.posters(0, Some(1.0)),
        "case 5: {:?}",
        lists.series_everywhere()
    );
}

/// Case 4: a series marked through its episodes is unmarked with one click, and so are its episodes.
#[tokio::test]
async fn a_series_marked_through_its_episodes_unmarks_with_one_click() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    lists.toggle(EPISODE_IDS[0]);
    lists.toggle(EPISODE_IDS[1]);
    settle().await;
    assert!(lists.series_played(true));

    lists.toggle(SERIES_ID);
    assert!(lists.series_played(false), "case 4: one click");
    assert!(lists.episodes_played(false));
    settle().await;
    assert!(lists.series_played(false) && lists.episodes_played(false));
    let unplayed = format!("/UserPlayedItems/{SERIES_ID}");
    assert_eq!(stand_in.count(&unplayed), 1, "one request for the series");
}

/// Case 6: after unmarking a series, an episode's own page shows it unplayed, at once and after the reloads.
#[tokio::test]
async fn an_episode_page_follows_its_series() {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_played_on_server(SERIES_ID, true);
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    let page_played = || {
        lists
            .episode
            .details()
            .borrow()
            .as_ref()
            .map(|details| details.summary.user.played)
    };
    assert_eq!(page_played(), Some(true));

    lists.toggle(SERIES_ID);
    assert_eq!(page_played(), Some(false), "case 6: at once");
    settle().await;
    assert_eq!(page_played(), Some(false), "case 6: after the reloads");
}

/// A series' mark the server refuses rolls back on its episodes and season too.
#[tokio::test]
async fn a_refused_series_mark_rolls_back_its_episodes() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    let path = format!("/UserPlayedItems/{SERIES_ID}");
    stand_in.fail(&path);
    lists.toggle(SERIES_ID);
    assert!(lists.episodes_played(true) && lists.season_played(true));
    eventually("rolled back", || {
        lists.series_played(false) && lists.episodes_played(false) && lists.season_played(false)
    })
    .await;
    settle().await;
    assert!(lists.series_played(false) && lists.episodes_played(false));
    assert!(lists.posters(2, Some(0.0)));
}

/// An episode's own newer mark wins over its series' older one while both are pending.
#[tokio::test]
async fn an_episode_s_newer_mark_wins_over_its_series() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    let path = format!("/UserPlayedItems/{SERIES_ID}");
    stand_in.hold(&path);
    lists.toggle(SERIES_ID);
    eventually("the request", || stand_in.count(&path) == 1).await;
    // Unmarked after the series was marked: it stays unmarked.
    lists.toggle(EPISODE_IDS[0]);
    let first = lists.item(EPISODE_IDS[0]);
    assert_eq!(
        lists.browse.user_state(&first).map(|state| state.played),
        Some(false)
    );
    stand_in.release(&path);
    settle().await;
    assert_eq!(
        lists.browse.user_state(&first).map(|state| state.played),
        Some(false)
    );
    let second = lists.item(EPISODE_IDS[1]);
    assert_eq!(
        lists.browse.user_state(&second).map(|state| state.played),
        Some(true)
    );
    assert!(lists.series_played(false), "one episode left unplayed");
    assert!(lists.posters(1, Some(0.5)));
}

/// Marking both episodes within the delay reads the series' and season's marks back once each, not once per episode.
#[tokio::test]
async fn read_backs_are_coalesced() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    lists.toggle(EPISODE_IDS[0]);
    lists.toggle(EPISODE_IDS[1]);
    settle().await;
    for id in [SERIES_ID, SEASON_ID] {
        assert_eq!(
            stand_in.count(&format!("/UserItems/{id}/UserData")),
            1,
            "{id}"
        );
    }
}

/// The estimate shown at once is replaced by the server's count when they differ: here the second episode was marked on another
/// device, so the series is played as soon as the first one is.
#[tokio::test]
async fn the_server_s_count_replaces_the_estimate() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    stand_in.set_played_on_server(EPISODE_IDS[1], true);
    lists.toggle(EPISODE_IDS[0]);
    assert!(
        lists.posters(1, Some(0.0)) && lists.series_played(false),
        "the estimate"
    );
    eventually("the server's count", || {
        lists.posters(0, Some(1.0)) && lists.series_played(true)
    })
    .await;
}

/// A read-back sent before a newer played change is dropped (its count predates the change); the series is read again after it.
#[tokio::test]
async fn a_read_back_older_than_a_newer_mark_is_dropped() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    let read_back = format!("/UserItems/{SERIES_ID}/UserData");
    stand_in.hold(&read_back);
    lists.toggle(EPISODE_IDS[0]);
    eventually("the read-back", || stand_in.count(&read_back) == 1).await;
    // The second episode is marked while the read-back waits; its own request waits too, so the server still counts one unplayed.
    let second = format!("/UserPlayedItems/{}", EPISODE_IDS[1]);
    stand_in.hold(&second);
    lists.toggle(EPISODE_IDS[1]);
    assert!(lists.series_played(true), "the estimate: nothing unplayed");
    stand_in.release(&read_back);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        lists.series_played(true),
        "the older answer is dropped: {:?}",
        lists
            .series_everywhere()
            .iter()
            .map(|s| (s.user.played, s.user.unplayed_count))
            .collect::<Vec<_>>()
    );
    assert!(lists.posters(0, None));
    stand_in.release(&second);
    eventually("read again", || stand_in.count(&read_back) >= 2).await;
    settle().await;
    assert!(lists.series_played(true) && lists.posters(0, Some(1.0)));
}

/// Case 1 on its own: once the server answered a series' mark and the lists reloaded, the series still shows it everywhere.
#[tokio::test]
async fn a_series_mark_stays_after_the_answer_and_the_reloads() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    lists.toggle(SERIES_ID);
    let path = format!("/UserPlayedItems/{SERIES_ID}");
    eventually("the answer", || stand_in.count(&path) == 1).await;
    settle().await;
    assert!(lists.series_played(true), "{:?}", lists.series_everywhere());
    assert!(
        lists
            .browse
            .user_state(&lists.item(SERIES_ID))
            .is_some_and(|state| state.played),
        "what the Played button reads"
    );
}

/// Episodes loaded while a series' mark waits for the server take it, and a refusal rolls them back too.
#[tokio::test]
async fn episodes_loaded_while_a_series_mark_waits_roll_back_with_it() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let libraries = browse.libraries().await.unwrap();
    let shows = libraries
        .iter()
        .find(|library| library.item.id.as_str() == SHOWS_LIBRARY_ID)
        .unwrap();
    let grid = browse.open_library(shows, current_app::library_query(shows));
    let series = ItemRef::new(server.clone(), ItemId::new(SERIES_ID));
    eventually("the grid", || browse.user_state(&series).is_some()).await;

    let path = format!("/UserPlayedItems/{SERIES_ID}");
    stand_in.hold(&path);
    browse.set_played(&series, true);
    eventually("the request", || stand_in.count(&path) == 1).await;
    // Only now is the series' page opened: its episodes load with the pending mark.
    let page = browse.details(&series);
    let episodes_played = |played: bool| {
        let episodes = page.episodes().snapshot();
        episodes.len() == 2 && episodes.iter().all(|episode| episode.user.played == played)
    };
    eventually("the page's episodes, marked", || episodes_played(true)).await;
    stand_in.fail(&path);
    stand_in.release(&path);
    eventually("rolled back", || episodes_played(false)).await;
    settle().await;
    assert!(episodes_played(false));
    assert!(
        browse
            .user_state(&series)
            .is_some_and(|state| !state.played)
    );
    drop(grid);
}

/// A season read back while its series' own mark waits for the server is dropped: the season keeps showing the series' mark.
#[tokio::test]
async fn a_season_read_back_older_than_its_series_mark_is_dropped() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    let season_read_back = format!("/UserItems/{SEASON_ID}/UserData");
    stand_in.hold(&season_read_back);
    lists.toggle(EPISODE_IDS[0]);
    eventually("the season's read-back", || {
        stand_in.count(&season_read_back) == 1
    })
    .await;
    // The series is marked played while the read-back waits, and its own request waits too.
    let series_mark = format!("/UserPlayedItems/{SERIES_ID}");
    stand_in.hold(&series_mark);
    lists.toggle(SERIES_ID);
    assert!(lists.season_played(true));
    stand_in.release(&season_read_back);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        lists.season_played(true),
        "the older answer (one unplayed) is dropped"
    );
    stand_in.release(&series_mark);
    settle().await;
    assert!(lists.season_played(true) && lists.series_played(true));
}

/// A read-back that fails keeps the estimate and tells the user nothing.
#[tokio::test]
async fn a_failed_read_back_keeps_the_estimate_without_a_notice() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let lists = Lists::open(&stand_in, &app).await;
    let mut notices = app.browse_notices();
    notices.borrow_and_update();
    let read_back = format!("/UserItems/{SERIES_ID}/UserData");
    stand_in.fail(&read_back);
    lists.toggle(EPISODE_IDS[0]);
    eventually("the read-back", || stand_in.count(&read_back) == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // Home's poster, which no reload reads again, keeps the estimate.
    let poster = lists.latest_series().unwrap();
    assert_eq!(poster.user.unplayed_count, Some(1), "the estimate");
    assert!(!poster.user.played);
    settle().await;
    assert_eq!(lists.latest_series().unwrap().user.unplayed_count, Some(1));
    assert!(!notices.has_changed().unwrap(), "no notice");
}
