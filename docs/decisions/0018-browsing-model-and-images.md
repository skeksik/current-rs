# 0018: Browsing: items, queries, paged lists and images

Status: proposed (2026-10-25), from the browse spike; accepted when its PR merges; open choices decided by the owner 2026-10-25
(see "Decided by the owner"). Amended 2026-10-25 (fix/library-changes): the navigation's libraries follow the server, below under
"Libraries that follow the server". Amended 2026-10-27 (owner, fix/windows-ui-polish): library search (sections 2 and 3), the
filters (section 3), lists chosen by marks (section 5), Home's rows (section 6), the Favourites and Recently added views and
remembered choices (section 7). Amended 2026-10-27 (owner, feat/global-search-filters): global search filters, people and a person's
titles ("Global search filters").
Research, quotes and sources: `docs/notes/jellyfin-browse.md`. Playback, media sources and streams are ADR 0019's (spike/playback-design);
section 6 of the notes lists what is left to it.

## Context
M2 makes a Jellyfin library browsable on Windows: Home, library grids, detail pages, search, played and favourite (docs/roadmap.md).
The core already has the identity types and `Artwork` (ADR 0005), `ObservableList` and diff lists across the bridge (ADR 0004), and
per-address gateway credentials that every request to a server must carry, images included (ADR 0017: "UI image loaders cannot add
headers").

What the research found:
- **Jellyfin 12.2 serves everything we need from a handful of endpoints**, and list answers can be trimmed: `fields`,
  `enableImageTypes`, `imageTypeLimit` and `enableUserData` control what each item carries; `Items/{id}` always returns all fields.
- **Paging is by index** (`startIndex`, `limit`) with a `TotalRecordCount` for the whole query. **Search is not pageable**: 12.2 asks
  its search providers for `3 × limit` candidates and pages within those. (Amended 2026-10-27: a library's own search box lives with
  this: see section 3, "Library search".)
- **An image's tag is its version**: with `tag`, Jellyfin answers `Cache-Control: ... immutable` and an `ETag`. A cache keyed by
  item, image type, index, tag and size never needs revalidation.
- **`fillWidth`/`fillHeight`** return an image that covers the box at the source's aspect ratio, never upscaled.
- **WinUI**: `GridView` supports random-access data virtualisation with `IItemsRangeInfo` and placeholders, which must not be `null` in
  WinUI 3 (not verified: see the correction in section 3). A collection `Reset` costs the scroll position. `BitmapImage` reads JPEG, PNG, BMP, GIF, TIFF, JPEG XR and ICO, not WebP,
  from a URI or from a stream through `SetSourceAsync`.
- **`Artwork.url` cannot stay a URL**: the address changes with roaming (ADR 0014) and addresses must not cross the bridge.

## Decision

### 1. Items in `current-model` (amends ADR 0005)
Pure records, no I/O, no serde, bridge-ready like the rest of the model.

- **`ImageRef`**: `item: ItemRef` (whose image: an episode's row may show its series' poster), `kind: ImageType`, `index: u32` (for
  backdrops after the first), `tag: String` (the server's version of the image). **`Artwork` keeps `kind`, `aspect_ratio`,
  `blurhash` and replaces `url` with `image: ImageRef`.** The service crate builds the URL when it fetches, from the address that
  serves at that moment.
- **`UserState`**: `played: bool`, `position: Option<Duration>` (resume point), `progress: Option<f32>` (0.0-1.0, from
  `PlayedPercentage`), `favourite: bool`, `unplayed_count: Option<u32>` (series, seasons, folders), `play_count: u32`.
- **`ItemSummary`**, what grids and rows show: `item: ItemRef`, `kind: MediaKind`, `name`, `year: Option<i32>`,
  `runtime: Option<Duration>`, `artwork: Vec<Artwork>` (at most one per image type, so primary, thumb and backdrop are available to
  pick per layout), `user: UserState`, `episode: Option<EpisodeLinks>` and `child_count: Option<u32>` (the number of new episodes when
  "latest" groups them).
  - `EpisodeLinks`: `series: Option<ItemRef>`, `series_name`, `season: Option<ItemRef>`, `season_name`, `number: Option<u32>`,
    `number_end: Option<u32>`, `season_number: Option<u32>`.
- **`Library`**: `item: ItemRef`, `name`, `collection: CollectionKind`, `artwork`. `CollectionKind` is a new open value (`MOVIES`,
  `TV_SHOWS`, `MUSIC`, `MUSIC_VIDEOS`, `HOME_VIDEOS`, `BOX_SETS`, `BOOKS`, `PHOTOS`, `PLAYLISTS`; anything else passes through).
- **`ItemDetails`**, what a detail page shows: `summary: ItemSummary`, `original_title`, `overview`, `taglines`, `genres: Vec<String>`,
  `studios: Vec<String>`, `people: Vec<Person>`, `official_rating: Option<String>`, `community_rating: Option<f32>`,
  `critic_rating: Option<f32>`, `premiere: Option<CalendarDate>`, `end: Option<CalendarDate>`, `status: Option<String>` (a series'
  "Continuing"/"Ended", as the server writes it), `backdrops: Vec<Artwork>`, `logo: Option<Artwork>`, `chapters: Vec<Chapter>`
  (`start: Duration`, `name`), `media: Option<MediaSummary>`.
  - `Person`: `person: ItemRef`, `name`, `role: Option<String>`, `kind: PersonKind` (open: `ACTOR`, `DIRECTOR`, `WRITER`, ...),
    `image: Option<Artwork>`.
  - `CalendarDate`: `year`, `month`, `day`. The model has no date type and takes no dependency for one.
  - **`MediaSummary`** (thin, by agreement with ADR 0019): `source_count: u32` (versions), `container: Option<String>`,
    `video: Option<StreamSummary>`, `audio: Vec<StreamSummary>`, `subtitles: Vec<StreamSummary>`, each `StreamSummary` being
    `label: String` (the server's display title), `language: Option<String>`, `codec: Option<String>`. These describe the default
    version only. ADR 0019 owns `MediaSource` and the full stream types; it may extend `StreamSummary` or replace it with its own
    types and map `MediaSummary` from them, without a second set of names.
- **Open values for keys**: `SortKey` (`SORT_NAME`, `DATE_CREATED`, `PREMIERE_DATE`, `PRODUCTION_YEAR`, `COMMUNITY_RATING`,
  `RUNTIME`, `DATE_PLAYED`, `PLAY_COUNT`, `RANDOM`, ...) with `SortOrder` (`Ascending`, `Descending`, a closed enum: Current defines
  it), and `ItemFilter` (`PLAYED`, `UNPLAYED`, `FAVOURITE`, `RESUMABLE`, ...). New `MediaKind` constants as browsing maps them
  (`MUSIC_ALBUM`, `AUDIO`, `MUSIC_ARTIST`, `MUSIC_VIDEO`, `VIDEO`, `FOLDER`, `BOX_SET`); every other Jellyfin type passes through.
  Music, books and photos are browsable as open values and open in a generic detail page; nothing in M2 plays them.
- `Duration` is `std::time::Duration` (the model stays dependency-free). Jellyfin's ticks (100 ns) convert in `current-jellyfin`.

### 2. Queries in `current-jellyfin`
Wire types stay private; each method returns model types. Every list query sends one of two field sets:
- **Summary**: `fields=PrimaryImageAspectRatio`, `enableImageTypes=Primary,Thumb,Backdrop`, `imageTypeLimit=1`, user data on (the
  default). Episodes add the series' primary image from `SeriesPrimaryImageTag` when the episode has none (whether that field comes without
  an `ItemFields` value was not verified; task 1 checks it). Checked 2026-10-25: it does, for episodes and seasons
  (`docs/notes/jellyfin-browse.md`, "The item record").
- **Details**: `Items/{id}` as it is (all fields).

| Method | Endpoint | Notes |
|---|---|---|
| `libraries()` | `GET UserViews` | the server returns the user's order and drops hidden views |
| `items(query, start, limit)` | `GET Items` | `parentId`, `includeItemTypes`, `recursive`, `sortBy`/`sortOrder` (always ending with `SortName`), `filters`, `genres`, `years`, `nameStartsWith`; `enableTotalRecordCount` only on a first page and a refresh |
| `latest(library, limit)` | `GET Items/Latest` | `parentId` per library, `groupItems=true`; a bare array |
| `resume(limit)` | `GET UserItems/Resume` | `mediaTypes=Video`, `enableTotalRecordCount=false` |
| `next_up(limit, cutoff)` | `GET Shows/NextUp` | `enableResumable=false` (no duplicate of "continue watching"), `enableTotalRecordCount=false`; `nextUpDateCutoff` 365 days back until a setting exists |
| `seasons(series)` | `GET Shows/{id}/Seasons` | |
| `episodes(series, season)` | `GET Shows/{id}/Episodes` | `seasonId` |
| `similar(item, limit)` | `GET Items/{id}/Similar` | `fields` as summary |
| `search(term, kinds, limit)` | `GET Items?searchTerm=` | one request per result group (movies, series, episodes, ..., people last; amended 2026-10-27), `recursive=true`, one page of up to 24 each, or 100 of the one type a filter keeps; not paged (see Context) |
| `items(query)` with `person` (amended 2026-10-27) | `GET Items?personIds=` | a person's titles: the items "containing the specified person id" (`ItemsController.GetItems`) |
| `items(query)` with `search_term` (amended 2026-10-27) | `GET Items?searchTerm=` | a library grid's search: the grid's `parentId` and kinds, `searchTerm`, **no `sortBy`, `sortOrder` or `filters`**, so the server's relevance order comes back; paged as any grid, within the `3 × limit` window (section 3) |
| `details(item)` | `GET Items/{id}` | |
| `set_played(item, bool)` | `POST`/`DELETE UserPlayedItems/{id}` | answers the item's user data |
| `set_favourite(item, bool)` | `POST`/`DELETE UserFavoriteItems/{id}` | answers the item's user data |
| `image(request)` | `GET Items/{id}/Images/{type}[/{index}]` | section 4 |

The user's "exclude from latest" libraries (`Configuration.LatestItemsExcludes`, read at sign-in and with the current user) are kept with
the session so Home can skip them. (Corrected 2026-10-25: sign-in did not read it before task 1.)

### 3. Paged lists in `current-app`
A library grid is a **`PagedItems`**: an `ObservableList<Slot>` sized to the query's total, where `Slot` is `Loading`, `Item(ItemSummary)`
or `Failed`. Placeholders are records, never null.
(Corrected 2026-10-25, task 4: "ListView and GridView data virtualization", learn.microsoft.com, ms.date 2026-03-16, says a source
may "return either `null` or a placeholder item"; that WinUI 3 crashes on null placeholders is not verified. Current uses placeholder
records anyway, so a tile template always has a slot to bind to.)

- **Loading.** The first request asks for page 0 with the total count, then the list is reset once to `total` slots (page 0 filled, the
  rest `Loading`). The UI reports what it shows with `set_visible_range(first, last)`; the core loads the pages that cover it plus one
  page either side, at most two requests at a time, the newest range first, and cancels requests for pages that left the window. A
  loaded page fills its slots with `Set` diffs in one batch. **Page size: 100** (what jellyfin-web uses; "prefer chunky to chatty").
  Loaded pages stay loaded for the life of the list; a list of 10,000 summaries is a few megabytes (not measured), so no pruning in M2.
- **Backlog.** The list is built `with_backlog` of at least eight pages of diffs, so a busy UI is not reset (a reset costs the scroll
  position).
- **Refresh** (the user asks, the page is shown again after five minutes, the network changed) **never resets.** The core re-reads the
  loaded pages nearest the visible range with the total count; slots whose item changed get `Set`; a smaller or larger total removes
  or appends slots at the end; other loaded pages become stale and reload when they are next visible. The scroll position stays
  because the UI's collection only sees `Set`, `Append` and `Remove`. A new sort or filter is a new query and does reset: the grid
  starts at the top, as a re-sorted list does.
- **A failed page** turns its slots into `Failed`; the UI shows those tiles empty with a retry glyph, and the list's state value
  (`PagedState { total, loading, failed: Option<NetProblem> }`) drives one message above the grid with a Retry button. Retry reloads
  every failed page; the next scroll that reaches a failed page retries it once by itself.
- **Short lists** (home rows, seasons, episodes, similar, search groups, details) are plain `ObservableList<ItemSummary>` or a value,
  loaded in one request.
- **Library search (amended 2026-10-27, owner).** A grid's own search box loads `library_search(query, term)`: from the query the grid
  showed before searching, the text trimmed, with no sort and no filters, so the best matches come first; blank text gives that query
  back, its sort and filters with it. `library_grid_tools(query)` says Sort and Filter are unavailable while a query searches, with
  their tooltips ("Sorted by relevance while searching", "Filters are off while searching"); the UI only greys them out. Once a
  search's total reaches the server's window (`LIBRARY_SEARCH_WINDOW`, 3 × `PAGE_SIZE` = 300), `library_search_note(query, total)`
  gives the line "Showing the best 300 matches. Type more to narrow it." A search is a new query, so it starts at the top.
- **Filters (amended 2026-10-27, owner).** A grid offers one choice of everything, unplayed or played (`library_filters`), and
  favourites as a tick beside it, on or off whatever the choice (`library_toggle_filters`); `with_filter_choice` and
  `with_filter_toggle` change one and keep the other, and a grid with both gets the items that match both: `ItemsController` calls
  `query.ApplyFilters(filters)`, which sets `IsPlayed` (true for played, false for unplayed) and `IsFavorite = true` on one query,
  and the repository applies each as its own `Where` (`Jellyfin.Api/Controllers/ItemsController.cs`,
  `MediaBrowser.Controller/Entities/InternalItemsQuery.cs`, `Jellyfin.Server.Implementations/Item/BaseItemRepository.TranslateQuery.cs`;
  jellyfin/jellyfin master, read 2026-10-27). Played and unplayed together throw "Conflicting filters"; the single choice never
  sends both.

### 4. Images
**The core fetches every image** through the server's `RoamingAddresses`, so the current address and its gateway credentials apply
(ADR 0014, ADR 0017), and hands the UI the bytes.

- **Request**: the UI asks for an `ImageRef` at a box size in physical pixels: the slot's size in view pixels times
  `XamlRoot.RasterizationScale` on Windows. The core rounds the width up to a bucket (160, 240, 320, 480, 640, 960, 1280, 1920, 2560,
  3840) and scales the height by the box's ratio, so a few sizes per layout serve every scale.
- **URL**: `fillWidth`, `fillHeight`, `quality=90` (the server's default), `tag`, `imageIndex` when non-zero; `Accept: image/jpeg,
  image/png` and no `format`, so the server sends JPEG, or PNG when the image needs transparency (logos), never WebP.
- **Delivery: bytes over UniFFI** (open choice 2). `Core::load_image(request, cancel)` returns the encoded bytes. In C#, an
  `ImageLoader` adapter turns them into a `BitmapImage` through `MemoryStream.AsRandomAccessStream()` and `SetSourceAsync`, with
  `DecodePixelWidth` set to the box width, and cancels when the tile is recycled. No path and no address crosses the bridge, the UI
  never reads the cache folder, and an eviction can never pull a file from under the UI.
- **Memory cache**: encoded bytes, least recently used, 64 MB. Identical requests in flight share one fetch. At most six image fetches
  per server at a time; the newest request goes first, since the user scrolled to it.
- **Disk cache** (open choices 3, 4 and 7):
  - Location: a cache folder the UI passes at start, next to the data folder argument (amends ADR 0013): `%LOCALAPPDATA%\Current\Cache`
    on Windows, inside the per-user data tree and never next to the program (ADR 0009); `~/Library/Caches/<bundle id>` on macOS (not
    verified). The core never looks folders up.
  - **Per server**: `images/<hash(server ref)>/<hash(server ref, item id, image type, index, tag, width, height)>`. No title, item name,
    address or user name goes into a file name or a log line; logs name the server key and the short hash only.
  - Written to a temporary file and renamed. An index of sizes and last-access times is built from the folder at start; a hit
    refreshes the last-access time at most once a minute per file.
  - Limit **500 MB** by default; when exceeded, the least recently used files are deleted down to 90%, on a blocking thread.
  - **Signing out of a server or removing it deletes its folder** (in `App::sign_out` and `App::remove_server`), and at start the
    core deletes folders of servers that are no longer saved or have no signed-in user.
- **BlurHash**: the core decodes `Artwork.blurhash` into a small BMP (at most 32 pixels on the long side, at the image's aspect ratio)
  with our own decoder (about a hundred lines from the published format, no dependency), through the same `load_image` path with a
  `blurhash` request kind. BMP needs no encoder and every UI reads it (open choice 5).
- No new dependency. The cache key hash is our own (open choice 7).

### 5. Played and favourite
One **`ItemStates`** store in `current-app`, per session, keyed by `ItemRef`:
1. `set_played(item, true)` (or favourite) records the new `UserState` as pending and broadcasts `(ItemRef, UserState)` inside the core.
2. **Every live list subscribes to the store** and patches its slots for that `ItemRef` with `Set` (each list keeps an
   `ItemRef → indices` map): grids, home rows, details, search. Items loaded while a change is pending take the pending state.
3. The request goes out; requests for one item are sent one at a time, and only the last wanted state is sent after one in flight.
4. Success: the server's answer becomes the item's state. Then the lists whose content depends on it reload in the background, a
   second after the last change: "continue watching", "next up", and the parent series and season (their unplayed count).
   (Amended 2026-10-27, owner: **every list chosen by marks** reloads too, after a played or a favourite change alike: Home's
   Favourites and Seen recently, the Favourites view, and a grid filtered by a mark or sorted by date played
   (`ItemQuery::depends_on_marks`). Before, a new favourite did not appear in them until a manual refresh.)
5. Failure: the store rolls back to the state before the change, broadcasts it, and raises one notice (`ActionFailed { action, problem }`)
   that the UI shows as a message. The item's name may appear in the message; it never goes to the log.
6. (**Amended 2026-10-27 (owner)**, fix/series-episode-marks.) **A series and its episodes.** Jellyfin
   marks a series or season by marking every episode below it, not the folder, and derives the folder's played state, unplayed count
   and played percentage from its episodes: `Folder.MarkPlayed` ("Sweep through recursively and update status", over `Recursive =
   true, IsFolder = false`) and `Folder.FillUserDataDtoValues` (`dto.UnplayedItemCount = unplayedCount; dto.PlayedPercentage =
   playedCount / (double)totalCount * 100; dto.Played = playedCount >= totalCount`), counting episodes that are neither folders nor
   virtual (`DescendantQueryHelper.IsCountableLeaf`). The mark request answers the folder's user data computed after the sweep
   (`PlaystateController.UpdatePlayedStatus`), as `GET UserItems/{id}/UserData` does (`ItemsController.GetItemUserData`). Jellyfin
   v12.2 (ca0f16eb), the same on master (02c2951e), read 2026-10-27. So:
   - **at once**, a played change on a series or season shows on every loaded episode and season below it, in every live list.
     An episode loaded while the change is pending takes it too, unless its own change is newer. A failure rolls them all back with
     it, those loaded meanwhile included, and the item's page and its parents' read the server again a second later. A played change on an episode or season moves its loaded series' and season's unplayed count at once (one for an episode, a
     season's own count when marked played), and they show played when nothing below is unplayed;
   - **read again** with the dependent reload a second after the server answered a played change (or a stop was reported): the
     series' and season's own marks (the posters' unplayed count and progress), with `GET UserItems/{id}/UserData`. That is one
     request per series or season however many changes came within the second. The estimate is shown in every list loaded meanwhile,
     until its read-back answers; the read-back corrects it where the server counts what the lists don't show. A read-back older than
     a newer played change is dropped, and the item is read again after the next delay. A failed read-back keeps the estimate and
     raises no notice;
   - **reload** a second later, with the dependents of point 4: the marked series' or season's own page (its episodes and seasons)
     and an episode's page whose series or season changed. The reload and the read-backs wait while a played change still waits for
     the server; its answer schedules them again.

   Not verified on a server: missing (virtual) episodes. Jellyfin's sweep marks them only when the user displays missing episodes,
   and they never count; a page that lists them may show them marked at once, before the reload a second later shows the server's
   state.

   **Amended 2026-10-27 (owner):** a workaround for an upstream
   Jellyfin bug, as a wording note under this point. *An item's own read (`GET Items/{id}`) is not trusted for its marks after a
   folder's sweep* (`docs/notes/jellyfin-browse.md`, "Upstream bugs and our workarounds"). Jellyfin v12.3 (tag 06de749, read 2026-10-27;
   v12.2 at ca0f16eb has the same lines) answers it from its cached instance of the item (`LibraryManager.GetItemById`: "`if (_cache.TryGet(id, out var
   item))`"), whose user data only a mark on the item's own id saves (`SaveUserData`: "`// rehydrate the cached userdata`"), while
   `Folder.MarkUnplayed` saves the instances `GetItemList` loads. So a detail page takes the item's marks from a list read of it
   (`GET Items?ids={id}`, whose user data `GetUserDataBatch` reads fresh), sent with the item's own read; and where two live lists'
   copies of an item differ, the one read or patched last is the item's state for the UI's actions (`Browse::user_state`). The
   series' and season's own read-backs (`GET UserItems/{id}/UserData`) stay: their played state, unplayed count and percentage are
   derived from their episodes by count queries (`Folder.FillUserDataDtoValues`), not from the cached instance. The workaround goes
   once a single-item read serves fresh user data after a folder's sweep: re-check on each new server baseline.

### 6. Home
Home's layout is **provisional**: the owner accepted it for now and will have it redesigned later (2026-10-25). The `HomeFeed`
shape below (rows, each with its own list) is meant to survive that redesign; the choice and order of rows will change.

Home is a **`HomeFeed`**: a list of rows, each with its own item list.
- **Rows, in order**: Continue watching (`resume`), Next up (`next_up`), then Latest in each library in the user's library order
  (open choice 6). A row is shown only when it has items, so no row assumes a library type exists: "Next up" appears because the
  server returned episodes, not because a TV library is present. Latest skips libraries in the user's "exclude from latest" list
  and collection kinds without a meaningful "latest" (`PLAYLISTS`, `BOX_SETS`, and the views jellyfin-web also skips).
- Rows load in parallel; each appears as soon as it has items, in its place. A failed row is left out and one message offers Retry.
  (Amended 2026-10-25, task 3: on a later load, a row that fails keeps the items it showed from the last load; only a row that never
  loaded is left out. The message still offers Retry.)
- Without a signed-in Jellyfin server, Home shows the M1 empty state (add a server); browse features read
  `Capabilities::is_available(&ServiceType::JELLYFIN)` and hide otherwise. An unreachable server keeps its rows from the last load,
  marked offline, and images from the disk cache still show.
- Refresh: on start, on return to the foreground (the existing session hook), after played or favourite changes (the dependent rows),
  after playback stops (an event ADR 0019 provides), and on request.
- Libraries are reached from the app's navigation (one entry per library), not from a tile row on Home (open choice 6).
- **Amended 2026-10-27 (owner): the rows, in order**: Continue watching, Next up, **Favourites** (the user's favourites in every
  library, by name), **Recently added** (every library, newest first), **Recently added in X** per library (the old "Latest in X",
  renamed; the user's library order, the same skips), then **Seen recently** (played items, by date played). Favourites and Seen
  recently reload when a mark changes (section 5). A row header links to the full list: Favourites and Recently added to their
  views, "Recently added in X" to library X's grid sorted newest first. **The Favourites view is rows**, not a grid: favourite films,
  series and episodes, a row each in that order, each only while it has items (`Browse::favourites`, the search results' shape,
  read again when a mark changes); a row's header opens every item of its kind across the libraries (`kind_query`) with the
  favourites tick on (`favourites_query(Some(kind))`). A series stands for its episodes in the rows chosen across
  libraries (Favourites, Recently added); Seen recently shows films, episodes and other videos. `HomeRowKind` gains `Favourites`,
  `RecentlyAdded` and `SeenRecently`; `Latest` keeps its name on the bridge.

### Libraries that follow the server (amended 2026-10-25)
On the owner's server, a library added while the app ran showed on Home but not in the navigation until a restart, and a removed one
kept its entry: the UI read `libraries()` once per session. Now each session's `Browse` holds the user's libraries as a value
(`Browse::library_list`, across the bridge `subscribe_libraries`): `None` until first loaded, then the last list loaded, in the user's
order, unknown collection kinds passed through. It loads when the session's browse state is made and again on a refresh, the
foreground refresh, Home's reload (which joins a load already running, so one `GET UserViews` serves both), after a library request
that fails as it does for a gone library (`NotAvailable`, 400 or 404), and when the server is reached again after a failed load. A
failed load keeps the list. A library grid whose library left the list says `NotAvailable` (ADR 0015's 401 rule, amended 2026-10-25).
The UI updates its navigation in place and decides what to show when the library on screen leaves the list; whether a library is gone
comes from the core's list.

### Global search filters (amended 2026-10-27, owner)
The title bar's search gets a Filter and a View on its results page (owner, 2026-10-26 and 2026-10-27). The core holds the model
(`search_filter.rs`): the choices, which is in effect, which are unavailable and why, the queries, and the line under a full group;
the UIs list what it gives.
- **Type: All, Movies, Series, People.** All shows every group the search finds, as before, with people as the last group; Movies,
  Series and People narrow it to that one group, up to 100 (`NARROWED_SEARCH_LIMIT`; the server then weighs 300 candidates). The
  kinds are the open-value media kinds (`movie`, `series`, `person`); a remembered kind the search does not offer reads as All.
- **People** (owner, 2026-10-27, from the owner's "Actors"): people are searched as items, `GET Items?searchTerm=&
  includeItemTypes=Person&recursive=true` with no `parentId`, so they come in the server's relevance order (exact, prefix, word prefix,
  then contains, on the clean name, original title and sort name: `SqlSearchProvider.SearchAsync`) within the same `3 × limit` window
  as every group, and map to ordinary summaries with their picture as the primary image (by-name items are let through the library
  access filter when no parent is given: `SearchQueryAccessFilter.Build`; ids from the providers skip the libraries' `TopParentIds`:
  `LibraryManager.AddUserToQuery`). That search returns every person kind, directors and writers with the actors, as Jellyfin's
  `Items` cannot narrow people by their part (its `personTypes` narrows the items a person is credited on), so the choice and the
  group are called **People**, not Actors. `GET Persons?searchTerm=&personTypes=Actor` gives actors only, but by a plain name match
  in name order (`PeopleRepository.GetPeople`: `OrderBy(e => e.Name.ToLower())`), not by relevance; `GET Search/Hints` needs a
  second answer shape and has no user data. (jellyfin/jellyfin v12.2, ca0f16eb, read 2026-10-27.) A user with content
  restrictions sees only people credited on an item they can reach: under `UserHasContentRestrictions`,
  `ApplyItemByNameAccessFiltering` keeps a Person "only when at least one item behind its name is reachable for the user"
  (`Jellyfin.Server.Implementations/Item/BaseItemRepository.QueryBuilding.cs`), and a user has content restrictions when not every
  folder is enabled, or with blocked folders, a parental rating limit, blocked or allowed tags, or blocked unrated items
  (`User.HasContentRestrictions`, `Jellyfin.Data/UserEntityExtensions.cs`; same tag, read 2026-10-27).
- **Opening a person** (owner, 2026-10-27) shows the films and series they are credited on, in every library, the newest first
  (`person_query(person)`: `ItemQuery.person`, `personIds`), as a grid titled with their name, with nothing remembered. A details
  page's cast and crew cards open the same. A person page of its own is later.
- **Source: All, Library, Catalog.** Catalog is what can be requested, with Seerr (M4); it is available while a Seerr server is
  signed in (`catalog_available`: the capabilities state's `is_available(SEERR)`), so today never, and M4 only has to provide one.
  Until then **the Source filter is shown, with Catalog unavailable**: disabled, its name saying why ("Catalog (needs Seerr)") and
  the tooltip "Needs a Seerr server". A disabled control "can't receive keyboard focus" and "can't handle any input events on itself"
  (Control.IsEnabled, learn.microsoft.com, updated 2026-07-28, read 2026-10-27); that its tooltip then does not show, and how
  Narrator reads it, are not verified, so the reason is in its name. **This departs from "hide what
  is unavailable" (CLAUDE.md), by the owner's decision: the app is also used without Seerr, and the option should be visible.** A
  Catalog choice that cannot apply reads as All. Without Seerr, All and Library find the same.
- **View**: posters or wide, with or without titles, as a library grid's (section 7's remembered view). Episodes and videos stay wide.
  The owner's decision (2026-10-27): people show as round cast cards whatever the view, and a search narrowed to one type wraps its
  tiles into a grid. The core shapes each group (`search_result_display(kind, filter, wide, show_titles) ->
  RowDisplay`: its tiles, whether they are cast cards, show titles and wrap, what opening an item does (a person opens their titles),
  whether it has the item menu (a person has none), and its note), from the filter as it applies.
- **No sort**: results stay in the server's relevance order (Context; section 3). A group that returns as many as it asked for (24,
  or 100 narrowed) shows "Showing the best N matches. Type more to narrow it." (`search_note(filter, count)`).
- **Remembered** under the existing switches (section 7): the search page's filter while "Remember filter" is on and its view while
  "Remember view" is, one entry for the page (`browse.search` in `settings.json`, ADR 0013's open-value rules: `{"filter": {"type":
  "movie" | "all", "source": "library"}, "view": {...}}`), forgotten when its switch goes off; every type and every source, the
  default, is left out.
- Across the bridge: `Browse::search(term, filter)`; `Core::search_tools(filter) -> SearchTools`, `search_choices()`,
  `remember_search_choices(choices)`; `search_note(filter, count)`, `person_query(person)`; `ItemQuery` gains `person`.

### 7. Across the bridge
Records mirror the model types (`ItemSummary`, `ItemDetails`, `Library`, `ImageRef`, `Artwork`, `UserState`, `Slot`, `PagedState`, ...);
item ids and image tags cross, addresses and paths never do.
- `Core::browse(server)` returns a `Browse` object: `libraries()` (a value), `open_library(library, query) -> PagedItems`,
  `home() -> HomeFeed`, `details(item) -> DetailsView` (the details value plus seasons, episodes and similar lists), `search(term)`.
- `PagedItems`: `subscribe_slots(listener)` (a diff list), `subscribe_state(listener)` (a value), `set_visible_range(first, last)`,
  `refresh()`, `retry()`.
- `HomeFeed`: `subscribe_rows` (a diff list of `HomeRow { key, kind, title }`), `subscribe_row(key, listener)` (a diff list of items).
- `set_played(item, bool)`, `set_favourite(item, bool)` on `Browse`; notices through a value on `Core`.
- (Amended 2026-10-27, owner.) **Lists that are not a library**: `Browse::open_items(query) -> PagedItems`, with the queries the core
  builds: `favourites_query(kind)` and `recently_added_everywhere_query(kind)` (every library; `kind` narrows to one media type, and
  an episode stands as its series), `seen_recently_query()`, `recently_added_query(library)` (a library newest first, for a Home
  row's header) and `library_search(query, term)` (section 2's search). `ItemQuery` gains `search_term: Option<String>`.
  `ItemSummary` gains `time_left_ms: Option<u64>` (runtime less the resume point, counted by the core). **Remembered choices**
  (Settings → Libraries, each off by default, kept per library in `settings.json` and forgotten with the server):
  `Core::library_memory()`, `set_remember_sort`, `set_remember_filter`, `set_remember_view`, `library_choices(library)` and
  `remember_library_choices(library, choices)` (`LibraryChoices`: the sort with its order, the filters and the view).
  Searching and filtering: `library_grid_tools(query) -> GridTools`, `library_search_note(query, total)`,
  `library_filter_choices()`, `library_toggle_filter_choices()`, `library_with_filter_choice(query, choice)`,
  `library_with_filter_toggle(query, toggle, on)`, `kind_query(kind)`; the Favourites rows: `Browse::favourites() -> SearchResults`.
- `load_image(request, cancel) -> bytes` (async, `CancelToken`, ADR 0004).
- C#: a `PagedCollection<T>` over `DiffObservableCollection<T>` that implements `IItemsRangeInfo` and forwards the visible range;
  `ImageLoader` as above; one `ListSubscription<T>` per row.

### 8. Tasks
| # | Task | Done when |
|---|---|---|
| 1 | `current-model` browse types and `Artwork.image`; `current-jellyfin` browse queries with the two field sets; the stand-in serves recorded-shape answers for each endpoint | unit tests per endpoint against hand-written 12.2 answers (paging, totals, search window, latest grouping, ticks, missing fields, unknown kinds and collection types pass through); played and favourite answers mapped; clippy, fmt, tests, deny green |
| 2 | Images: `image()` in `current-jellyfin` (URL, `Accept`, buckets); disk and memory cache in `current-app` (per-server folders, temp-and-rename, index, eviction, removal on sign-out, on server removal and at start); BlurHash decoder and BMP writer; the cache folder argument in `App::open` and `open_core` | tests: the gateway header reaches the stand-in's image endpoint; a cached image is served without a request; eviction keeps the newest under the limit; signing out of or removing a server deletes its folder; cancelling stops the fetch; file names hold only hashes; BlurHash output matches known vectors (the format's published examples, not verified yet) |
| 3 | `current-app`: `PagedItems`, `HomeFeed`, details, search, `ItemStates`, notices | tests: visible-range loading and cancellation; refresh sends no `Reset` and keeps loaded slots; a failed page becomes `Failed` and retry fills it; an optimistic change shows in every live list and rolls back on failure; dependent rows reload; rows without items are absent |
| 4 | `current-ffi` and C#: the surface in section 7, `PagedCollection` with `IItemsRangeInfo`, `ImageLoader` | Rust bridge tests and C# tests against the stand-in (paged grid, image bytes, cancellation, a rolled-back toggle); CI's C# job green |
| 5 | Windows UI: Home, library grid (`GridView`), detail pages (movie, series with seasons and episodes), search, played and favourite, images with BlurHash placeholders, offline state, a "Storage" section in Settings with the cache size and Clear | the smoke run passes; the owner browses a real library on Windows and checks: a grid of thousands scrolls without blank tiles staying, a refresh keeps the position, played and favourite toggle everywhere, images load behind a gateway |

Tasks 1 and 2 can run in parallel; 3 needs 1 (and 2 for images); 4 needs 3; 5 needs 4.

## Decided by the owner (2026-10-25)
1. Grid loading: **option A**, a list sized to the total with placeholders, loading pages of 100 around the visible range.
2. Image delivery: **option A**, bytes over UniFFI.
3. Disk cache: **500 MB**, least recently used first, no setting in M2; a server's images are deleted **on sign-out as well as on
   removal**.
4. "Clear cache": **option A**, a "Storage" section in Settings with the cache size and Clear.
5. BlurHash: **option A**, decoded in the core to a small BMP.
6. Home rows: **as proposed** (Continue watching, Next up, Latest per library in the user's order; libraries in the navigation). The
   owner's words: "as proposed for now, later will be remade". Home's layout is provisional and will be redesigned later.
7. Cache file-name hash: **option A**, our own 128-bit FNV-1a.
8. Played and favourite: **option A**, one `ItemStates` store.

The amendments to ADR 0005 and ADR 0013 below follow from these choices and are approved with this ADR.

The options as they were put to the owner follow.

## Open choices (as proposed)

1. **How a long grid loads.** A: a list sized to the server's total with placeholders, loading pages around what is visible (random
   access; the scroll bar shows the whole library from the start, the user can jump anywhere). B: append pages as the user nears the
   end (`ISupportIncrementalLoading`; simpler, but the scroll bar grows and jumping to the end loads everything before it).
   **Recommendation: A**, with pages of 100. Consequence: the C# side implements `IItemsRangeInfo` and placeholder tiles.

2. **How image bytes reach the UI.** A: bytes over UniFFI. B: a path to the cached file, which the UI opens. C: a loopback HTTP URL.
   B needs the core to keep a file alive while the UI reads it, puts paths that contain the account name across the bridge, and its
   `UriSource` form is not verified for unpackaged apps; C needs an HTTP server and a token in the core. **Recommendation: A.**
   Consequence: one copy of each encoded image (tens of kilobytes) into the UI per display.

3. **The disk cache's size and lifetime.** Proposed: 500 MB, least recently used first; deleted with the server. Also: should signing
   out of a server delete its images (the server stays saved, but the user asked to be forgotten)? **Recommendation:** 500 MB, no
   setting in M2; delete on sign-out as well as on removal. Consequence: after signing in again, images download again.

4. **Where "Clear cache" goes.** A: a "Storage" section in Settings showing the cache size, with "Clear" (a pattern like Windows'
   own storage settings; how Windows Settings words it was not verified). B: under Diagnostics, next to "copy diagnostics". C: none in
   M2. **Recommendation: A**, small, in the Windows UI task. Consequence: one more Settings section.

5. **BlurHash placeholders.** A: the core decodes to a small BMP (our own code). B: each UI decodes (a library per platform, or our own
   code three times). C: no blur, a neutral tile until the image arrives. **Recommendation: A.** Consequence: about a hundred lines
   and tests in the core, once for every platform.

6. **Home rows.** Proposed: Continue watching, Next up, then Latest per library in the user's order; libraries in the app's navigation,
   not as a tile row on Home; Continue watching and Next up kept as two rows (as the server's own web client does). The alternative is
   one merged "Up next" row or a library tile row first. **Recommendation:** as proposed. Consequence: a user with many libraries gets
   many Latest rows; a later setting can hide some.

7. **The cache file-name hash.** A: our own 128-bit FNV-1a (a dozen lines, no dependency; not cryptographic, which the cache does not
   need: the inputs are ids and tags, not secrets, and a 128-bit collision among a cache's files is negligible). B: SHA-256 from `ring`, already in the build through `rustls` but not a declared
   dependency (declaring it needs the owner's approval). **Recommendation: A.** Consequence: none outside the cache.

8. **How other lists see a played or favourite change.** A: one `ItemStates` store every live list follows (section 5). B: reload
   every visible list after each change. **Recommendation: A.** Consequence: each list keeps an index by `ItemRef`.

Decided with the manager before writing (2026-10-25): the details record carries only `MediaSummary`; ADR 0019 owns media sources and
streams.

## Amendments to merged ADRs (approved with this ADR)
The implementation tasks write these into the ADRs they amend (task 1: ADR 0005; task 2: ADR 0013); this PR does not edit them.
- **ADR 0005:** `Artwork.url` becomes `Artwork.image: ImageRef`; the model gains `UserState`, `ItemSummary`, `ItemDetails`, `Library`,
  `CalendarDate`, `MediaSummary`, `StreamSummary`, `Person`, `Chapter` and the open values `CollectionKind`, `SortKey`, `ItemFilter`,
  `PersonKind`.
- **ADR 0013:** `App::open` and `open_core` take a cache folder from the UI as well as the data folder; the table gains a cache column.

## Consequences
- Grids of any size open at the speed of one page and keep their place through refreshes.
- Images follow the server's address and gateway like every other request, and a cache of one server never outlives it.
- The core grows an image cache and a BlurHash decoder, and holds image bytes in memory; the UIs only decode and draw.
- Search shows the best matches per group, not an endless list; that is what the server offers. Narrowed to one type, it shows more
  of that type, still the best matches only.
- ADR 0019 and this ADR share `MediaSummary`; whichever merges second adapts to the other.
