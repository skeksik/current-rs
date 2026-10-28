//! Browsing end to end through the bridge (ADR 0018, task 4), against the Jellyfin stand-in on 127.0.0.1 and its fake libraries:
//! a paged grid's slots and visible-range loading, a failed page and retry, image bytes and cancellation, a rolled-back toggle
//! with its notice, Home rows, and no browse without a signed-in server. Calls run as the foreign side makes them: from a plain
//! thread, futures polled with no tokio context.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use jellyfin_stand_in::library::{FILM_IDS, FILMS_LIBRARY_ID, PERSON_ID, SHOWS_LIBRARY_ID};
use jellyfin_stand_in::paging::{BIG_LIBRARY_ID, BIG_LIBRARY_SIZE, big_item_name, page_key};
use jellyfin_stand_in::{IMAGE, PASSWORD, StandIn, USER_NAME};

use super::*;
use crate::tests::{WAIT, block_on, eventually};
use crate::{AddServerState, SignInState, runtime};

/// Records what a listener receives.
struct Recorder<T>(Mutex<Sender<T>>);

fn recorder<T>() -> (Box<Recorder<T>>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(Recorder(Mutex::new(tx))), rx)
}

impl<T> Recorder<T> {
    fn send(&self, value: T) {
        let _ = self.0.lock().unwrap().send(value);
    }
}

impl SlotsListener for Recorder<Vec<SlotDiff>> {
    fn on_diffs(&self, diffs: Vec<SlotDiff>) {
        self.send(diffs);
    }
}

impl ItemsListener for Recorder<Vec<ItemSummaryDiff>> {
    fn on_diffs(&self, diffs: Vec<ItemSummaryDiff>) {
        self.send(diffs);
    }
}

impl HomeRowsListener for Recorder<Vec<HomeRowDiff>> {
    fn on_diffs(&self, diffs: Vec<HomeRowDiff>) {
        self.send(diffs);
    }
}

impl SearchGroupsListener for Recorder<Vec<SearchGroupDiff>> {
    fn on_diffs(&self, diffs: Vec<SearchGroupDiff>) {
        self.send(diffs);
    }
}

impl PagedStateListener for Recorder<PagedState> {
    fn on_value(&self, value: PagedState) {
        self.send(value);
    }
}

impl BrowseNoticeListener for Recorder<Option<BrowseNotice>> {
    fn on_value(&self, value: Option<BrowseNotice>) {
        self.send(value);
    }
}

/// A list as a UI mirrors it, fed by a recorder.
struct Mirror<T> {
    values: Vec<T>,
    resets: usize,
}

impl<T> Mirror<T> {
    fn new() -> Self {
        Self {
            values: Vec::new(),
            resets: 0,
        }
    }
}

macro_rules! apply {
    ($mirror:expr, $diff:expr, $kind:ident) => {
        match $diff {
            $kind::Append { values } => $mirror.values.extend(values),
            $kind::Insert { index, value } => $mirror.values.insert(index as usize, value),
            $kind::Set { index, value } => $mirror.values[index as usize] = value,
            $kind::Remove { index } => {
                $mirror.values.remove(index as usize);
            }
            $kind::Reset { values } => {
                $mirror.values = values;
                $mirror.resets += 1;
            }
        }
    };
}

/// Applies every batch received so far, then waits for more until `done` holds.
fn follow_slots(
    mirror: &mut Mirror<Slot>,
    rx: &Receiver<Vec<SlotDiff>>,
    what: &str,
    done: impl Fn(&[Slot]) -> bool,
) {
    while !done(&mirror.values) {
        let batch = rx
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        for diff in batch {
            apply!(mirror, diff, SlotDiff);
        }
    }
}

fn follow_items(
    mirror: &mut Mirror<ItemSummary>,
    rx: &Receiver<Vec<ItemSummaryDiff>>,
    what: &str,
    done: impl Fn(&[ItemSummary]) -> bool,
) {
    while !done(&mirror.values) {
        let batch = rx
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        for diff in batch {
            apply!(mirror, diff, ItemSummaryDiff);
        }
    }
}

