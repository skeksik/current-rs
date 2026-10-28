//! A small fake library for the browse endpoints (ADR 0018), in Jellyfin 12.2's shapes: PascalCase names, nulls left out, ids as 32
//! hex digits. Two libraries: films and shows, with one series of one season. Libraries can be added, renamed, reordered and
//! removed while it runs ([`crate::Controls::add_library`] and the others). Every name is an example.
//!
//! Played marks follow Jellyfin 12.2 (ca0f16eb, read 2026-10-27): marking a series or season marks every episode below it and not the
//! folder itself (`Folder.MarkPlayed` and `MarkUnplayed`: "Sweep through recursively and update status" over `Recursive = true,
//! IsFolder = false`); a folder's played state is derived from its episodes (`Folder.FillUserDataDtoValues`: `UnplayedItemCount =
//! total - played`, `PlayedPercentage = played / total * 100`, `Played = playedCount >= totalCount`); Next up lists a series only once
//! one of its episodes was played (`NextUpService.GetNextUpSeriesKeys`: `LastPlayedDate != null && LastPlayedDate >= dateCutoff`);
//! and a stop report near the end marks the item played (`UserDataManager.UpdatePlayState`: "mark as completed close to the end").
//! A collection is marked through its films (`BoxSet.MarkPlayed`: "`foreach (var item in GetLinkedChildren(user)) item.MarkPlayed(...)`",
//! v12.3 at 06de749, read 2026-10-27), each taken from `LibraryManager.GetItemById` (`BaseItem.GetLinkedChild`), so their cached
//! instances are saved too; its played state and counts come from them (`ItemCountService.GetPlayedAndTotalCountFromLinkedChildren`).
//!
//! Models an upstream Jellyfin bug, which `JellyfinClient::details` works around (`docs/notes/jellyfin-browse.md`, "Upstream bugs and
//! our workarounds"): a single-item read of an episode or film answers from the server's cached instance of it, which a folder's sweep
//! does not update (Jellyfin v12.3, tag 06de749, read 2026-10-27; v12.2 at ca0f16eb has the same lines): `GET Items/{id}` and `GET UserItems/{id}/UserData` take the item from
//! `LibraryManager.GetItemById` ("`if (_cache.TryGet(id, out var item))`", an LRU of `BaseItem`s) and read its user data from that
//! instance (`UserDataManager.GetUserData`: "`ResolveUserDataRow(item, item.UserData?.Where(...))`"); `SaveUserData` reloads only the
//! instance it was given ("`// rehydrate the cached userdata`"), and `Folder.MarkUnplayed` saves the instances `GetItemList` loads. A
//! list (`GET Items`, `Shows/{id}/Episodes`) reads every item's user data fresh, through `GetUserDataBatch`, which reads the per-user
//! cache every save updates. The stand-in caches an instance on its first single-item read or a mark or stop on its own id; Jellyfin
//! also caches items as a scan creates them (`RegisterItem` from `CreateItems`), and evicts some: not modelled. The model can go with
//! the workaround, once a single-item read serves fresh user data after a folder's sweep: re-check on each new server baseline.

use std::collections::{HashMap, HashSet};

/// The films library.
pub const FILMS_LIBRARY_ID: &str = "57a2d1e0000000000000000000001001";
/// The shows library.
pub const SHOWS_LIBRARY_ID: &str = "57a2d1e0000000000000000000001002";
/// The films, in name order.
pub const FILM_IDS: [&str; 3] = [
    "57a2d1e0000000000000000000002001",
    "57a2d1e0000000000000000000002002",
    "57a2d1e0000000000000000000002003",
];
pub const SERIES_ID: &str = "57a2d1e0000000000000000000003001";
pub const SEASON_ID: &str = "57a2d1e0000000000000000000003101";
/// The season's episodes, in order. The second has no image of its own: it shows the series' poster. Each runs 600 s, as the player's
/// test engine does, so playing one to its end marks it played.
pub const EPISODE_IDS: [&str; 2] = [
    "57a2d1e0000000000000000000003201",
    "57a2d1e0000000000000000000003202",
];
/// A collection (a `BoxSet`) of the first and third films, in a collections library the user's views do not list: only a query that
/// names it (its id, or it as the parent) finds it, so it leaves the other lists as they were.
pub const BOX_SETS_LIBRARY_ID: &str = "57a2d1e0000000000000000000001003";
pub const BOX_SET_ID: &str = "57a2d1e0000000000000000000006001";
/// The collection's films: linked children, which stay in the films library.
pub const BOX_SET_MEMBERS: [&str; 2] = [FILM_IDS[0], FILM_IDS[2]];
/// An actor in the first film and the series.
pub const PERSON_ID: &str = "57a2d1e0000000000000000000004001";
/// The director of the third film by name ("Example Film Two"). A search for people finds them too: Jellyfin's `Items` search answers
/// every person kind (`SqlSearchProvider` filters on `Type` only; jellyfin/jellyfin v12.2, ca0f16eb, read 2026-10-27).
pub const DIRECTOR_ID: &str = "57a2d1e0000000000000000000004002";

