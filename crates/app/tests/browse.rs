//! Browsing end to end through `App` (ADR 0018, task 3), against the Jellyfin stand-in on 127.0.0.1 and its fake libraries: a
//! grid loads the pages around the visible range, two at a time, and cancels those that left it; a refresh sends no `Reset` and
//! keeps loaded slots; a failed page becomes `Failed` and retry fills it; a played or favourite change shows at once in every live
//! list and rolls back on failure, with a notice that names the item and a request log that does not; the rows that depend on it
//! reload a second after the last change; rows without items are absent; a `Browse` ends with its session.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use current_app::{
    AddServerState, App, HomeRowKind, ItemAction, ListSubscription, NoticeKind, ObservableList,
    PagedItems, SignInState, Slot, VectorDiff,
};
use current_jellyfin::AuthError;
use current_model::{
    CollectionKind, ItemId, ItemQuery, ItemRef, ItemSummary, Library, MediaKind, ServerRef,
};
use jellyfin_stand_in::library::{
    EPISODE_IDS, FILM_IDS, FILMS_LIBRARY_ID, SERIES_ID, SHOWS_LIBRARY_ID,
};
use jellyfin_stand_in::paging::{BIG_LIBRARY_ID, BIG_LIBRARY_SIZE, big_item_name, page_key};
use jellyfin_stand_in::{PASSWORD, StandIn, USER_NAME};

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

fn item(server: &ServerRef, id: &str) -> ItemRef {
    ItemRef::new(server.clone(), ItemId::new(id))
}

fn big_library(server: &ServerRef) -> Library {
    Library {
        item: item(server, BIG_LIBRARY_ID),
        name: "Example Big Library".into(),
        collection: Some(CollectionKind::MOVIES),
        artwork: Vec::new(),
    }
}

fn name(slot: Option<Slot>) -> Option<String> {
    match slot? {
        Slot::Item(summary) => Some(summary.name),
        _ => None,
    }
}

fn is_item(slot: Option<Slot>) -> bool {
    matches!(slot, Some(Slot::Item(_)))
}

/// Every diff a subscriber received, in order, from the first batch on.
fn record<T: Clone + Send + 'static>(
    list: &Arc<ObservableList<T>>,
) -> Arc<Mutex<Vec<VectorDiff<T>>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let mut subscription: ListSubscription<T> = list.subscribe();
    let sink = seen.clone();
    tokio::spawn(async move {
        while let Some(batch) = subscription.next_batch().await {
            sink.lock().unwrap().extend(batch);
        }
    });
    seen
}

async fn started(grid: &PagedItems, total: u32) {
    let state = grid.state();
    eventually(|| state.borrow().total == total && grid.slots().len() == total as usize).await;
}

#[tokio::test]
async fn the_visible_range_loads_its_pages_two_at_a_time_and_cancels_those_that_left() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    for start in [100, 200, 300, 400] {
        stand_in.hold(&page_key(start));
    }

    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    assert!(is_item(grid.slots().get(99)));
    assert_eq!(grid.slots().get(100), Some(Slot::Loading));
    // Nothing shown yet: page 0 and the page after it.
    eventually(|| stand_in.pages().len() == 2).await;
    let first = stand_in.pages();
    assert_eq!((first[0].start, first[0].with_total), (0, true));
    assert_eq!((first[1].start, first[1].with_total), (100, false));
    assert_eq!(first[1].limit, 100);

    // Pages 2 (visible), 1 and 3 (either side) are wanted; page 1 is in flight, so only page 2 joins it.
    grid.set_visible_range(250, 260);
    eventually(|| stand_in.pages().len() == 3).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let starts: Vec<usize> = stand_in.pages().iter().map(|page| page.start).collect();
    assert_eq!(starts, [0, 100, 200], "at most two requests at a time");
    assert!(grid.state().borrow().loading);

    // The newest range first: page 4 (visible), then page 3; pages 1 and 2 left the window and are cancelled.
    grid.set_visible_range(449, 440);
    eventually(|| {
        stand_in.cancelled(&page_key(100)) == 1 && stand_in.cancelled(&page_key(200)) == 1
    })
    .await;
    eventually(|| stand_in.pages().len() == 5).await;
    let starts: Vec<usize> = stand_in.pages().iter().map(|page| page.start).collect();
    assert_eq!(starts, [0, 100, 200, 400, 300]);

    for start in [100, 200, 300, 400] {
        stand_in.release(&page_key(start));
    }
    eventually(|| is_item(grid.slots().get(449)) && is_item(grid.slots().get(300))).await;
    assert_eq!(name(grid.slots().get(449)), Some(big_item_name(449, 0)));
    // The cancelled pages stay loading until they are visible again.
    assert_eq!(grid.slots().get(150), Some(Slot::Loading));
    assert_eq!(grid.slots().get(250), Some(Slot::Loading));
    eventually(|| !grid.state().borrow().loading).await;
    assert_eq!(stand_in.pages().len(), 5);

    grid.set_visible_range(150, 150);
    eventually(|| is_item(grid.slots().get(150)) && is_item(grid.slots().get(250))).await;
}