/// Receives values until one satisfies `done`; returns it.
fn latest_notice(
    rx: &Receiver<Option<BrowseNotice>>,
    what: &str,
    done: impl Fn(&Option<BrowseNotice>) -> bool,
) -> Option<BrowseNotice> {
    loop {
        let value = rx
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
        if done(&value) {
            return value;
        }
    }
}

fn stand_in() -> StandIn {
    runtime::handle()
        .unwrap()
        .block_on(StandIn::start())
        .unwrap()
}

/// A core that stores nothing, signed in to the stand-in; returns it and the server.
fn signed_in(stand_in: &StandIn) -> (Arc<Core>, ServerRef) {
    sign_in(Core::new().unwrap(), stand_in)
}

/// `core` signed in to the stand-in; returns it and the server.
fn sign_in(core: Arc<Core>, stand_in: &StandIn) -> (Arc<Core>, ServerRef) {
    let add = core.add_server();
    block_on(add.probe(stand_in.address().to_owned(), CancelToken::new())).unwrap();
    assert!(
        matches!(add.state(), AddServerState::Ready { .. }),
        "{:?}",
        add.state()
    );
    let flow = add.sign_in().unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    let SignInState::SignedIn { server, .. } = flow.state() else {
        panic!("{:?}", flow.state());
    };
    (core, server)
}

fn item(server: &ServerRef, id: &str) -> ItemRef {
    ItemRef {
        server: server.clone(),
        id: id.to_owned(),
    }
}

fn big_library(server: &ServerRef) -> Library {
    Library {
        item: item(server, BIG_LIBRARY_ID),
        name: "Example Big Library".into(),
        collection: Some("movies".into()),
        artwork: Vec::new(),
    }
}

fn name(slot: &Slot) -> Option<&str> {
    match slot {
        Slot::Item { summary } => Some(&summary.name),
        _ => None,
    }
}

#[test]
fn a_grid_is_sized_to_the_total_and_loads_the_visible_range() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    assert_eq!(browse.server(), server);

    let grid = browse.open_library(big_library(&server), ItemQuery::default());
    let (listener, rx) = recorder();
    let _slots = grid.subscribe_slots(listener);
    let mut mirror = Mirror::new();
    follow_slots(&mut mirror, &rx, "the first page", |slots| {
        slots.len() == BIG_LIBRARY_SIZE && name(&slots[0]).is_some()
    });
    assert_eq!(grid.state().total, BIG_LIBRARY_SIZE as u32);
    assert_eq!(name(&mirror.values[0]), Some(big_item_name(0, 0).as_str()));
    assert_eq!(mirror.values[BIG_LIBRARY_SIZE - 1], Slot::Loading);

    // Far down: the last page loads and fills its slots with Set, never a second Reset.
    let resets = mirror.resets;
    grid.set_visible_range(449, 440);
    follow_slots(&mut mirror, &rx, "the last page", |slots| {
        name(&slots[449]).is_some()
    });
    assert_eq!(
        name(&mirror.values[449]),
        Some(big_item_name(449, 0).as_str())
    );
    assert_eq!(mirror.resets, resets, "loading a page sends no Reset");
    assert!(stand_in.pages().iter().any(|page| page.start == 400));
}

#[test]
fn a_failed_page_becomes_failed_and_retry_fills_it() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    stand_in.fail(&page_key(400));

    let grid = browse.open_library(big_library(&server), ItemQuery::default());
    let (listener, rx) = recorder();
    let _slots = grid.subscribe_slots(listener);
    let (states, state_rx) = recorder();
    let _state = grid.subscribe_state(states);
    let mut mirror = Mirror::new();
    follow_slots(&mut mirror, &rx, "the first page", |slots| {
        slots.len() == BIG_LIBRARY_SIZE
    });
    grid.set_visible_range(445, 449);
    follow_slots(&mut mirror, &rx, "the failed page", |slots| {
        slots[449] == Slot::Failed
    });
    let failed = loop {
        let state = state_rx.recv_timeout(WAIT).expect("a failed state");
        if let Some(failed) = state.failed {
            break failed;
        }
    };
    assert!(
        matches!(failed, BrowseProblem::Network { .. }),
        "{failed:?}"
    );

    stand_in.unfail(&page_key(400));
    grid.retry();
    follow_slots(&mut mirror, &rx, "the retried page", |slots| {
        name(&slots[449]).is_some()
    });
    eventually("the state clears", || grid.state().failed.is_none());
}