/// Who is credited on what: (person, item). `GET Items?personIds=` lists the items of a person.
const CREDITS: &[(&str, &str)] = &[
    (PERSON_ID, FILM_IDS[0]),
    (PERSON_ID, SERIES_ID),
    (DIRECTOR_ID, FILM_IDS[2]),
];

/// One item of the library.
struct Item {
    id: &'static str,
    /// What `includeItemTypes` filters on.
    kind: &'static str,
    name: &'static str,
    parent: &'static str,
    /// The rest of the record, without `Id`, `Name`, `Type` and `UserData`.
    fields: &'static str,
}

const ITEMS: &[Item] = &[
    Item {
        id: FILM_IDS[0],
        kind: "Movie",
        name: "Example Film One",
        parent: FILMS_LIBRARY_ID,
        fields: r#""RunTimeTicks":72000000000,"ProductionYear":2010,"IsFolder":false,"PrimaryImageAspectRatio":0.6666666666666666,"ImageTags":{"Primary":"57a2d1e0a0000001"},"BackdropImageTags":["57a2d1e0a0000002"],"ImageBlurHashes":{"Primary":{"57a2d1e0a0000001":"LEHV6nWB2yk8pyo0adR*.7kCMdnj"}}"#,
    },
    Item {
        id: FILM_IDS[1],
        kind: "Movie",
        name: "Example Film Three",
        parent: FILMS_LIBRARY_ID,
        fields: r#""RunTimeTicks":54000000000,"ProductionYear":2014,"IsFolder":false,"ImageTags":{},"BackdropImageTags":[]"#,
    },
    Item {
        id: FILM_IDS[2],
        kind: "Movie",
        name: "Example Film Two",
        parent: FILMS_LIBRARY_ID,
        fields: r#""RunTimeTicks":61200000000,"ProductionYear":2012,"IsFolder":false,"ImageTags":{"Primary":"57a2d1e0a0000003"},"BackdropImageTags":[]"#,
    },
    Item {
        id: SERIES_ID,
        kind: "Series",
        name: "Example Series",
        parent: SHOWS_LIBRARY_ID,
        fields: r#""ProductionYear":2018,"IsFolder":true,"Status":"Continuing","PrimaryImageAspectRatio":0.68,"ImageTags":{"Primary":"57a2d1e0a0000004"},"BackdropImageTags":[]"#,
    },
    Item {
        id: SEASON_ID,
        kind: "Season",
        name: "Season 1",
        parent: SERIES_ID,
        fields: r#""IndexNumber":1,"IsFolder":true,"SeriesName":"Example Series","SeriesId":"57a2d1e0000000000000000000003001","SeriesPrimaryImageTag":"57a2d1e0a0000004","ImageTags":{},"BackdropImageTags":[]"#,
    },
    Item {
        id: EPISODE_IDS[0],
        kind: "Episode",
        name: "Example Episode One",
        parent: SEASON_ID,
        fields: r#""RunTimeTicks":6000000000,"IndexNumber":1,"ParentIndexNumber":1,"IsFolder":false,"SeriesName":"Example Series","SeriesId":"57a2d1e0000000000000000000003001","SeasonId":"57a2d1e0000000000000000000003101","SeasonName":"Season 1","SeriesPrimaryImageTag":"57a2d1e0a0000004","ImageTags":{"Primary":"57a2d1e0a0000005"},"BackdropImageTags":[]"#,
    },
    Item {
        id: EPISODE_IDS[1],
        kind: "Episode",
        name: "Example Episode Two",
        parent: SEASON_ID,
        fields: r#""RunTimeTicks":6000000000,"IndexNumber":2,"ParentIndexNumber":1,"IsFolder":false,"SeriesName":"Example Series","SeriesId":"57a2d1e0000000000000000000003001","SeasonId":"57a2d1e0000000000000000000003101","SeasonName":"Season 1","SeriesPrimaryImageTag":"57a2d1e0a0000004","PrimaryImageAspectRatio":0.68,"ImageTags":{},"BackdropImageTags":[]"#,
    },
    Item {
        id: BOX_SET_ID,
        kind: "BoxSet",
        name: "Example Collection",
        parent: BOX_SETS_LIBRARY_ID,
        fields: r#""IsFolder":true,"ImageTags":{},"BackdropImageTags":[]"#,
    },
    // People are by-name items: in no library, so only a query that asks for their kind, without a parent, lists them
    // (`SearchQueryAccessFilter.Build`: "a by-name item belongs to no library ... A search scoped to a parent gets no exemption").
    Item {
        id: PERSON_ID,
        kind: "Person",
        name: "Example Actor",
        parent: "",
        fields: r#""IsFolder":false,"PrimaryImageAspectRatio":0.6666666666666666,"ImageTags":{"Primary":"57a2d1e0a0000006"},"BackdropImageTags":[],"ImageBlurHashes":{"Primary":{"57a2d1e0a0000006":"LEHV6nWB2yk8pyo0adR*.7kCMdnj"}}"#,
    },
    Item {
        id: DIRECTOR_ID,
        kind: "Person",
        name: "Example Director",
        parent: "",
        fields: r#""IsFolder":false,"ImageTags":{},"BackdropImageTags":[]"#,
    },
];