#[tokio::test]
async fn a_refresh_sends_no_reset_and_keeps_loaded_slots() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    // Every page loaded: pages 2 to 4 around slot 310, then 0 to 2 around slot 120.
    grid.set_visible_range(310, 320);
    eventually(|| is_item(grid.slots().get(449))).await;
    grid.set_visible_range(120, 130);
    eventually(|| is_item(grid.slots().get(150))).await;
    eventually(|| !grid.state().borrow().loading).await;
    let diffs = record(&grid.slots());
    eventually(|| !diffs.lock().unwrap().is_empty()).await;

    // The library changed: names and a smaller total. While the refresh is held, the loaded slots keep their items.
    stand_in.set_big_library_revision(1);
    stand_in.set_big_library_size(420);
    stand_in.clear_pages();
    stand_in.hold(&page_key(100));
    grid.refresh();
    eventually(|| stand_in.pages().iter().any(|page| page.start == 100)).await;
    let refresh = stand_in.pages()[0];
    assert_eq!(
        (refresh.start, refresh.with_total),
        (100, true),
        "the visible page reads the total"
    );
    assert!((0..450).all(|index| is_item(grid.slots().get(index))));
    stand_in.release(&page_key(100));
    eventually(|| grid.slots().len() == 420).await;
    eventually(|| name(grid.slots().get(250)) == Some(big_item_name(250, 1))).await;
    assert_eq!(name(grid.slots().get(150)), Some(big_item_name(150, 1)));
    // Page 3 was loaded but is not near the visible range: stale, it keeps its items until it is shown again.
    assert_eq!(name(grid.slots().get(350)), Some(big_item_name(350, 0)));
    assert_eq!(grid.state().borrow().total, 420);

    // A larger total appends slots at the end.
    stand_in.set_big_library_size(470);
    grid.refresh();
    eventually(|| grid.slots().len() == 470).await;
    assert_eq!(grid.slots().get(469), Some(Slot::Loading));
    grid.set_visible_range(460, 469);
    eventually(|| name(grid.slots().get(469)) == Some(big_item_name(469, 1))).await;
    eventually(|| name(grid.slots().get(350)) == Some(big_item_name(350, 1))).await;

    let diffs = diffs.lock().unwrap();
    assert!(
        matches!(diffs[0], VectorDiff::Reset { .. }),
        "a subscription starts with its snapshot"
    );
    assert!(
        diffs[1..].iter().all(|diff| matches!(
            diff,
            VectorDiff::Set { .. } | VectorDiff::Append { .. } | VectorDiff::Remove { .. }
        )),
        "a refresh sends only Set, Append and Remove"
    );
    assert!(
        diffs
            .iter()
            .any(|diff| matches!(diff, VectorDiff::Remove { .. }))
    );
    assert!(
        diffs
            .iter()
            .any(|diff| matches!(diff, VectorDiff::Append { .. }))
    );
}