#[test]
fn images_cross_as_bytes_and_a_cancelled_load_stops_its_fetch() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let poster = |id: &str| ImageRequest {
        source: ImageSource::Server {
            image: ImageRef {
                item: item(&server, id),
                kind: "primary".into(),
                index: 0,
                tag: "57a2d1e0a0000001".into(),
            },
        },
        width: 320,
        height: 480,
    };

    let bytes = block_on(core.load_image(poster(FILM_IDS[0]), CancelToken::new())).unwrap();
    assert_eq!(bytes, IMAGE);

    // Cancelled while the server holds the answer: the call ends Cancelled and the request is given up.
    let path = format!("/Items/{}/Images/Primary", FILM_IDS[1]);
    stand_in.hold(&path);
    let token = CancelToken::new();
    let load = {
        let core = core.clone();
        let token = token.clone();
        let request = poster(FILM_IDS[1]);
        std::thread::spawn(move || block_on(core.load_image(request, token)))
    };
    eventually("the image request arrives", || stand_in.count(&path) == 1);
    token.cancel();
    assert_eq!(load.join().unwrap(), Err(CoreError::Cancelled));
    eventually("the fetch stops", || stand_in.cancelled(&path) == 1);
    stand_in.release(&path);

    // A box without area is refused without a request; so is an invalid placeholder.
    let empty = ImageRequest {
        width: 0,
        ..poster(FILM_IDS[2])
    };
    assert_eq!(
        block_on(core.load_image(empty, CancelToken::new())),
        Err(CoreError::Image {
            problem: ImageProblem::EmptyBox
        })
    );
    let placeholder = ImageRequest {
        source: ImageSource::BlurHash {
            hash: "LEHV6nWB2yk8pyo0adR*.7kCMdnj".into(),
        },
        width: 320,
        height: 480,
    };
    let bmp = block_on(core.load_image(placeholder, CancelToken::new())).unwrap();
    assert_eq!(&bmp[..2], b"BM");
    assert!(
        stand_in
            .images()
            .iter()
            .all(|seen| !seen.target.contains(FILM_IDS[2]))
    );
}

#[test]
fn a_refused_toggle_rolls_back_with_a_notice_that_keeps_the_name_out_of_debug() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    let libraries = block_on(browse.libraries(CancelToken::new())).unwrap();
    let films = libraries
        .into_iter()
        .find(|library| library.item.id == FILMS_LIBRARY_ID)
        .unwrap();
    let film = item(&server, FILM_IDS[0]);

    let grid = browse.open_library(films, ItemQuery::default());
    let (listener, rx) = recorder();
    let _slots = grid.subscribe_slots(listener);
    let mut mirror = Mirror::new();
    let favourite = |slots: &[Slot]| {
        slots.iter().find_map(|slot| match slot {
            Slot::Item { summary } if summary.item == film => Some(summary.user.favourite),
            _ => None,
        })
    };
    follow_slots(&mut mirror, &rx, "the films", |slots| {
        favourite(slots).is_some()
    });
    assert_eq!(favourite(&mirror.values), Some(false));
    let (notices, notice_rx) = recorder();
    let _notices = core.subscribe_browse_notices(notices);
    // The current value first: no notice yet.
    assert_eq!(latest_notice(&notice_rx, "the first value", |_| true), None);

    let path = format!("/UserFavoriteItems/{}", FILM_IDS[0]);
    stand_in.hold(&path);
    browse.set_favourite(film.clone(), true);
    follow_slots(&mut mirror, &rx, "the change shown at once", |slots| {
        favourite(slots) == Some(true)
    });
    assert!(browse.user_state(film.clone()).unwrap().favourite);
    eventually("the request arrives", || stand_in.count(&path) == 1);
    stand_in.fail(&path);
    stand_in.release(&path);
    follow_slots(&mut mirror, &rx, "the rollback", |slots| {
        favourite(slots) == Some(false)
    });

    let notice = latest_notice(&notice_rx, "the notice", Option::is_some).unwrap();
    let NoticeKind::ActionFailed {
        action,
        item_name,
        problem,
    } = &notice.kind;
    assert_eq!(*action, ItemAction::SetFavourite { favourite: true });
    assert_eq!(item_name.as_deref(), Some("Example Film One"));
    assert!(
        matches!(problem, BrowseProblem::Network { .. }),
        "{problem:?}"
    );
    let debug = format!("{notice:?}");
    assert!(!debug.contains("Example Film"), "{debug}");
    assert!(debug.contains("SetFavourite"), "{debug}");
}