/// What `Items/{id}` adds for the first film: every field.
const FILM_DETAILS: &str = r#""OriginalTitle":"Example Original Title","Overview":"An example overview.","Taglines":["An example tagline."],"Genres":["Drama"],"Studios":[{"Name":"Example Studio","Id":"57a2d1e0000000000000000000005001"}],"People":[{"Name":"Example Actor","Id":"57a2d1e0000000000000000000004001","Role":"Example Role","Type":"Actor"}],"OfficialRating":"PG-13","CommunityRating":7.5,"PremiereDate":"2010-07-16T00:00:00.0000000Z","Chapters":[{"StartPositionTicks":0,"Name":"Chapter 1","ImageDateModified":"0001-01-01T00:00:00.0000000Z"}],"Container":"mkv","MediaSourceCount":1,"MediaSources":[{"Protocol":"File","Id":"57a2d1e0000000000000000000002001","Container":"mkv","MediaStreams":[{"Codec":"h264","Type":"Video","DisplayTitle":"1080p H264 SDR","Index":0},{"Codec":"aac","Language":"eng","Type":"Audio","DisplayTitle":"English - AAC - Stereo - Default","Index":1}]}]"#;

/// Whether `path` is a browse endpoint, which needs a signed-in user.
pub(crate) fn is_browse_path(path: &str) -> bool {
    [
        "/UserViews",
        "/Items",
        "/UserItems/",
        "/Shows/",
        "/UserPlayedItems/",
        "/UserFavoriteItems/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

/// What the user has marked, shared by every request. `played` holds items that are not folders: a series' or season's played state
/// is derived from its episodes.
#[derive(Default)]
pub(crate) struct Marks {
    pub played: HashSet<String>,
    pub favourite: HashSet<String>,
    /// When each item was last played, as a count that only goes up: Jellyfin's `LastPlayedDate`.
    last_played: HashMap<String, u64>,
    clock: u64,
    /// The cached instances of items that are not folders: their user data as last loaded or saved through them.
    instances: HashMap<String, Instance>,
    /// The items whose user data was saved since the last `UserDataChanged` went out, and their parents ([`Marks::saved`]).
    changed: Vec<String>,
}

/// What a cached instance holds of an item's user data.
#[derive(Clone, Copy)]
struct Instance {
    played: bool,
    favourite: bool,
}

impl Marks {
    /// `id`'s user data was saved: it goes out in the next `UserDataChanged`, with its parent. Jellyfin's `UserDataChangeNotifier`
    /// takes every save but a progress report's, and adds the item's parent ("`// Go up one level`": `GetOwner() ?? GetParent()`);
    /// v12.3, tag 06de749, `Emby.Server.Implementations/EntryPoints/UserDataChangeNotifier.cs`, read 2026-10-27.
    fn saved(&mut self, id: &str) {
        for id in std::iter::once(id).chain(find(id).map(|item| item.parent)) {
            if find(id).is_some() && !self.changed.iter().any(|known| known == id) {
                self.changed.push(id.to_owned());
            }
        }
    }

    /// Takes the items saved since the last call ([`Marks::saved`]), as `UserDataChanged`'s `UserDataList` holds them: each one's
    /// `UserItemDataDto` with its `ItemId`. `None` when nothing was saved.
    pub(crate) fn take_changed(&mut self) -> Option<String> {
        if self.changed.is_empty() {
            return None;
        }
        let changed = std::mem::take(&mut self.changed);
        let list: Vec<String> = changed.iter().map(|id| user_data(id, self)).collect();
        Some(format!("[{}]", list.join(",")))
    }

    /// Marks `id` played or unplayed as `POST`/`DELETE UserPlayedItems` does: a series or season through every episode below it
    /// (`Folder.MarkPlayed`). Played keeps an earlier last-played date ("`datePlayed ?? data.LastPlayedDate ?? DateTime.UtcNow`",
    /// `BaseItem.MarkPlayed`); unplayed clears it (`ResetPlayedState`).
    pub(crate) fn set_played(&mut self, id: &str, played: bool) {
        let box_set = id == BOX_SET_ID;
        let ids: Vec<&str> = match find(id) {
            Some(item) if is_folder(item) => leaves_below(id).map(|leaf| leaf.id).collect(),
            _ => vec![id],
        };
        for id in ids {
            // A collection's films are saved through their cached instances (`BaseItem.GetLinkedChild` takes them from
            // `GetItemById`), a series' or season's episodes through those `GetItemList` loads.
            let rehydrate = box_set;
            if played {
                self.played.insert(id.to_owned());
                if !self.last_played.contains_key(id) {
                    self.clock += 1;
                    self.last_played.insert(id.to_owned(), self.clock);
                }
            } else {
                self.played.remove(id);
                self.last_played.remove(id);
            }
            if rehydrate {
                self.rehydrate(id);
            }
            self.saved(id);
        }
    }

    /// A stop report for `id` at `position_ticks`, as `UserDataManager.UpdatePlayState` takes it with Jellyfin's defaults
    /// (`ServerConfiguration`: `MinResumePct = 5`, `MaxResumePct = 90`, `MinResumeDurationSeconds = 300`; v12.2 ca0f16eb, read
    /// 2026-10-27): below 5 % the position counts as 0 ("ignore progress during the beginning"); past 90 % or within a second of the
    /// end, or anything of an item shorter than 300 s, it is played ("mark as completed close to the end"). It is then last played
    /// now if it played to its end or kept a position (`SessionManager.OnPlaybackStopped`: "`if (playedToCompletion ||
    /// data.PlaybackPositionTicks > 0) data.LastPlayedDate = DateTime.UtcNow`"). The stand-in keeps no resume point itself.
    pub(crate) fn stopped(&mut self, id: &str, position_ticks: i64, failed: bool) {
        self.write_back(id);
        self.stop(id, position_ticks, failed);
        // `SessionManager.OnPlaybackStopped` saves the item it took from `GetItemById`.
        self.rehydrate(id);
    }

    fn stop(&mut self, id: &str, position_ticks: i64, failed: bool) {
        let Some(runtime) = find(id).and_then(runtime_ticks).filter(|ticks| *ticks > 0) else {
            return;
        };
        if failed || position_ticks <= 0 {
            return;
        }
        let percent = position_ticks as f64 / runtime as f64 * 100.0;
        let finished = if percent < 5.0 {
            false
        } else {
            percent > 90.0 || position_ticks >= runtime - 10_000_000 || runtime < 300 * 10_000_000
        };
        let kept = percent >= 5.0 && !finished;
        if finished {
            self.played.insert(id.to_owned());
        }
        if finished || kept {
            self.clock += 1;
            self.last_played.insert(id.to_owned(), self.clock);
        }
        self.saved(id);
    }
}

impl Marks {
    /// A playback start report for `id` (`SessionManager.OnPlaybackStart(User, BaseItem)`), saved through the item
    /// `GetNowPlayingItem` took from `GetItemById`: what its instance holds goes back into the database ([`Marks::write_back`]).
    pub(crate) fn started(&mut self, id: &str) {
        self.write_back(id);
        self.rehydrate(id);
        self.saved(id);
    }

    /// A mark saved through `id`'s own instance, which `GetItemById` caches if it was not: it holds the saved state.
    pub(crate) fn rehydrate(&mut self, id: &str) {
        if find(id).is_some_and(|item| !is_folder(item)) {
            let saved = Instance {
                played: self.played.contains(id),
                favourite: self.favourite.contains(id),
            };
            self.instances.insert(id.to_owned(), saved);
        }
    }

    /// A write through `id`'s cached instance, if it has one: Jellyfin reads the user data to change from the instance
    /// (`UserDataManager.GetUserData`: "`ResolveUserDataRow(item, item.UserData?.Where(e => e.UserId.Equals(user.Id)))`") and saves
    /// the whole row (`SaveUserData`: "`dbContext.UserData.Attach(userDataEntry).State = EntityState.Modified`"), so a played mark a
    /// folder's sweep left out of the instance goes back into the database. Done by a favourite (`UserLibraryController.MarkFavorite`:
    /// "`var data = _userDataRepository.GetUserData(user, item); data.IsFavorite = isFavorite;`"), a playback start
    /// (`SessionManager.OnPlaybackStart(User, BaseItem)`: "`var data = _userDataManager.GetUserData(user, item); data.PlayCount++;`")
    /// and a progress or stop report (`OnPlaybackProgress`, `OnPlaybackStopped`, through `UpdatePlayState`, which sets `Played` only
    /// near the end). Jellyfin v12.3 (tag 06de749) and v12.2 (ca0f16eb), read 2026-10-27; seen in the source only, not on a server.
    /// An upstream bug the client works around (`docs/notes/jellyfin-browse.md`, "Upstream bugs and our workarounds").
    pub(crate) fn write_back(&mut self, id: &str) {
        let Some(instance) = self.instances.get(id).copied() else {
            return;
        };
        self.saved(id);
        if instance.played {
            self.played.insert(id.to_owned());
        } else {
            self.played.remove(id);
        }
    }

    /// `id`'s cached instance, loaded with what the database holds now on its first read.
    fn instance(&mut self, id: &str) -> Instance {
        let fresh = Instance {
            played: self.played.contains(id),
            favourite: self.favourite.contains(id),
        };
        *self.instances.entry(id.to_owned()).or_insert(fresh)
    }
}

fn is_folder(item: &Item) -> bool {
    matches!(item.kind, "Series" | "Season" | "BoxSet")
}

/// Whether `item` is one of the collection's films.
fn in_box_set(item: &Item, box_set: &str) -> bool {
    box_set == BOX_SET_ID && BOX_SET_MEMBERS.contains(&item.id)
}

/// The episodes below `folder`, in order: what a folder's played state counts (`DescendantQueryHelper.IsCountableLeaf`: "`!b.IsFolder
/// && !b.IsVirtualItem`").
fn leaves_below(folder: &str) -> impl Iterator<Item = &'static Item> + '_ {
    ITEMS.iter().filter(move |item| {
        !is_folder(item) && (is_below(item, folder) || in_box_set(item, folder))
    })
}

/// The item's `RunTimeTicks`, read from its record.
fn runtime_ticks(item: &Item) -> Option<i64> {
    let rest = item.fields.split("\"RunTimeTicks\":").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// The series' next episode, as `GET Shows/NextUp` answers: the first unplayed episode after the played episode that comes last in
/// the series' order (`NextUpService.GetNextUpEpisodesBatch`: played episodes "`OrderByDescending(e => e.ParentIndexNumber)
/// .ThenByDescending(e => e.IndexNumber)`"), or the first unplayed one when none is played. Without `seriesId`, a series is asked
/// for only once one of its episodes has a last-played date (`GetNextUpSeriesKeys`); with it, whatever was played (what Jellyfin
/// answers for a series never started is not verified: the stand-in gives its first unplayed episode).
fn next_up<'a>(marks: &Marks, by_id: bool) -> Option<&'a Item> {
    let episodes: Vec<&Item> = leaves_below(SERIES_ID).collect();
    let started = episodes
        .iter()
        .any(|episode| marks.last_played.contains_key(episode.id));
    if !started && !by_id {
        return None;
    }
    // The stand-in lists its episodes in their order.
    let from = episodes
        .iter()
        .rposition(|episode| marks.played.contains(episode.id))
        .map_or(0, |at| at + 1);
    episodes
        .into_iter()
        .skip(from)
        .find(|episode| !marks.played.contains(episode.id))
}

/// A request to a browse endpoint.
pub(crate) struct Browse<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub query: &'a [(String, String)],
}

impl Browse<'_> {
    fn param(&self, key: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.as_str())
    }

    fn number(&self, key: &str) -> Option<usize> {
        self.param(key)?.parse().ok()
    }
}