#[tokio::test]
async fn a_new_query_resets_the_list() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    // The page after the first loads too; recording before it lands would put its diffs between the first batch and the resets.
    eventually(|| is_item(grid.slots().get(100))).await;
    let diffs = record(&grid.slots());
    eventually(|| !diffs.lock().unwrap().is_empty()).await;
    stand_in.clear_pages();
    grid.set_query(ItemQuery {
        order: current_model::SortOrder::Descending,
        ..ItemQuery::default()
    });
    eventually(|| diffs.lock().unwrap().len() >= 3).await;
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    let diffs = diffs.lock().unwrap();
    assert!(matches!(&diffs[1], VectorDiff::Reset { values } if values.is_empty()));
    assert!(matches!(&diffs[2], VectorDiff::Reset { values } if values.len() == BIG_LIBRARY_SIZE));
    let first = stand_in.pages()[0];
    assert_eq!(
        (first.start, first.with_total),
        (0, true),
        "the new query starts at the top"
    );
}

#[tokio::test]
async fn a_failed_page_becomes_failed_and_retry_fills_it() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    stand_in.fail(&page_key(100));
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    eventually(|| grid.slots().get(100) == Some(Slot::Failed)).await;
    assert!((100..200).all(|index| grid.slots().get(index) == Some(Slot::Failed)));
    assert!(grid.state().borrow().failed.is_some());
    assert!(is_item(grid.slots().get(0)));

    stand_in.unfail(&page_key(100));
    grid.retry();
    eventually(|| is_item(grid.slots().get(199))).await;
    eventually(|| grid.state().borrow().failed.is_none()).await;

    // A page that fails, then comes into view again, is retried once by itself; not twice.
    stand_in.fail(&page_key(300));
    grid.set_visible_range(300, 310);
    eventually(|| grid.slots().get(300) == Some(Slot::Failed)).await;
    let count = || {
        stand_in
            .pages()
            .iter()
            .filter(|page| page.start == 300)
            .count()
    };
    assert_eq!(count(), 1);
    for round in 0..2 {
        grid.set_visible_range(0, 10);
        grid.set_visible_range(300, 310);
        eventually(|| !grid.state().borrow().loading).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(count(), 2, "round {round}: one retry by itself");
        assert_eq!(grid.slots().get(300), Some(Slot::Failed));
    }
    stand_in.unfail(&page_key(300));
    grid.retry();
    eventually(|| is_item(grid.slots().get(300))).await;
}

fn played(list: &ObservableList<ItemSummary>, item: &ItemRef) -> Option<bool> {
    list.snapshot()
        .into_iter()
        .find(|summary| &summary.item == item)
        .map(|summary| summary.user.played)
}

fn favourite(list: &ObservableList<ItemSummary>, item: &ItemRef) -> Option<bool> {
    list.snapshot()
        .into_iter()
        .find(|summary| &summary.item == item)
        .map(|summary| summary.user.favourite)
}

fn slot_of(grid: &PagedItems, item: &ItemRef) -> Option<ItemSummary> {
    grid.slots()
        .snapshot()
        .into_iter()
        .find_map(|slot| match slot {
            Slot::Item(summary) if &summary.item == item => Some(*summary),
            _ => None,
        })
}