#[test]
fn home_rows_cross_with_one_list_each() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server).unwrap();
    let home = browse.home();
    let (listener, rx) = recorder();
    let _rows = home.subscribe_rows(listener);
    let latest = format!("latest:{FILMS_LIBRARY_ID}");
    let mut rows: Mirror<HomeRow> = Mirror::new();
    while !rows.values.iter().any(|row| row.key == latest) {
        for diff in rx.recv_timeout(WAIT).expect("the Latest row") {
            apply!(rows, diff, HomeRowDiff);
        }
    }
    let row = rows.values.iter().find(|row| row.key == latest).unwrap();
    assert_eq!(row.kind, HomeRowKind::Latest);
    assert_eq!(row.title, "Example Films");
    assert!(!format!("{row:?}").contains("Example Films"));
    assert!(
        rows.values
            .iter()
            .all(|row| row.kind != HomeRowKind::Latest || row.library.is_some())
    );

    let (items, items_rx) = recorder();
    let _row = home.subscribe_row(latest, items).unwrap();
    let mut films = Mirror::new();
    follow_items(&mut films, &items_rx, "the row's items", |items| {
        !items.is_empty()
    });
    assert!(films.values.iter().all(|film| film.kind == "movie"));
    let (missing, _) = recorder::<Vec<ItemSummaryDiff>>();
    assert!(home.subscribe_row("no-such-row".into(), missing).is_none());
    eventually("Home loaded", || !home.state().loading);
}

#[test]
fn the_image_cache_s_size_grows_and_clear_empties_it_for_every_server() {
    let temp = tempfile::tempdir().unwrap();
    let path = |name: &str| temp.path().join(name).to_str().unwrap().to_owned();
    let core = block_on(crate::open_core(
        path("data"),
        path("cache"),
        String::new(),
        crate::SecretStore::Memory,
    ))
    .unwrap();
    let stand_in = stand_in();
    let (core, server) = sign_in(core, &stand_in);
    let size = || block_on(core.image_cache_size(CancelToken::new())).unwrap();
    let poster = |id: &str| ImageRequest {
        source: ImageSource::Server {
            image: ImageRef {
                item: item(&server, id),
                kind: "primary".into(),
                index: 0,
                tag: "57a2d1e0a0000001".into(),
            },
        },
        width: 320,
        height: 480,
    };
    assert_eq!(size(), 0);

    block_on(core.load_image(poster(FILM_IDS[0]), CancelToken::new())).unwrap();
    // The disk write is not awaited by the load.
    eventually("the image is on disk", || size() == IMAGE.len() as u64);
    block_on(core.load_image(poster(FILM_IDS[1]), CancelToken::new())).unwrap();
    eventually("both images are on disk", || {
        size() == 2 * IMAGE.len() as u64
    });

    block_on(core.clear_image_cache()).unwrap();
    assert_eq!(size(), 0);
    // Cleared from memory too: the image is fetched from the server again, and stored again.
    let path = format!("/Items/{}/Images/Primary", FILM_IDS[0]);
    let before = stand_in.count(&path);
    let bytes = block_on(core.load_image(poster(FILM_IDS[0]), CancelToken::new())).unwrap();
    assert_eq!(bytes, IMAGE);
    assert_eq!(stand_in.count(&path), before + 1);
    eventually("the image is stored again", || size() == IMAGE.len() as u64);

    // Signing out deletes the server's images.
    block_on(core.sign_out(server.clone())).unwrap();
    assert_eq!(size(), 0);

    // A core that stores nothing has no disk cache.
    assert_eq!(
        block_on(Core::new().unwrap().image_cache_size(CancelToken::new())),
        Ok(0)
    );
}

impl crate::CapabilitiesListener for Recorder<crate::CapabilitiesSnapshot> {
    fn on_value(&self, value: crate::CapabilitiesSnapshot) {
        self.send(value);
    }
}