/// The answer to a browse request, or `None` if the path is not one.
pub(crate) fn answer(
    request: &Browse<'_>,
    marks: &mut Marks,
    listed: &Views,
) -> Option<(u16, String)> {
    let path = request.path;
    if let Some(id) = path.strip_prefix("/UserPlayedItems/") {
        return Some(mark(marks, Mark::Played, id, request.method));
    }
    if let Some(id) = path.strip_prefix("/UserFavoriteItems/") {
        return Some(mark(marks, Mark::Favourite, id, request.method));
    }
    if request.method != "GET" {
        return None;
    }
    if let Some(answer) = single_item(request, marks) {
        return Some(answer);
    }
    let marks = &*marks;
    let answer = match path {
        "/UserViews" => {
            let with_hidden = request
                .param("includeHidden")
                .is_some_and(|value| value.eq_ignore_ascii_case("true"));
            let views = views(listed, with_hidden);
            let total = views.len();
            (200, result(&views, 0, Some(total)))
        }
        "/Items" => items(request, marks),
        "/Items/Latest" => {
            let parent = request.param("parentId").unwrap_or_default();
            let latest: Vec<String> = if parent == SHOWS_LIBRARY_ID {
                // New episodes of one series are grouped into it.
                vec![record(find(SERIES_ID)?, marks, r#","ChildCount":2"#)]
            } else {
                in_library(parent)
                    .map(|item| record(item, marks, ""))
                    .collect()
            };
            (200, format!("[{}]", latest.join(",")))
        }
        "/UserItems/Resume" => {
            let resumable = vec![record(find(FILM_IDS[1])?, marks, "")];
            (200, result(&resumable, 0, None))
        }
        "/Shows/NextUp" => {
            let next: Vec<String> = next_up(marks, request.param("seriesId").is_some())
                .map(|episode| record(episode, marks, ""))
                .into_iter()
                .collect();
            (200, result(&next, 0, None))
        }
        _ => return item_paths(request, marks),
    };
    Some(answer)
}

fn item_paths(request: &Browse<'_>, marks: &Marks) -> Option<(u16, String)> {
    if let Some(rest) = request.path.strip_prefix("/Shows/") {
        let (series, what) = rest.split_once('/')?;
        if series != SERIES_ID {
            return Some((404, String::new()));
        }
        let parent = match what {
            "Seasons" => SERIES_ID,
            "Episodes" => request.param("seasonId").unwrap_or(SEASON_ID),
            _ => return None,
        };
        // `startItemId` skips to that item; `startIndex` and `limit` page what follows (`TvShowsController.GetEpisodes`).
        let start = request.param("startItemId");
        let skip = request.number("startIndex").unwrap_or(0);
        let limit = request.number("limit").unwrap_or(usize::MAX);
        let children: Vec<String> = ITEMS
            .iter()
            .filter(|item| item.parent == parent)
            .skip_while(|item| start.is_some_and(|start| item.id != start))
            .skip(skip)
            .take(limit)
            .map(|item| record(item, marks, ""))
            .collect();
        let total = children.len();
        return Some((200, result(&children, 0, Some(total))));
    }
    let rest = request.path.strip_prefix("/Items/")?;
    // `Items/{id}` itself is a single-item read ([`single_item`]).
    let Some((id, "Similar")) = rest.split_once('/') else {
        return None;
    };
    let Some(item) = find(id) else {
        return Some((404, String::new()));
    };
    let others: Vec<String> = ITEMS
        .iter()
        .filter(|other| other.kind == item.kind && other.id != item.id)
        .map(|other| record(other, marks, ""))
        .collect();
    let total = others.len();
    Some((200, result(&others, 0, Some(total))))
}

/// `GET Items/{id}` and `GET UserItems/{id}/UserData`: an item that is not a folder answers its cached instance's user data.
fn single_item(request: &Browse<'_>, marks: &mut Marks) -> Option<(u16, String)> {
    let path = request.path;
    let (id, whole) = match path.strip_prefix("/UserItems/") {
        Some(rest) => (rest.strip_suffix("/UserData")?, false),
        None => {
            let id = path.strip_prefix("/Items/")?;
            (id, true)
        }
    };
    if id.contains('/') || id == "Latest" {
        return None;
    }
    let Some(item) = find(id) else {
        return Some((404, String::new()));
    };
    let data = if is_folder(item) {
        user_data(id, marks)
    } else {
        let instance = marks.instance(id);
        leaf_user_data(id, instance.played, instance.favourite)
    };
    if !whole {
        return Some((200, data));
    }
    let extra = if id == FILM_IDS[0] {
        format!(",{FILM_DETAILS}")
    } else {
        String::new()
    };
    Some((200, record_with(item, extra.as_str(), &data)))
}

/// `GET Items`: parent, kinds, a person's items (`personIds`), search term, paging and the total, sorted by name (Jellyfin orders a
/// search by its score first: with the stand-in's few names, by name is enough).
fn items(request: &Browse<'_>, marks: &Marks) -> (u16, String) {
    let parent = request.param("parentId");
    let recursive = request.param("recursive") == Some("true");
    let kinds: Option<Vec<&str>> = request
        .param("includeItemTypes")
        .map(|kinds| kinds.split(',').collect());
    let people: Option<Vec<&str>> = request
        .param("personIds")
        .map(|people| people.split(',').collect());
    let term = request.param("searchTerm").map(str::to_lowercase);
    // `ids` asks for those items wherever they are (`Folder.GetItems`: "`if (query.ItemIds.Length > 0)`", then
    // `LibraryManager.GetItemsResult(query)`; v12.3, read 2026-10-27).
    let ids: Option<Vec<&str>> = request.param("ids").map(|ids| ids.split(',').collect());
    let mut found: Vec<&Item> = ITEMS
        .iter()
        .filter(|item| match parent {
            _ if let Some(ids) = &ids => ids.contains(&item.id),
            // A person is in no library.
            _ if item.kind == "Person" => {
                parent.is_none()
                    && kinds
                        .as_ref()
                        .is_some_and(|kinds| kinds.contains(&"Person"))
            }
            Some(parent) if recursive => is_below(item, parent) || in_box_set(item, parent),
            Some(parent) => item.parent == parent || in_box_set(item, parent),
            // The collections library is not among the user's views.
            None => recursive && !is_below(item, BOX_SETS_LIBRARY_ID),
        })
        .filter(|item| {
            people.as_ref().is_none_or(|people| {
                CREDITS
                    .iter()
                    .any(|(person, credited)| *credited == item.id && people.contains(person))
            })
        })
        .filter(|item| {
            kinds
                .as_ref()
                .is_none_or(|kinds| kinds.contains(&item.kind))
        })
        .filter(|item| {
            term.as_ref()
                .is_none_or(|term| item.name.to_lowercase().contains(term))
        })
        .collect();
    found.sort_by_key(|item| item.name);
    if request.param("sortOrder") == Some("Descending") {
        found.reverse();
    }
    let start = request.number("startIndex").unwrap_or(0);
    let limit = request.number("limit").unwrap_or(usize::MAX);
    let page: Vec<String> = found
        .iter()
        .skip(start)
        .take(limit)
        .map(|item| record(item, marks, ""))
        .collect();
    let total = (request.param("enableTotalRecordCount") != Some("false")).then_some(found.len());
    (200, result(&page, start, total))
}

fn is_below(item: &Item, ancestor: &str) -> bool {
    let mut parent = item.parent;
    loop {
        if parent == ancestor {
            return true;
        }
        match find(parent) {
            Some(up) => parent = up.parent,
            None => return false,
        }
    }
}

fn in_library(library: &str) -> impl Iterator<Item = &'static Item> + '_ {
    ITEMS.iter().filter(move |item| item.parent == library)
}

fn find(id: &str) -> Option<&'static Item> {
    ITEMS.iter().find(|item| item.id == id)
}

/// One of the user's libraries as `/UserViews` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct View {
    pub id: String,
    pub name: String,
    /// The `CollectionType`, passed as given (unknown kinds too).
    pub collection: String,
}