#[tokio::test]
async fn an_optimistic_change_shows_in_every_live_list_and_rolls_back_on_failure() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let film = item(&server, FILM_IDS[0]);
    let libraries = browse.libraries().await.unwrap();
    let films = libraries
        .iter()
        .find(|library| library.item.id.as_str() == FILMS_LIBRARY_ID)
        .unwrap();

    let grid = browse.open_library(films, ItemQuery::default());
    let home = browse.home();
    let details = browse.details(&film);
    let search = browse.search("Example Film");
    let latest = format!("latest:{FILMS_LIBRARY_ID}");
    eventually(|| slot_of(&grid, &film).is_some()).await;
    eventually(|| {
        home.row(&latest)
            .is_some_and(|row| played(&row, &film).is_some())
    })
    .await;
    eventually(|| details.details().borrow().is_some()).await;
    eventually(|| search.group(&MediaKind::MOVIE).is_some()).await;
    let row = home.row(&latest).unwrap();
    let group = search.group(&MediaKind::MOVIE).unwrap();
    let shown_everywhere = |check: &dyn Fn(&ItemSummary) -> bool| {
        slot_of(&grid, &film).is_some_and(|summary| check(&summary))
            && row
                .snapshot()
                .iter()
                .any(|summary| summary.item == film && check(summary))
            && group
                .snapshot()
                .iter()
                .any(|summary| summary.item == film && check(summary))
            && details
                .details()
                .borrow()
                .as_ref()
                .is_some_and(|details| check(&details.summary))
    };

    // Played: shown everywhere while the request is still held, then confirmed.
    let played_path = format!("/UserPlayedItems/{}", FILM_IDS[0]);
    stand_in.hold(&played_path);
    browse.set_played(&film, true);
    assert!(
        shown_everywhere(&|summary| summary.user.played),
        "shown at once"
    );
    eventually(|| stand_in.count(&played_path) == 1).await;
    stand_in.release(&played_path);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(shown_everywhere(&|summary| summary.user.played));
    assert_eq!(
        browse.user_state(&film).map(|state| state.played),
        Some(true)
    );

    // Favourite: the server refuses, so it rolls back everywhere with one notice.
    let favourite_path = format!("/UserFavoriteItems/{}", FILM_IDS[0]);
    let mut notices = app.browse_notices();
    notices.borrow_and_update();
    stand_in.hold(&favourite_path);
    browse.set_favourite(&film, true);
    assert!(shown_everywhere(&|summary| summary.user.favourite));
    eventually(|| stand_in.count(&favourite_path) == 1).await;
    stand_in.fail(&favourite_path);
    stand_in.release(&favourite_path);
    eventually(|| shown_everywhere(&|summary| !summary.user.favourite)).await;
    assert_eq!(favourite(&group, &film), Some(false));
    let notice = notices.borrow_and_update().clone().unwrap();
    match notice.kind {
        NoticeKind::ActionFailed {
            action,
            item_name,
            problem,
        } => {
            assert_eq!(action, ItemAction::SetFavourite(true));
            assert_eq!(item_name.as_deref(), Some("Example Film One"));
            assert!(matches!(problem, AuthError::Network(_)), "{problem:?}");
        }
    }
    // The name goes to the notice only, never to the request log.
    assert!(!app.diagnostics_text().contains("Example Film"));

    // A second identical failure is a new notice.
    browse.set_favourite(&film, true);
    eventually(|| {
        notices
            .borrow()
            .as_ref()
            .is_some_and(|next| next.seq > notice.seq)
    })
    .await;
    assert!(shown_everywhere(&|summary| !summary.user.favourite));
}

#[tokio::test]
async fn play_starts_the_item_or_the_series_next_episode() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();

    // A film plays itself, from the start while it has no resume point.
    let film = browse.details(&item(&server, FILM_IDS[0]));
    let play = film.play();
    eventually(|| play.borrow().is_some()).await;
    let target = play.borrow().clone().unwrap();
    assert_eq!(target.item.item, item(&server, FILM_IDS[0]));
    assert!(!target.resumes());

    // A series plays its next episode (the stand-in's next up: the first unplayed one).
    let series = browse.details(&item(&server, SERIES_ID));
    let play = series.play();
    eventually(|| play.borrow().is_some() && !series.state().borrow().loading).await;
    assert_eq!(
        play.borrow().as_ref().unwrap().item.item,
        item(&server, EPISODE_IDS[0])
    );
    assert_eq!(stand_in.count("/Shows/NextUp"), 1);

    // Played, at once it starts that episode again; a second later the series' reload names the next one.
    browse.set_played(&item(&server, EPISODE_IDS[0]), true);
    eventually(|| {
        play.borrow()
            .as_ref()
            .is_some_and(|target| target.item.user.played)
    })
    .await;
    eventually(|| {
        play.borrow()
            .as_ref()
            .is_some_and(|target| target.item.item == item(&server, EPISODE_IDS[1]))
    })
    .await;

    // Every episode played: no next up, so the series' first episode, from its start.
    browse.set_played(&item(&server, EPISODE_IDS[1]), true);
    let episodes = format!("/Shows/{SERIES_ID}/Episodes");
    let first = stand_in.count(&episodes);
    eventually(|| stand_in.count(&episodes) >= first + 2).await;
    eventually(|| {
        play.borrow().as_ref().is_some_and(|target| {
            target.item.item == item(&server, EPISODE_IDS[0]) && !target.resumes()
        })
    })
    .await;
    // Nothing plays a season.
    let season = browse.details(&item(&server, jellyfin_stand_in::library::SEASON_ID));
    eventually(|| !season.state().borrow().loading && season.details().borrow().is_some()).await;
    assert!(season.play().borrow().is_none());
}