/// The first server's status now.
fn status(core: &Core) -> crate::ServerStatusSnapshot {
    let (listener, rx) = recorder();
    let _handle = core.subscribe_capabilities(listener);
    rx.recv_timeout(WAIT).unwrap().servers.remove(0)
}

#[test]
fn the_core_picks_tile_artwork_and_lists_a_library_s_choices() {
    let sorts = library_sort_choices();
    assert_eq!(
        sorts[0],
        SortChoice {
            key: "sort_name".into(),
            order: SortOrder::Ascending
        }
    );
    assert_eq!(library_filter_choices()[0], None);
    assert!(library_filter_choices().contains(&Some("unplayed".into())));

    // A poster tile's artwork is the item's primary image, picked in the core.
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    let libraries = block_on(browse.libraries(CancelToken::new())).unwrap();
    let films = libraries
        .into_iter()
        .find(|library| library.item.id == FILMS_LIBRARY_ID)
        .unwrap();
    let grid = browse.open_library(films, ItemQuery::default());
    let (listener, rx) = recorder();
    let _slots = grid.subscribe_slots(listener);
    let mut mirror = Mirror::new();
    follow_slots(&mut mirror, &rx, "the films", |slots| {
        slots.iter().any(|slot| matches!(slot, Slot::Item { .. }))
    });
    let Some(Slot::Item { summary }) = mirror
        .values
        .iter()
        .find(|slot| matches!(slot, Slot::Item { summary } if summary.artwork.iter().any(|art| art.kind == "primary")))
    else {
        panic!("a film with a primary image");
    };
    assert_eq!(
        summary.poster.as_ref(),
        summary.artwork.iter().find(|art| art.kind == "primary")
    );
}

impl PlayTargetListener for Recorder<Option<PlayTarget>> {
    fn on_value(&self, value: Option<PlayTarget>) {
        self.send(value);
    }
}

#[test]
fn play_crosses_for_a_tile_and_a_detail_page() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    let details = browse.details(item(&server, FILM_IDS[0]));
    let (listener, rx) = recorder();
    let _play = details.subscribe_play(listener);
    let target = loop {
        if let Some(target) = rx.recv_timeout(WAIT).unwrap() {
            break target;
        }
    };
    assert_eq!(target.item.item, item(&server, FILM_IDS[0]));
    assert!(!target.resumes && target.start_ms == 0);

    // A tile's item with a resume point resumes; played, it starts again; a series has no tile target.
    let mut summary = target.item.clone();
    summary.user.position_ms = Some(90_000);
    let tile = play_target(summary.clone()).unwrap();
    assert!(tile.resumes && tile.start_ms == 90_000);
    summary.user.played = true;
    assert_eq!(play_target(summary.clone()).unwrap().start_ms, 0);
    summary.kind = "series".into();
    assert!(play_target(summary).is_none());
}

#[test]
fn signing_in_again_as_the_same_user_names_a_new_session_and_ends_the_old_browse() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let first = status(&core);
    assert!(
        first.session.is_some(),
        "a signed-in server names its session"
    );
    let old = core.browse(server.clone()).unwrap();
    assert!(block_on(old.libraries(CancelToken::new())).is_ok());

    let flow = core.sign_in_again(server.clone()).unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    assert!(matches!(flow.state(), SignInState::SignedIn { .. }));
    let second = status(&core);
    assert_eq!(
        second.user.as_ref().map(|user| &user.id),
        first.user.as_ref().map(|user| &user.id),
        "the same user"
    );
    assert!(second.session.is_some());
    assert_ne!(
        second.session, first.session,
        "a new session, so a new Browse"
    );

    // The old Browse belongs to the replaced session; a new one works.
    assert_eq!(
        block_on(old.libraries(CancelToken::new())),
        Err(CoreError::Browse {
            problem: BrowseProblem::NotSignedIn
        })
    );
    let new = core.browse(server.clone()).unwrap();
    assert!(block_on(new.libraries(CancelToken::new())).is_ok());

    block_on(core.sign_out(server)).unwrap();
    assert_eq!(status(&core).session, None, "no session after a sign-out");
}

impl LibrariesListener for Recorder<Option<Vec<Library>>> {
    fn on_value(&self, value: Option<Vec<Library>>) {
        self.send(value);
    }
}