/// The user's libraries: those listed, in the user's order, and those removed. Starts with the films and the shows.
#[derive(Debug)]
pub(crate) struct Views {
    pub listed: Vec<View>,
    /// Removed while the server still knows them: `GET Items` under one answers 401 with a valid token, as Jellyfin 12.2 does
    /// for a parent folder the user cannot see (`ItemsController.GetItems`).
    pub hidden: HashSet<String>,
    /// Removed and forgotten: `GET Items` under one answers 400, as Jellyfin 12.2 does for a parent id it does not know
    /// (`LibraryManager.GetParentItem`).
    pub forgotten: HashSet<String>,
    /// Kept out of "My media" (`MyMediaExcludes`): listed only with `includeHidden=true`, as Jellyfin 12.3's
    /// `UserViewManager.GetUserViews` does.
    pub excluded: HashSet<String>,
}

impl Default for Views {
    fn default() -> Self {
        let view = |id: &str, name: &str, collection: &str| View {
            id: id.to_owned(),
            name: name.to_owned(),
            collection: collection.to_owned(),
        };
        Self {
            listed: vec![
                view(FILMS_LIBRARY_ID, "Example Films", "movies"),
                view(SHOWS_LIBRARY_ID, "Example Shows", "tvshows"),
            ],
            hidden: HashSet::new(),
            forgotten: HashSet::new(),
            excluded: HashSet::new(),
        }
    }
}