#[tokio::test]
async fn a_series_keeps_its_play_button_when_its_next_episode_cannot_be_asked_for() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let series = browse.details(&item(&server, SERIES_ID));
    let play = series.play();
    eventually(|| play.borrow().is_some() && !series.state().borrow().loading).await;
    let shown = play.borrow().clone();

    // The refresh's next-up request fails: the button stays as it was, and the page, whose details loaded, is not failed.
    stand_in.fail("/Shows/NextUp");
    let asked = stand_in.count("/Shows/NextUp");
    series.refresh();
    eventually(|| stand_in.count("/Shows/NextUp") > asked && !series.state().borrow().loading)
        .await;
    assert_eq!(*play.borrow(), shown);
    assert!(series.state().borrow().failed.is_none());
    assert!(series.details().borrow().is_some());
}

#[tokio::test]
async fn playback_stopping_gives_every_list_the_item_s_new_marks() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    // The half-watched film, in a list that no stop reloads: the similar items of another film.
    let half_watched = item(&server, FILM_IDS[1]);
    let other = browse.details(&item(&server, FILM_IDS[0]));
    eventually(|| !other.state().borrow().loading && !other.similar().is_empty()).await;
    let shown = || {
        other
            .similar()
            .snapshot()
            .into_iter()
            .find(|summary| summary.item == half_watched)
    };
    let before = shown().expect("the half-watched film among the similar items");
    assert!(before.user.position.is_some() && !before.user.played);
    assert!(PlayTargetOf::resumes(&before));

    // Played to the end elsewhere (the server knows, the list does not); then playback of it stops here.
    stand_in.set_played_on_server(FILM_IDS[1], true);
    browse.playback_stopped(&half_watched);
    eventually(|| shown().is_some_and(|summary| summary.user.played)).await;
    let after = shown().unwrap();
    assert_eq!(after.user.position, None);
    assert!(!PlayTargetOf::resumes(&after));
    assert_eq!(
        browse.user_state(&half_watched).map(|state| state.played),
        Some(true)
    );
}

#[tokio::test]
async fn a_late_answer_after_a_stop_does_not_undo_a_newer_mark() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    // The half-watched film, in a list that no stop reloads: the similar items of another film.
    let film = item(&server, FILM_IDS[1]);
    let other = browse.details(&item(&server, FILM_IDS[0]));
    eventually(|| !other.state().borrow().loading && !other.similar().is_empty()).await;
    let played = || {
        other
            .similar()
            .snapshot()
            .into_iter()
            .find(|summary| summary.item == film)
            .is_some_and(|summary| summary.user.played)
    };
    assert!(!played());

    // The stop asks for the film's marks; the answer is slow.
    let details = format!("/Items/{}", FILM_IDS[1]);
    let asked = stand_in.count(&details);
    stand_in.hold(&details);
    browse.playback_stopped(&film);
    eventually(|| stand_in.count(&details) > asked).await;
    // Meanwhile the user marks it played, and the server confirms that.
    let marked = format!("/UserPlayedItems/{}", FILM_IDS[1]);
    browse.set_played(&film, true);
    eventually(|| stand_in.count(&marked) == 1 && played()).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    // The answer, as the server held it before the mark, arrives last.
    stand_in.set_played_on_server(FILM_IDS[1], false);
    stand_in.release(&details);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(played());
}

/// What Play on a tile of the summary does, by the core's rule.
struct PlayTargetOf;

impl PlayTargetOf {
    fn resumes(summary: &ItemSummary) -> bool {
        current_model::PlayTarget::for_item(summary).is_some_and(|target| target.resumes())
    }
}