/// Waits for the libraries `rx` hears until their ids are `ids`.
fn libraries_become(rx: &Receiver<Option<Vec<Library>>>, ids: &[&str]) -> Vec<Library> {
    loop {
        let value = rx.recv_timeout(WAIT).expect("the libraries");
        let libraries = value.unwrap_or_default();
        if libraries
            .iter()
            .map(|library| library.item.id.as_str())
            .eq(ids.iter().copied())
        {
            return libraries;
        }
    }
}

#[test]
fn the_libraries_follow_the_server_and_a_gone_one_is_not_available() {
    const ADDED: &str = "57a2d1e0000000000000000000001003";
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    let (listener, rx) = recorder();
    let _libraries = browse.subscribe_libraries(listener);
    let first = libraries_become(&rx, &[FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]);

    stand_in.add_library(ADDED, "examplekind", "Example Added");
    browse.refresh();
    let added = libraries_become(&rx, &[FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID, ADDED]);
    assert_eq!(added[2].collection.as_deref(), Some("examplekind"));

    // The films library removed, its stale entry opened: the session stays, the grid says "not available", and the list drops it.
    stand_in.remove_library(FILMS_LIBRARY_ID);
    let grid = browse.open_library(first[0].clone(), library_query(first[0].clone()));
    eventually("not available", || {
        grid.state().failed == Some(BrowseProblem::NotAvailable)
    });
    libraries_become(&rx, &[SHOWS_LIBRARY_ID, ADDED]);
    assert!(status(&core).signed_in);
}

#[test]
fn no_browse_without_a_signed_in_server() {
    let core = Core::new().unwrap();
    let server = ServerRef {
        service: "jellyfin".into(),
        key: "1".into(),
    };
    assert!(matches!(
        core.browse(server.clone()),
        Err(CoreError::UnknownServer)
    ));

    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    block_on(core.sign_out(server.clone())).unwrap();
    assert!(matches!(
        core.browse(server.clone()),
        Err(CoreError::NotSignedIn)
    ));
    // The old Browse belongs to the ended session.
    assert_eq!(
        block_on(browse.libraries(CancelToken::new())),
        Err(CoreError::Browse {
            problem: BrowseProblem::NotSignedIn
        })
    );
}

#[test]
fn listeners_are_released_with_their_handles() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let before = core.live_listeners();
    let browse = core.browse(server.clone()).unwrap();
    let grid = browse.open_library(big_library(&server), ItemQuery::default());
    let home = browse.home();
    let details = browse.details(item(&server, FILM_IDS[0]));
    let search = browse.search("Example".into(), SearchFilter::default());
    let handles = [
        grid.subscribe_slots(recorder::<Vec<SlotDiff>>().0),
        grid.subscribe_state(recorder::<PagedState>().0),
        home.subscribe_rows(recorder::<Vec<HomeRowDiff>>().0),
        details.subscribe_similar(recorder::<Vec<ItemSummaryDiff>>().0),
        core.subscribe_browse_notices(recorder::<Option<BrowseNotice>>().0),
    ];
    let search_state = {
        struct Ignore;
        impl LoadStateListener for Ignore {
            fn on_value(&self, _: LoadState) {}
        }
        search.subscribe_state(Box::new(Ignore))
    };
    assert_eq!(core.live_listeners(), before + 6);
    for handle in &handles {
        handle.cancel();
    }
    search_state.cancel();
    eventually("every listener is dropped", || {
        core.live_listeners() == before
    });
}