impl Views {
    /// What `GET Items` under `parent` answers instead of its items, if anything: 401 for a hidden library, 400 for a forgotten
    /// one.
    pub(crate) fn refusal(&self, parent: Option<&str>) -> Option<u16> {
        let parent = parent?;
        if self.hidden.contains(parent) {
            Some(401)
        } else if self.forgotten.contains(parent) {
            Some(400)
        } else {
            None
        }
    }
}

fn views(views: &Views, with_hidden: bool) -> Vec<String> {
    views
        .listed
        .iter()
        .filter(|view| with_hidden || !views.excluded.contains(&view.id))
        .map(|view| {
            format!(
                r#"{{"Name":"{}","ServerId":"{}","Id":"{}","IsFolder":true,"Type":"CollectionFolder","CollectionType":"{}","ImageTags":{{}},"BackdropImageTags":[]}}"#,
                view.name,
                crate::SERVER_ID,
                view.id,
                view.collection
            )
        })
        .collect()
}

/// A `QueryResult`. Without a total (`enableTotalRecordCount=false`), Jellyfin sends 0.
fn result(items: &[String], start: usize, total: Option<usize>) -> String {
    format!(
        r#"{{"Items":[{}],"TotalRecordCount":{},"StartIndex":{start}}}"#,
        items.join(","),
        total.unwrap_or(0)
    )
}

