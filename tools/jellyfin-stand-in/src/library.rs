//! A small fake library for the browse endpoints (ADR 0018), in Jellyfin 12.2's shapes: PascalCase names, nulls left out, ids as 32
//! hex digits. Two libraries: films and shows, with one series of one season. Libraries can be added, renamed, reordered and
//! removed while it runs ([`crate::Controls::add_library`] and the others). Every name is an example.

use std::collections::HashSet;

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
/// The season's episodes, in order. The second has no image of its own: it shows the series' poster.
pub const EPISODE_IDS: [&str; 2] = [
    "57a2d1e0000000000000000000003201",
    "57a2d1e0000000000000000000003202",
];
/// An actor in the first film.
pub const PERSON_ID: &str = "57a2d1e0000000000000000000004001";

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
        fields: r#""RunTimeTicks":13000000000,"IndexNumber":1,"ParentIndexNumber":1,"IsFolder":false,"SeriesName":"Example Series","SeriesId":"57a2d1e0000000000000000000003001","SeasonId":"57a2d1e0000000000000000000003101","SeasonName":"Season 1","SeriesPrimaryImageTag":"57a2d1e0a0000004","ImageTags":{"Primary":"57a2d1e0a0000005"},"BackdropImageTags":[]"#,
    },
    Item {
        id: EPISODE_IDS[1],
        kind: "Episode",
        name: "Example Episode Two",
        parent: SEASON_ID,
        fields: r#""RunTimeTicks":13000000000,"IndexNumber":2,"ParentIndexNumber":1,"IsFolder":false,"SeriesName":"Example Series","SeriesId":"57a2d1e0000000000000000000003001","SeasonId":"57a2d1e0000000000000000000003101","SeasonName":"Season 1","SeriesPrimaryImageTag":"57a2d1e0a0000004","PrimaryImageAspectRatio":0.68,"ImageTags":{},"BackdropImageTags":[]"#,
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

/// What the user has marked, shared by every request.
#[derive(Default)]
pub(crate) struct Marks {
    pub played: HashSet<String>,
    pub favourite: HashSet<String>,
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
    let marks = &*marks;
    let answer = match path {
        "/UserViews" => {
            let views = views(listed);
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
            let next = EPISODE_IDS
                .iter()
                .filter(|id| !marks.played.contains(**id))
                .take(1)
                .filter_map(|id| Some(record(find(id)?, marks, "")))
                .collect::<Vec<_>>();
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
    let (id, similar) = match rest.split_once('/') {
        Some((id, "Similar")) => (id, true),
        Some(_) => return None,
        None => (rest, false),
    };
    let Some(item) = find(id) else {
        return Some((404, String::new()));
    };
    if similar {
        let others: Vec<String> = ITEMS
            .iter()
            .filter(|other| other.kind == item.kind && other.id != item.id)
            .map(|other| record(other, marks, ""))
            .collect();
        let total = others.len();
        return Some((200, result(&others, 0, Some(total))));
    }
    let extra = if id == FILM_IDS[0] {
        format!(",{FILM_DETAILS}")
    } else {
        String::new()
    };
    Some((200, record(item, marks, &extra)))
}

/// `GET Items`: parent, kinds, search term, paging and the total, sorted by name.
fn items(request: &Browse<'_>, marks: &Marks) -> (u16, String) {
    let parent = request.param("parentId");
    let recursive = request.param("recursive") == Some("true");
    let kinds: Option<Vec<&str>> = request
        .param("includeItemTypes")
        .map(|kinds| kinds.split(',').collect());
    let term = request.param("searchTerm").map(str::to_lowercase);
    let mut found: Vec<&Item> = ITEMS
        .iter()
        .filter(|item| match parent {
            Some(parent) if recursive => is_below(item, parent),
            Some(parent) => item.parent == parent,
            None => recursive,
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

fn views(views: &Views) -> Vec<String> {
    views
        .listed
        .iter()
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
    format!(
        r#"{{"Name":"{}","ServerId":"{}","Id":"{}","Type":"{}",{}{extra},"UserData":{}}}"#,
        item.name,
        crate::SERVER_ID,
        item.id,
        item.kind,
        item.fields,
        user_data(item.id, marks)
    )
}

/// `UserItemDataDto`.
fn user_data(id: &str, marks: &Marks) -> String {
    let played = marks.played.contains(id);
    let favourite = marks.favourite.contains(id);
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

/// `POST` sets the mark, `DELETE` clears it; both answer the item's user data.
fn mark(marks: &mut Marks, which: Mark, id: &str, method: &str) -> (u16, String) {
    if find(id).is_none() {
        return (404, String::new());
    }
    let set = match which {
        Mark::Played => &mut marks.played,
        Mark::Favourite => &mut marks.favourite,
    };
    match method {
        "POST" => {
            set.insert(id.to_owned());
        }
        "DELETE" => {
            set.remove(id);
        }
        _ => return (405, String::new()),
    }
    (200, user_data(id, marks))
}