#[test]
fn a_search_narrowed_to_people_and_a_persons_titles_cross() {
    let stand_in = stand_in();
    let (core, server) = signed_in(&stand_in);
    let browse = core.browse(server.clone()).unwrap();
    let search = browse.search(
        "example".into(),
        SearchFilter {
            kind: Some("person".into()),
            source: SearchSource::Library,
        },
    );
    let (listener, rx) = recorder::<Vec<SearchGroupDiff>>();
    let _groups = search.subscribe_groups(listener);
    let mut groups = Mirror::new();
    while groups.values.is_empty() {
        let batch = rx
            .recv_timeout(WAIT)
            .unwrap_or_else(|_| panic!("timed out waiting for the people"));
        for diff in batch {
            apply!(groups, diff, SearchGroupDiff);
        }
    }
    let kinds: Vec<_> = groups.values.iter().map(|g| g.kind.as_str()).collect();
    assert_eq!(kinds, ["person"]);
    let (listener, rx) = recorder::<Vec<ItemSummaryDiff>>();
    let _people = search.subscribe_group("person".into(), listener).unwrap();
    let mut people = Mirror::new();
    follow_items(&mut people, &rx, "the people", |people| people.len() == 2);
    assert_eq!(people.values[0].item, item(&server, PERSON_ID));
    assert_eq!(people.values[0].kind, "person");

    let titles = browse.open_items(person_query(item(&server, PERSON_ID)));
    let (listener, rx) = recorder::<Vec<SlotDiff>>();
    let _slots = titles.subscribe_slots(listener);
    titles.set_visible_range(0, 10);
    let mut slots = Mirror::new();
    follow_slots(&mut slots, &rx, "the person's titles", |slots| {
        slots.len() == 2 && slots.iter().all(|slot| matches!(slot, Slot::Item { .. }))
    });
}

impl PaneListener for Recorder<Option<PaneLayout>> {
    fn on_value(&self, value: Option<PaneLayout>) {
        self.send(value);
    }
}

/// Waits for the layouts `rx` hears until `check` passes.
fn pane_becomes(
    rx: &Receiver<Option<PaneLayout>>,
    check: impl Fn(&PaneLayout) -> bool,
) -> PaneLayout {
    loop {
        if let Some(layout) = rx.recv_timeout(WAIT).expect("the pane")
            && check(&layout)
        {
            return layout;
        }
    }
}

fn library_ids(layout: &PaneLayout) -> Vec<&str> {
    layout
        .libraries
        .iter()
        .map(|entry| entry.library.item.id.as_str())
        .collect()
}

#[test]
fn the_arrangement_crosses_the_bridge_and_is_saved() {
    let temp = tempfile::tempdir().unwrap();
    let path = |name: &str| temp.path().join(name).to_str().unwrap().to_owned();
    let core = block_on(crate::open_core(
        path("data"),
        path("cache"),
        String::new(),
        crate::SecretStore::Memory,
    ))
    .unwrap();
    let stand_in = stand_in();
    let (core, server) = sign_in(core, &stand_in);
    let browse = core.browse(server.clone()).unwrap();
    let (listener, rx) = recorder();
    let _pane = browse.subscribe_pane(listener);
    let layout = pane_becomes(&rx, |layout| layout.libraries.len() == 2);
    assert_eq!(library_ids(&layout), [FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]);
    assert!(
        layout
            .libraries
            .iter()
            .all(|entry| !entry.hidden && !entry.hidden_on_server)
    );

    block_on(browse.move_item(NavGroup::Libraries, SHOWS_LIBRARY_ID.into(), None)).unwrap();
    pane_becomes(&rx, |layout| {
        library_ids(layout) == [SHOWS_LIBRARY_ID, FILMS_LIBRARY_ID]
    });
    block_on(browse.set_hidden(NavGroup::Views, "favourites".into(), true)).unwrap();
    pane_becomes(&rx, |layout| {
        layout
            .views
            .iter()
            .any(|view| view.key == "favourites" && view.hidden)
    });
    let latest = format!("latest:{FILMS_LIBRARY_ID}");
    block_on(browse.set_hidden(NavGroup::HomeRows, latest.clone(), true)).unwrap();
    let rows = browse.home_layout();
    assert!(
        rows.iter()
            .any(|setting| setting.row.key == latest && setting.hidden)
    );
    assert_eq!(rows[0].row.kind, HomeRowKind::ContinueWatching);
    // Saved per server and user in the settings file.
    let file = std::fs::read_to_string(temp.path().join("data").join(current_store::SETTINGS_FILE))
        .unwrap();
    assert!(file.contains("\"navigation\""), "{file}");
    assert!(
        file.contains(&format!("\"jellyfin/{}/", server.key)),
        "{file}"
    );
    assert!(file.contains("\"home_rows\""), "{file}");

    // Reset: the server's order again.
    block_on(browse.reset_group(NavGroup::Libraries)).unwrap();
    pane_becomes(&rx, |layout| {
        library_ids(layout) == [FILMS_LIBRARY_ID, SHOWS_LIBRARY_ID]
    });
}