fn record(item: &Item, marks: &Marks, extra: &str) -> String {
    record_with(item, extra, &user_data(item.id, marks))
}

fn record_with(item: &Item, extra: &str, user_data: &str) -> String {
    format!(
        r#"{{"Name":"{}","ServerId":"{}","Id":"{}","Type":"{}",{}{extra},"UserData":{}}}"#,
        item.name,
        crate::SERVER_ID,
        item.id,
        item.kind,
        item.fields,
        user_data
    )
}

/// `UserItemDataDto`. A series' or season's `Played`, `UnplayedItemCount` and `PlayedPercentage` come from its episodes
/// (`Folder.FillUserDataDtoValues`); its own position and play count stay 0.
fn user_data(id: &str, marks: &Marks) -> String {
    let favourite = marks.favourite.contains(id);
    if find(id).is_some_and(is_folder) {
        let total = leaves_below(id).count();
        let played = leaves_below(id)
            .filter(|leaf| marks.played.contains(leaf.id))
            .count();
        let percentage = if total > 0 {
            format!(
                ",\"PlayedPercentage\":{}",
                played as f64 / total as f64 * 100.0
            )
        } else {
            String::new()
        };
        return format!(
            r#"{{"PlaybackPositionTicks":0{percentage},"UnplayedItemCount":{},"PlayCount":0,"IsFavorite":{favourite},"Played":{},"Key":"{id}","ItemId":"{id}"}}"#,
            total - played,
            played >= total
        );
    }
    leaf_user_data(id, marks.played.contains(id), favourite)
}