#[tokio::test]
async fn dependent_rows_reload_a_second_after_the_last_change() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let home = browse.home();
    let series = browse.details(&item(&server, SERIES_ID));
    eventually(|| home.row("next_up").is_some() && !home.state().borrow().loading).await;
    eventually(|| series.episodes().len() == 2 && !series.state().borrow().loading).await;
    let resume = stand_in.count("/UserItems/Resume");
    let next_up = stand_in.count("/Shows/NextUp");
    let seasons_path = format!("/Shows/{SERIES_ID}/Seasons");
    let seasons = stand_in.count(&seasons_path);

    // Both episodes of the season, one after the other.
    for id in EPISODE_IDS {
        browse.set_played(&item(&server, id), true);
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    assert!(
        series
            .episodes()
            .snapshot()
            .iter()
            .all(|episode| episode.user.played)
    );
    // Not yet: the reload waits a second after the last change. Then Home's row and the series page (its next episode for Play) each
    // ask once.
    assert_eq!(stand_in.count("/Shows/NextUp"), next_up);
    eventually(|| stand_in.count("/Shows/NextUp") == next_up + 2).await;
    eventually(|| stand_in.count(&seasons_path) == seasons + 1).await;
    assert_eq!(stand_in.count("/UserItems/Resume"), resume + 1);
    // Once: the two changes made one reload.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stand_in.count("/Shows/NextUp"), next_up + 2);
    // Every episode is played: "next up" has no items, so its row is gone.
    eventually(|| home.row("next_up").is_none()).await;
    assert!(
        home.rows()
            .snapshot()
            .iter()
            .all(|row| row.kind != HomeRowKind::NextUp)
    );

    // Playback stopping (ADR 0019's event) reloads the same rows.
    browse.playback_stopped(&item(&server, EPISODE_IDS[0]));
    eventually(|| stand_in.count("/Shows/NextUp") >= next_up + 3).await;

    // Coming back to the foreground refreshes every live list.
    let libraries = stand_in.count("/UserViews");
    app.returned_to_foreground();
    eventually(|| stand_in.count("/UserViews") == libraries + 1).await;
}

#[tokio::test]
async fn rows_without_items_are_absent_and_a_failed_row_offers_retry() {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_latest_excludes(&[SHOWS_LIBRARY_ID]);
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    stand_in.fail("/Shows/NextUp");
    let home = browse.home();
    eventually(|| home.state().borrow().failed.is_some()).await;
    eventually(|| !home.state().borrow().loading).await;
    let keys: Vec<String> = home
        .rows()
        .snapshot()
        .into_iter()
        .map(|row| row.key)
        .collect();
    assert_eq!(
        keys,
        ["resume".to_owned(), format!("latest:{FILMS_LIBRARY_ID}")]
    );
    let latest = &home.rows().snapshot()[1];
    assert_eq!(latest.kind, HomeRowKind::Latest);
    assert_eq!(latest.title, "Example Films");
    assert_eq!(
        stand_in.count("/Items/Latest"),
        1,
        "the excluded library is not asked"
    );

    // Retry: "next up" answers and takes its place, between "continue watching" and "latest".
    stand_in.unfail("/Shows/NextUp");
    home.retry();
    eventually(|| home.rows().len() == 3).await;
    eventually(|| home.state().borrow().failed.is_none()).await;
    let kinds: Vec<HomeRowKind> = home
        .rows()
        .snapshot()
        .into_iter()
        .map(|row| row.kind)
        .collect();
    assert_eq!(
        kinds,
        [
            HomeRowKind::ContinueWatching,
            HomeRowKind::NextUp,
            HomeRowKind::Latest
        ]
    );
    assert_eq!(home.row("next_up").unwrap().len(), 1);
    assert!(!home.state().borrow().offline);
}