/// The user data of an item that is not a folder.
fn leaf_user_data(id: &str, played: bool, favourite: bool) -> String {
    // The second film is half watched until it is marked played.
    let (position, percentage) = if id == FILM_IDS[1] && !played {
        (27_000_000_000_i64, ",\"PlayedPercentage\":50")
    } else {
        (0, "")
    };
    format!(
        r#"{{"PlaybackPositionTicks":{position}{percentage},"PlayCount":{},"IsFavorite":{favourite},"Played":{played},"Key":"{id}","ItemId":"{id}"}}"#,
        u8::from(played)
    )
}

#[derive(Clone, Copy)]
enum Mark {
    Played,
    Favourite,
}

/// `POST` sets the mark, `DELETE` clears it; both answer the item's user data as it is after the change, a folder's derived from its
/// episodes (`PlaystateController.UpdatePlayedStatus`: "`return _userDataRepository.GetUserDataDto(item, user)`").
fn mark(marks: &mut Marks, which: Mark, id: &str, method: &str) -> (u16, String) {
    if find(id).is_none() {
        return (404, String::new());
    }
    let on = match method {
        "POST" => true,
        "DELETE" => false,
        _ => return (405, String::new()),
    };
    match which {
        Mark::Played => marks.set_played(id, on),
        Mark::Favourite if on => {
            marks.write_back(id);
            marks.favourite.insert(id.to_owned());
            marks.saved(id);
        }
        Mark::Favourite => {
            marks.write_back(id);
            marks.favourite.remove(id);
            marks.saved(id);
        }
    }
    // Saved through the item's own instance (`PlaystateController` and `UserLibraryController` take it from `GetItemById`).
    marks.rehydrate(id);
    (200, user_data(id, marks))
}