#[tokio::test]
async fn a_browse_ends_with_its_session() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    assert!(browse.libraries().await.is_ok());

    app.sign_out(&server).await.unwrap();
    assert!(
        app.browse(&server).is_none(),
        "browse needs a signed-in server"
    );
    let requests = stand_in.paths().len();
    assert_eq!(browse.libraries().await, Err(AuthError::NotSignedIn));
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    eventually(|| grid.state().borrow().failed == Some(AuthError::NotSignedIn)).await;
    browse.set_played(&item(&server, FILM_IDS[0]), true);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        stand_in.paths().len(),
        requests,
        "nothing reaches the server"
    );

    // A new sign-in is a new session: the old `Browse` stays ended, a new one works.
    let flow = app.sign_in_again(&server).unwrap().unwrap();
    flow.with_password(USER_NAME, PASSWORD).await;
    assert!(matches!(
        *flow.state().borrow(),
        SignInState::SignedIn { .. }
    ));
    assert_eq!(browse.libraries().await, Err(AuthError::NotSignedIn));
    let again = app.browse(&server).unwrap();
    assert_eq!(again.libraries().await.unwrap().len(), 2);
}

#[tokio::test]
async fn no_browse_without_a_signed_in_jellyfin_server() {
    let app = App::new();
    let server = ServerRef::new(
        current_model::ServiceType::JELLYFIN,
        current_model::ServerKey::new("1"),
    );
    assert!(app.browse(&server).is_none());
}

#[tokio::test]
async fn a_failed_refresh_sends_one_request_and_waits_for_retry() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    eventually(|| is_item(grid.slots().get(199)) && !grid.state().borrow().loading).await;

    stand_in.set_big_library_revision(1);
    stand_in.fail(&page_key(0));
    stand_in.fail(&page_key(100));
    stand_in.clear_pages();
    grid.refresh();
    eventually(|| grid.state().borrow().failed.is_some() && !grid.state().borrow().loading).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let starts: Vec<usize> = stand_in.pages().iter().map(|page| page.start).collect();
    assert_eq!(
        starts,
        [0, 100],
        "each failed page was asked once: {starts:?}"
    );
    // The loaded slots keep their items.
    assert_eq!(name(grid.slots().get(0)), Some(big_item_name(0, 0)));

    stand_in.unfail(&page_key(0));
    stand_in.unfail(&page_key(100));
    grid.retry();
    eventually(|| name(grid.slots().get(150)) == Some(big_item_name(150, 1))).await;
    assert_eq!(name(grid.slots().get(0)), Some(big_item_name(0, 1)));
    eventually(|| grid.state().borrow().failed.is_none()).await;
}

#[tokio::test]
async fn a_short_page_reads_the_total_again() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, BIG_LIBRARY_SIZE as u32).await;
    // The library shrank on the server; the list does not know until a page answers short.
    stand_in.set_big_library_size(420);
    grid.set_visible_range(440, 449);
    eventually(|| grid.slots().len() == 420).await;
    eventually(|| (300..420).all(|index| is_item(grid.slots().get(index)))).await;
    assert_eq!(grid.state().borrow().total, 420);
}

#[tokio::test]
async fn a_large_shrink_sends_no_reset() {
    let stand_in = StandIn::start().await.unwrap();
    stand_in.set_big_library_size(2_000);
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    let grid = browse.open_library(&big_library(&server), ItemQuery::default());
    started(&grid, 2_000).await;
    let diffs = record(&grid.slots());
    eventually(|| !diffs.lock().unwrap().is_empty()).await;
    stand_in.set_big_library_size(100);
    grid.refresh();
    eventually(|| grid.slots().len() == 100).await;
    let diffs = diffs.lock().unwrap();
    assert!(
        diffs[1..]
            .iter()
            .all(|diff| !matches!(diff, VectorDiff::Reset { .. })),
        "1,900 removals in one change are not a reset"
    );
}

#[tokio::test]
async fn a_dependent_reload_keeps_a_full_loads_failure() {
    let stand_in = StandIn::start().await.unwrap();
    let app = App::new();
    let server = signed_in(&stand_in, &app).await;
    let browse = app.browse(&server).unwrap();
    stand_in.fail("/Items/Latest");
    let home = browse.home();
    eventually(|| home.state().borrow().failed.is_some() && !home.state().borrow().loading).await;
    let next_up = stand_in.count("/Shows/NextUp");
    browse.set_played(&item(&server, EPISODE_IDS[0]), true);
    eventually(|| stand_in.count("/Shows/NextUp") == next_up + 1).await;
    eventually(|| !home.state().borrow().loading).await;
    assert!(
        home.state().borrow().failed.is_some(),
        "the Latest rows are still missing: Retry stays"
    );
}
