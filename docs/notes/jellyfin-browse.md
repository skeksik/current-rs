# Jellyfin browsing, lists and images (spike)

Research behind ADR 0018: which Jellyfin endpoints and fields the browse screens need, how a long grid loads through the core, how
artwork reaches a WinUI image when the core must fetch it, and what the Windows controls expect. All sources were read on 2026-10-25
unless a date says otherwise. Nothing was run against a server; everything below is from source code and documentation. Anything not
checked against a source is marked "not verified".

Source trees read (shallow clones at the tags ADR 0017's notes pinned):

| Project | Tag or commit |
|---|---|
| Jellyfin server | `v12.2` (commit ca0f16eb, 2026-10-05) |
| jellyfin-web | `v12.2` (commit 61b1f890), read only as evidence of what the server accepts and how its own client asks |

Microsoft Learn pages are listed in "Sources" at the end with the date each page gives.

## 1. Jellyfin endpoints for browsing

Paths are relative to the server's base URL. Every one below takes the signed-in user from the token (`RequestHelpers.GetUserId(User,
userId)`), so `userId` can be left out. The older `Users/{userId}/...` routes still exist next to each one; we use the new ones.

### Libraries
`GET UserViews` (`Jellyfin.Api/Controllers/UserViewsController.cs`): returns `QueryResult<BaseItemDto>` of the user's views. It always
adds two fields:

```csharp
var dtoOptions = new DtoOptions();
dtoOptions.Fields = [.. dtoOptions.Fields, ItemFields.PrimaryImageAspectRatio, ItemFields.DisplayPreferencesId];
```

`new DtoOptions()` means all fields (see "Keeping answers small"), so the view list is never trimmed; it is short, so that does not
matter. `UserViewManager.GetUserViews` drops the user's `MyMediaExcludes` and orders by the user's `OrderedViews` preference
(`Emby.Server.Implementations/Library/UserViewManager.cs`, lines 137 and 141), so the server's order is the user's chosen order.

A view's `CollectionType` (`Jellyfin.Data/Enums/CollectionType.cs`) is one of `unknown, movies, tvshows, music, musicvideos, trailers,
homevideos, boxsets, books, photos, ...` (lower case on the wire; the enum continues past what was read). We map the ones we give
meaning to and pass the rest through.

### Items: the grid query
`GET Items` (`ItemsController.GetItems`, line 171) takes, among about a hundred parameters:
- `parentId`, `includeItemTypes`, `excludeItemTypes`, `recursive`, `startIndex`, `limit`, `sortBy`, `sortOrder`, `filters`,
  `isPlayed`, `isFavorite`, `genres`, `years`, `nameStartsWith`, `searchTerm`, `ids`;
- `fields`, `enableImageTypes`, `imageTypeLimit`, `enableUserData`, `enableImages`, `enableTotalRecordCount` (default `true`).

From the XML doc comments on the same method:
- `sortBy`: "Options: Album, AlbumArtist, Artist, Budget, CommunityRating, CriticRating, DateCreated, DatePlayed, PlayCount,
  PremiereDate, ProductionYear, SortName, Random, Revenue, Runtime." The enum `ItemSortBy` has more (`Name, SeriesSortName,
  DateLastContentAdded, IndexNumber, ParentIndexNumber, IsFavoriteOrLiked, ...`).
- `filters`: "Options: IsFolder, IsNotFolder, IsUnplayed, IsPlayed, IsFavorite, IsResumable, Likes, Dislikes." (`ItemFilter` also
  has `IsFavoriteOrLikes`.)
- `startIndex`: "The record index to start at. All items with a lower index will be dropped from the results."

Behaviour worth knowing:
- On a library folder with `includeItemTypes`, the server makes the query recursive unless the client says otherwise:
  ```csharp
  // includeItemTypes on a library lists its contents recursively rather than just its
  // immediate children, so default to a recursive query when the client didn't choose.
  if (folder is ICollectionFolder && includeItemTypes.Length > 0)
  {
      recursive ??= true;
  }
  ```
- The answer is `QueryResult<BaseItemDto>(startIndex, result.TotalRecordCount, items)`: `TotalRecordCount` is the size of the whole
  query, which a placeholder-sized grid needs.
- `RequestHelpers.GetOrderBy` passes the sort keys through as given and adds no tie-breaker. Whether the database orders ties stably
  across pages was not verified; we always end the sort with `SortName` so two pages agree (see ADR 0018).

### Search: `Items?searchTerm=` against `Search/Hints`
Both go through the search providers in 12.2 (`Emby.Server.Implementations/Library/Search/SearchManager.cs`). In `GetItems`:

```csharp
var searchProviderQuery = new SearchProviderQuery
{
    SearchTerm = searchTerm,
    ...
    Limit = limit.HasValue ? limit.Value * 3 : null,
    ParentId = parentId
};
```

then the providers' ids replace the query's ids, the items are loaded with the user's access rules (`folder.GetItems(query)`), ordered
by score then `SortName`, and `startIndex`/`limit` are applied to that list:

```csharp
var totalCount = orderedItems.Length;
if (startIndex.HasValue && startIndex.Value > 0)
{
    orderedItems = orderedItems.Skip(startIndex.Value).ToArray();
}
```

So a search through `Items` returns at most `3 × limit` candidates (100 when no limit is given: `var requestedLimit =
providerQuery.Limit ?? 100;` in `SearchManager`), and `TotalRecordCount` counts only those. **Search results are not pageable** past
that window: we ask for one page large enough for the screen. When the providers find nothing, the query falls back to the plain
`SearchTerm` filter (`SearchTerm = searchResultScores is null ? searchTerm : null`).

`GET Search/Hints` (`SearchController.GetSearchHints`) returns `SearchHint` records (name, type, a primary image tag, a few fields),
not `BaseItemDto`, and includes people, genres, studios and artists by default. It needs a second shape and gives no user data.
ADR 0018 uses `Items?searchTerm=` so search results are ordinary summaries.

### Latest per library
`GET Items/Latest` (`UserLibraryController.GetLatestMedia`, line 527): `parentId`, `includeItemTypes`, `isPlayed`, `fields`,
`enableImages`, `imageTypeLimit`, `enableImageTypes`, `enableUserData`, `limit` (default 20), `groupItems` (default true). It returns a
bare array, not a `QueryResult`. Two server rules:
- "If `isPlayed` is not given and the user has `HidePlayedInLatest`, `isPlayed = false`" (`if (user.HidePlayedInLatest) { isPlayed =
  false; }`).
- With `groupItems`, several new episodes of one series come back as the series (or season) with `ChildCount` set to the number
  grouped (`dtos[i].ChildCount = childCounts[i];`).

The user's "exclude from latest" libraries are applied when no `parentId` is given (`PreferenceKind.LatestItemExcludes`,
`UserViewManager.cs` line 306). With a `parentId` per library, the client must skip excluded libraries itself; jellyfin-web does
(`recentlyAdded.ts`: `const userExcludeItems = user.Configuration?.LatestItemsExcludes ?? [];` and skips `['playlists', 'livetv',
'boxsets', 'channels', 'folders']`). `LatestItemsExcludes` is in the user's configuration (`UserDto.Configuration`), which the sign-in
already reads (ADR 0015) but does not keep yet.

### Continue watching (resume)
`GET UserItems/Resume` (`ItemsController.GetResumeItems`, line 921): `startIndex`, `limit`, `parentId`, `fields`, `mediaTypes`,
`enableUserData`, `imageTypeLimit`, `enableImageTypes`, `includeItemTypes`, `excludeItemTypes`, `enableTotalRecordCount`,
`excludeActiveSessions`. It orders by `DatePlayed` descending and filters `IsResumable = true`; with no `parentId` it leaves out the
user's `LatestItemExcludes` libraries. jellyfin-web asks per media type (`mediaTypes: [ mediaType ]`, `resume.ts`), with
`enableTotalRecordCount: false`.

### Next up
`GET Shows/NextUp` (`TvShowsController.GetNextUp`, line 75): `startIndex`, `limit`, `fields`, `seriesId`, `parentId`, image and user data
options, `nextUpDateCutoff`, `enableTotalRecordCount`, `enableResumable` (default true), `enableRewatching` (default false). jellyfin-web
sends `enableResumable: false` (so an episode in progress shows in "Continue watching", not twice), `nextUpDateCutoff` from the user's
"max days for next up" setting, and `enableTotalRecordCount: false` (`nextUp.ts`).

### Series, seasons, episodes
- `GET Shows/{seriesId}/Seasons` (line 332): `fields`, `isSpecialSeason`, `isMissing`, `adjacentTo`, image and user data options.
- `GET Shows/{seriesId}/Episodes` (line 202): `season`, `seasonId`, `isMissing`, `adjacentTo`, `startItemId`, `startIndex`, `limit`,
  `sortBy`, `fields`, image and user data options. Missing episodes are included when the user's `DisplayMissingEpisodes` is set.
- An episode's summary carries `SeriesId`, `SeriesName`, `SeasonId`, `SeasonName`, `IndexNumber`, `ParentIndexNumber`,
  `IndexNumberEnd` (`BaseItemDto`).

### Details and similar
- `GET Items/{itemId}` (`UserLibraryController.GetItem`, line 82) takes no `fields`: it builds `new DtoOptions()`, which is **all
  fields** except `SeasonUserData` and `RefreshState` (`DtoOptions.cs`: `Fields = allFields ? AllItemFields : [];` with
  `DefaultExcludedFields`). That includes `MediaSources`, `MediaStreams`, `Chapters`, `People`, `Trickplay`. It also refreshes the item
  first when needed (`RefreshOnDemandIfNeeded`).
- `GET Items/{itemId}/Similar` (`LibraryController`, line 801): `limit`, `fields`, `excludeArtistIds`. No image options: the default
  `DtoOptions` for that call was not read (not verified whether it trims).

### Played and favourite
- `POST UserPlayedItems/{itemId}` (optional `datePlayed`) and `DELETE UserPlayedItems/{itemId}` (`PlaystateController`, lines 72 and
  139) answer `UserItemDataDto`.
- `POST UserFavoriteItems/{itemId}` and `DELETE UserFavoriteItems/{itemId}` (`UserLibraryController`, lines 220 and 268) answer
  `UserItemDataDto`.
- `POST UserItems/{itemId}/UserData` with an `UpdateUserItemDataDto` body (`ItemsController`, line 1128) sets several fields at once.
  We use the dedicated endpoints.
- ~~Whether marking a series or season played marks its episodes was not verified.~~ Checked 2026-10-27 in v12.2 (ca0f16eb), the same
  on master (02c2951e), for fix/series-episode-marks:
  - `PlaystateController.UpdatePlayedStatus` calls `item.MarkPlayed(user, datePlayed, true)` or `item.MarkUnplayed(user)`, then
    answers `_userDataRepository.GetUserDataDto(item, user)`.
  - `Folder.MarkPlayed` and `MarkUnplayed` mark every item below that is not a folder ("Sweep through recursively and update
    status", `Recursive = true, IsFolder = false`), skipping virtual items when marking played unless the user displays missing
    episodes. The folder's own row is left as it was.
  - `BaseItem.MarkPlayed` sets `PlayCount` to at least 1, `PlaybackPositionTicks = 0` and `LastPlayedDate = datePlayed ??
    data.LastPlayedDate ?? DateTime.UtcNow`. `ResetPlayedState` clears play count, position and `LastPlayedDate`.
  - A folder's user data is derived (`Folder.FillUserDataDtoValues`: `UnplayedItemCount = total - played`, `PlayedPercentage =
    played / total * 100`, `Played = playedCount >= totalCount`, and `Played = true` with nothing to count). It counts items that are
    neither folders nor virtual (`DescendantQueryHelper.IsCountableLeaf`: `!b.IsFolder && !b.IsVirtualItem`). So the mark's answer
    for a series already carries its new count.
  - `GET UserItems/{itemId}/UserData` (`ItemsController.GetItemUserData`, line 1072) answers the same `GetUserDataDto`: one item's
    marks without its record.
- Next up lists a series only once one of its episodes has a `LastPlayedDate` on or after the cutoff
  (`NextUpService.GetNextUpSeriesKeys`). A stop report marks the item played when past `MaxResumePct` (90 by default) or within a
  second of the end, and sets `LastPlayedDate` when it played to the end or kept a position (`UserDataManager.UpdatePlayState`,
  `SessionManager.OnPlaybackStopped`). `ReportPlaybackStopped` awaits this before it answers. With `seriesId`, Next up answers for
  that series whatever was played; what it answers for a series never started is not verified.

### The item record
`BaseItemDto` (`MediaBrowser.Model/Dto/BaseItemDto.cs`) fields the browse screens use:
- identity and kind: `Id`, `Type` (`BaseItemKind`), `MediaType`, `CollectionType` (views), `IsFolder`;
- text: `Name`, `OriginalTitle`, `SortName`, `Overview`, `Taglines`, `Genres`, `Studios` (`NameGuidPair[]`), `People`
  (`BaseItemPerson[]`: `Name`, `Id`, `Role`, `Type`, `PrimaryImageTag`, `ImageBlurHashes`), `OfficialRating`, `CommunityRating`,
  `CriticRating`, `ProductionYear`, `PremiereDate`, `EndDate`, `Status`;
- time: `RunTimeTicks` (100 ns ticks), `CumulativeRunTimeTicks`;
- links: `SeriesId`, `SeriesName`, `SeasonId`, `SeasonName`, `IndexNumber`, `IndexNumberEnd`, `ParentIndexNumber`, `ParentId`,
  `ChildCount`, `RecursiveItemCount`;
- images: `ImageTags` (`Dictionary<ImageType, string>`), `BackdropImageTags` (`string[]`), `ImageBlurHashes`
  (`Dictionary<ImageType, Dictionary<string, string>>`, keyed by tag), `PrimaryImageAspectRatio` (double), and the parent fallbacks
  `SeriesPrimaryImageTag`, `ParentThumbItemId`/`ParentThumbImageTag`, `ParentBackdropItemId`/`ParentBackdropImageTags`,
  `ParentLogoItemId`/`ParentLogoImageTag`, `ParentPrimaryImageItemId`/`ParentPrimaryImageTag`;
- media (details only): `MediaSources` (`MediaSourceInfo[]`), `MediaStreams`, `MediaSourceCount`, `Container`, `Chapters`
  (`ChapterInfo`: `StartPositionTicks`, `Name`, `ImageTag`), `Trickplay`;
- `UserData` (`UserItemDataDto`): `PlayedPercentage`, `UnplayedItemCount`, `PlaybackPositionTicks`, `PlayCount`, `IsFavorite`,
  `Played`, `LastPlayedDate`, `Likes`, `Rating`, `Key`, `ItemId`.

`ItemFields` (`MediaBrowser.Model/Querying/ItemFields.cs`), the opt-in fields: `AirTime, CanDelete, CanDownload, ChannelInfo, Chapters,
Trickplay, ChildCount, CumulativeRunTimeTicks, CustomRating, DateCreated, DateLastMediaAdded, DisplayPreferencesId, Etag, ExternalUrls,
Genres, ItemCounts, MediaSourceCount, MediaSources, OriginalTitle, Overview, ParentId, Path, People, PlayAccess, ProductionLocations,
ProviderIds, PrimaryImageAspectRatio, RecursiveItemCount, Settings, SeriesStudio, SortName, SpecialEpisodeNumbers, Studios, Taglines,
Tags, RemoteTrailers, MediaStreams, SeasonUserData, DateLastRefreshed, DateLastSaved, RefreshState, ChannelImage,
EnableMediaSourceDisplay, Width, Height, ExtraIds, LocalTrailerCount, IsHD, SpecialFeatureCount`. Which `BaseItemDto` fields are
always present without an `ItemFields` value (for example `Name`, `ProductionYear`, `RunTimeTicks`, `UserData`, `ImageTags`) was read
from how jellyfin-web asks (it asks only for `PrimaryImageAspectRatio` and a few others on grids) and not from `DtoService` itself:
not verified field by field. The current-jellyfin task records a real answer from the stand-in schema and checks it.

**`SeriesPrimaryImageTag` needs no `ItemFields` value** (checked by ADR 0018 task 1, 2026-10-25, in the Jellyfin server at `v12.2`,
commit ca0f16eb, `Emby.Server.Implementations/Dto/DtoService.cs`). For an episode (lines 1517-1548) and for a season (lines 1603-1619)
the block that sets it has its condition commented out: `// if (options.ContainsField(ItemFields.SeriesPrimaryImage))`, followed by
`dto.SeriesPrimaryImageTag = GetTagAndFillBlurhash(dto, episodeSeries, ImageType.Primary);`. So every list answer carries it, and
its BlurHash goes into the item's `ImageBlurHashes` under `Primary`. When the item has no primary image of its own, the same block calls
`AttachPrimaryImageAspectRatio(dto, episodeSeries)`, so `PrimaryImageAspectRatio` is then the series poster's (but only where
`fields` includes `PrimaryImageAspectRatio`, lines 376-380, which the summary set does). With a primary image limit above 0, episodes
also get `ParentPrimaryImageItemId`/`ParentPrimaryImageTag`, set to the season's poster or else the series' (lines 1550-1567).
current-jellyfin reads `SeriesPrimaryImageTag` with `SeriesId`, as ADR 0018 says.

### Keeping answers small
`AddAdditionalDtoOptions` (`Jellyfin.Api/Extensions/DtoExtensions.cs`) sets the image and user data options on every list query:

```csharp
dtoOptions.EnableImages = enableImages ?? true;
if (imageTypeLimit.HasValue) { dtoOptions.ImageTypeLimit = imageTypeLimit.Value; }
if (enableUserData.HasValue) { dtoOptions.EnableUserData = enableUserData.Value; }
if (enableImageTypes.Count != 0) { dtoOptions.ImageTypes = enableImageTypes; }
```

and `new DtoOptions { Fields = fields }` starts from the given fields only. So a list query with `fields=PrimaryImageAspectRatio`,
`enableImageTypes=Primary,Backdrop,Thumb`, `imageTypeLimit=1` returns the base record, the aspect ratio and at most one tag per image
type. This is what jellyfin-web sends for its home rows (`recentlyAdded.ts`, `nextUp.ts`, `resume.ts`). User data stays on (default
`true` in the `DtoOptions` constructor).

### What jellyfin-web asks (evidence only)
- Library grid page size: `userSettings.libraryPageSize()` "|| 100" (`scripts/settings/userSettings.js`, line 527), with a pager, not
  infinite scroll. Grid fields: `MediaSourceCount`, and `PrimaryImageAspectRatio` when the grid shows primary images
  (`utils/items.ts`, `getItemFieldsEnum`).
- Home rows: latest 16 items per library when the row scrolls horizontally (`enableOverflow`), next up 24, resume 12.
- Default home sections, in order (`constants/homeSectionType.ts`, `DEFAULT_SECTIONS`): `SmallLibraryTiles, Resume, ResumeAudio,
  ResumeBook, LiveTv, NextUp, LatestMedia`. Each section stays hidden until its items load (`frag.classList.add('hide')`).
- Image URLs (`components/cardbuilder/utils/url.ts`):
  ```ts
  fillHeight: height ? Math.ceil(height * dpr) : undefined,
  fillWidth: width ? Math.ceil(width * dpr) : undefined,
  quality: 96,
  tag: imgTag
  ```

## 2. Jellyfin images

`GET Items/{itemId}/Images/{imageType}` (`ImageController.GetItemImage`, line 552) takes `maxWidth`, `maxHeight`, `width`, `height`,
`quality` ("from 0-100. Defaults to 90 and should suffice in most cases"), `fillWidth`, `fillHeight` ("Width of box to fill"), `tag`
("Supply the cache tag from the item object to receive strong caching headers"), `format`, `percentPlayed`, `unplayedCount`, `blur`,
`backgroundColor`, `foregroundLayer`, `imageIndex`. A backdrop other than the first is `imageIndex`.

**Size.** `ImageHelper.GetNewImageSize` applies `Resize` (width/height/max), then `ResizeFill`, then "Never encode larger than the
source" (`ScaleDownToFit`). `ResizeFill` scales by `Math.Min(widthRatio, heightRatio)` so the result covers the box with the source's
aspect ratio and returns the source unchanged when it is already smaller ("Clamp to current size"). So `fillWidth`+`fillHeight` gives
an image at least as large as the slot in both directions, never upscaled and never cropped.

**Format.** With no `format`, the server reads `Accept` (`GetClientSupportedFormats`): WebP only if the client accepts it, then JPEG
and PNG, GIF if accepted. `ImageProcessor.GetOutputFormat` picks WebP if both sides support it, "If transparency is needed and webp
isn't supported, than png is the only option", else the first client format the server supports. A client that sends `Accept:
image/jpeg, image/png` therefore gets JPEG, or PNG for images with transparency (logos). WinUI's `BitmapImage` does not list WebP (see
section 4), so we do not accept it.

**Caching headers.** With a `tag`, the answer carries `Cache-Control: public, max-age=31536000, immutable` ("When tag is provided, the
URL is effectively immutable - the tag changes when the image changes") and `ETag: "<tag>"`, and answers `304` to a matching
`If-None-Match`. So **the tag is a content version**: a cache entry keyed by (item, image type, index, tag, size) never needs
revalidation, and a new tag means a new entry.

**BlurHash.** `ImageBlurHashes[type][tag]` gives the BlurHash for each image tag. The BlurHash format is defined by its reference
project (woltapp/blurhash, MIT): base-83 characters encoding the number of components, a maximum AC value, a DC colour and the AC
components; decoding is a sum of cosines over the components. Reading the spec and writing a decoder needs no dependency; the
algorithm's details were not re-read for this spike (not verified line by line).

## 3. Windows: virtualised grids

From "ListView and GridView data virtualization" (Microsoft Learn, ms.date 2026-03-16):
- Incremental: "loads data sequentially. ... As items are loaded, the scroll bar's thumb decreases in size." Needs `IList`,
  `INotifyCollectionChanged` and `ISupportIncrementalLoading`; "The count should represent the number of items locally, not the true
  size of the dataset."
- Random access: "allows loading from an arbitrary point in the data set. ... At all times, the scroll bar's thumb indicates that the
  ListView contains a million items." Needs `IList`, `INotifyCollectionChanged`, optionally `IItemsRangeInfo` and `ISelectionInfo`.
- `IItemsRangeInfo` "provides information about which items the control is actively using. The items control calls this method
  whenever its view is changing", with the items in the viewport and "A buffer of items around the viewport ... The focused item. The
  first item." It "is designed to be used by only a single instance of an items control".
- "If you don't have it, then return either `null` or a placeholder item." and "Find the sweet spot in the size of the batches you
  fetch items in. Prefer chunky to chatty."
- "a feature is enabled by default for ListView and GridView that displays temporary placeholder visuals while the user is panning or
  scrolling quickly" (`ShowsScrollingPlaceholders`).

From "User interface migration (including WinUI)" (Microsoft Learn, ms.date 2026-07-13), "Virtualized list null-placeholder
behavior": "UWP's `ListView` and `GridView` tolerate `null` items ... WinUI 3 does not — null items cause an `E_POINTER` (0x80004003)
crash at runtime." and "replace those values with a non-null sentinel object". The same page: `DisplayInformation.GetForCurrentView()`
throws in WinUI 3 desktop apps; "Use XamlRoot.RasterizationScale instead, and subscribe to the XamlRoot.Changed event to respond to DPI
changes."

From "ItemsRepeater" (Microsoft Learn, ms.date 2026-09-19):
- `ISupportIncrementalLoading`, `IItemsRangeInfo` and `ISelectionInfo` "enable special functionality in the ListView and GridView
  controls, but currently have no effect on an ItemsRepeater".
- "Typically, a reset causes a control to release existing child elements and start over, building the UI from the beginning at
  scroll position 0 as it has no awareness of exactly how the data has changed during the reset." Only `IKeyIndexMapping` (an
  ItemsRepeater interface) avoids that.
- ItemsRepeater "has no default UI and provides no policy around focus, selection, or user interaction" and "does not provide a
  default accessibility experience".

What this means for us:
- `GridView` gives the platform's own keyboard, selection, focus and accessibility behaviour (CLAUDE.md: native behaviour by default)
  and supports `IItemsRangeInfo`; `ItemsRepeater` needs all of that rebuilt. ADR 0018 uses `GridView` for library grids and a
  horizontal `ListView` or `GridView` for home rows (the exact row control is the Windows UI task's call).
- A `Reset` on the bound collection loses the scroll position (stated for ItemsRepeater; for GridView not verified, assumed the same).
  So a refresh must never send `Reset`, and `DiffObservableCollection`'s Reset (`ListDiff.cs`: "One Reset notification, not one per
  item.") must stay for the first batch and for recovery only.
- Placeholders must be non-null records.

## 4. Windows: showing image bytes

From the `BitmapImage` class page (Windows App SDK API, updated 2026-07-28):
- "You can define a BitmapImage by using a Uniform Resource Identifier (URI) that references an image source file, or by calling
  SetSourceAsync and supplying a stream."
- Formats: "JPEG, PNG, bitmap (BMP), GIF, TIFF, JPEG XR, icons (ICO)". "If the image source is a stream, that stream is expected to
  contain an image file in one of these formats." WebP is not listed.
- "ImageFailed and ImageOpened are mutually exclusive. One event or the other will always be raised whenever a BitmapImage object has
  its source value set or reset."

From `BitmapSource.SetSourceAsync` (updated 2026-07-28):
- Example: `using (IRandomAccessStream fileStream = await file.OpenAsync(...)) { ... bitmapImage.DecodePixelWidth = decodePixelWidth;
  await bitmapImage.SetSourceAsync(fileStream); ... }` with the comment "Ensure the stream is disposed once the image is loaded".
- "If you have a Microsoft .NET stream that you want to use as a source, you can use the AsRandomAccessStream extension method".
- "If the app changes the image source again via SetSourceAsync, SetSource or UriSource while a SetSourceAsync call is already in
  progress, the pending SetSourceAsync action will throw a TaskCanceledException".

From `BitmapImage.UriSource` (updated 2026-07-28): nothing about which URI schemes an unpackaged app may use. A third-party framework's
documentation states that `ms-appdata:///` "is not supported on Image" in unpackaged Windows App SDK apps and recommends
`SetSourceAsync`; Microsoft's own pages say nothing either way, and whether a `file:///` URI works in `UriSource` for an unpackaged app
was **not verified**. The example above disposes the stream after `SetSourceAsync` completes, which implies the decoded bitmap does not
need the file afterwards; that is an inference, not a statement on the page (not verified).

`DecodePixelWidth` (updated 2026-07-28): "The width (in pixels) to use for image decoding operations." The class example sets it to the
display width so the image decodes at that size.

### Three ways to hand the image to the UI

| | Bytes over UniFFI | Cached file path | Loopback HTTP URL |
|---|---|---|---|
| UI code | `byte[]` → `MemoryStream.AsRandomAccessStream()` → `SetSourceAsync` | open the file → `SetSourceAsync` (or `UriSource`, not verified) | `UriSource = http://127.0.0.1:port/...` |
| Copies | one copy of the encoded image into the UI's memory | the UI reads the file | over a socket |
| Eviction while shown | none: the UI holds its own bytes | the core may delete a file the UI is opening; on Windows a delete of an open file fails unless it was opened with delete sharing (not verified for WinRT `OpenAsync`) | none |
| Memory cache in the core | used directly | bypassed (the UI reads disk) | used |
| What crosses the bridge | bytes, never a path | a path in the data folder, which holds the user's account name (ADR 0013 keeps paths out of errors and logs) | a port and a per-run token |
| Cost | nothing new | the core must keep a file alive while the UI reads it | an HTTP server in the core (a new dependency or our own), a port, a token against other local processes |
| Other platforms | SwiftUI and GTK take bytes as easily (not verified per API) | same | AVKit and GTK would need it too |

A poster at the sizes in ADR 0018 is a JPEG of tens of kilobytes (not measured); a screen of 30 posters is around a megabyte copied
once. ADR 0018 recommends bytes.

## 5. Current's own code

- `ObservableList` (`crates/app/src/list.rs`): one lock for change and broadcast, `Reset` first and after lag
  (`DEFAULT_BACKLOG = 256` diffs). A page of 100 slots filled one by one is 100 `Set` diffs in one batch; two quick pages already use
  most of the default backlog, so a paged list needs `with_backlog` sized to several pages, or a lagging UI gets a `Reset` (which
  loses the scroll position, section 3).
- `list_diff!` (`crates/ffi/src/diff.rs`) and `watch_list` (`crates/ffi/src/subscribe.rs`) carry each batch to a listener on a core
  worker thread; a failed delivery requests a `Reset`.
- `DiffObservableCollection<T>` (`bindings/csharp/Current.Bindings/ListDiff.cs`) is an `ObservableCollection<T>` and so already
  implements `IList` and `INotifyCollectionChanged`; a subclass can add `IItemsRangeInfo`.
- `Artwork` (`crates/model/src/artwork.rs`) holds a `url` string "as built by the service crate". A URL holds the server's address,
  which changes with roaming (ADR 0014) and must not cross the bridge (docs/architecture.md: "Nothing secret crosses in a state: no
  address"). ADR 0018 replaces it with an image reference.
- `ServerKey` is an opaque string the app assigns (`crates/model/src/identity.rs`); `App::remove_server` exists
  (`crates/app/src/lib.rs`, line 317) and is where a server's images are deleted.
- The workspace has no hashing crate. `ring` is already in the dependency tree through `rustls` (`Cargo.toml`: `rustls = { ...
  features = ["ring", ...] }`) but is not a declared workspace dependency.

## 6. Left to ADR 0019 (playback)

ADR 0018 puts in the details record only what the detail page shows before anyone presses play (`MediaSummary`, see the ADR). These
belong to ADR 0019 and spike/playback-design, and ADR 0018 does not define them:
- `MediaSource` and the stream types (video, audio, subtitle) with codecs, profiles, bit depth, HDR, channels, bitrates, delivery
  methods, external subtitle URLs, `SupportsDirectPlay`/`DirectStream`/`Transcoding`, `PlaybackInfo`, device profiles, the route plan;
- the version picker (several `MediaSources` per item) beyond the count;
- resume position semantics while playing, progress reporting (`Sessions/Playing*`), and the played state a finished playback sets;
- chapters' images and trickplay thumbnails during playback (ADR 0018 lists chapter names and start times only);
- intro and credits segments (`MediaSegments`).

ADR 0018's `MediaSummary` names its fields so ADR 0019's stream types can extend them (a `StreamSummary` for one stream per kind that
ADR 0019 may replace with its full type).

## 7. Upstream bugs and our workarounds

### An episode's own read keeps a mark its series' sweep undid
- **Steps** (the owner's run, 2026-10-27, on a one-season series): on the series' page, mark an episode played from its tile; mark
  the series played; unmark the series; refresh. The episode's tile (a list read) shows unplayed; its details page (`GET
  Items/{id}`) still shows played, after a refresh too, until the server drops the item from its cache.
- **Why**: `GET Items/{id}` and `GET UserItems/{id}/UserData` take the item from `LibraryManager.GetItemById` ("`if
  (_cache.TryGet(id, out var item))`", an LRU of `BaseItem`s) and read its user data from that instance
  (`UserDataManager.GetUserData`: "`ResolveUserDataRow(item, item.UserData?.Where(e => e.UserId.Equals(user.Id)))`"). `SaveUserData`
  reloads only the instance it saved through ("`// rehydrate the cached userdata`"); `Folder.MarkPlayed` and `MarkUnplayed` save the
  instances `GetItemList` loads, so the cached episode keeps what its own mark saved. Lists (`DtoService.GetBaseItemDtos`) read user
  data through `GetUserDataBatch`, which reads the per-user cache every save updates, so they are right.
- **Versions**: seen on 12.3 (the owner's server, 2026-10-27; source at tag 06de749, read 2026-10-27). 12.2 (ca0f16eb) has the same lines, from
  the source; not seen on a 12.2 server.
- **Workaround**: a detail page takes the item's marks from `GET Items?ids={id}`, sent with `GET Items/{id}`
  (`JellyfinClient::details`, `crates/jellyfin/src/browse.rs`); the stand-in models the bug (`tools/jellyfin-stand-in/src/library.rs`)
  and `crates/app/tests/series_marks.rs` tests it. Our series' and season's read-backs (`UserItems/{id}/UserData`) are not affected:
  their marks are derived by count queries (`Folder.FillUserDataDtoValues`).
- **Removal**: the workaround and the stand-in's model go once a single-item read serves fresh user data after a folder's sweep;
  re-check on each new server baseline. Reporting it upstream is the owner's call.

### A write through an item's cached instance puts back a mark its series' sweep changed
- **Steps** (from the source; not seen on a server): mark an episode played on its own (the server caches its instance with it
  played); mark its series played and then unplayed; favourite the episode, or start playing it. The episode is played again, in
  the database, for every client.
- **Why**: every write reads the user data it changes from the item's cached instance and saves the whole row back.
  `UserLibraryController.MarkFavorite`: "`var data = _userDataRepository.GetUserData(user, item); data.IsFavorite = isFavorite;`";
  `SessionManager.OnPlaybackStart(User, BaseItem)`: "`var data = _userDataManager.GetUserData(user, item); data.PlayCount++;`";
  `OnPlaybackProgress` and `OnPlaybackStopped` through `UpdatePlayState`, which sets `Played` only near the end.
  `UserDataManager.GetUserData` reads the instance ("`ResolveUserDataRow(item, item.UserData?.Where(e => e.UserId.Equals(user.Id)))`"),
  and `SaveUserData` saves every field ("`dbContext.UserData.Attach(userDataEntry).State = EntityState.Modified`"), then puts that row
  in the per-user cache and rehydrates the instance, so lists, single reads and `UserDataChanged` all agree on the reverted state. The
  instance is the one the previous entry describes: `Folder.MarkPlayed` and `MarkUnplayed` save other instances. A collection's
  sweep is not affected: `BoxSet.MarkPlayed` marks each `GetLinkedChildren(user)`, taken from `GetItemById` (`BaseItem.GetLinkedChild`).
- **Versions**: v12.3 (tag 06de749) and v12.2 (ca0f16eb), the same lines, read 2026-10-27. Not reproduced on a server.
- **Workaround**: before a favourite or a playback start on an episode whose series or season this session marked since, the core
  reads the episode's marks with a list read (`GET Items?ids=`) and sends its played mark on its own id (`UserPlayedItems/{id}`), which
  saves through the cached instance and rehydrates it (`Context::mend`, `crates/app/src/browse/mod.rs`, called by
  `JellyfinMedia::report_start` for playback). It does not cover another device's sweeps or an earlier session's, nor a plain folder's.
  Marking played again moves the episode's last-played date to now. The stand-in models the write-back (`Marks::write_back`,
  `tools/jellyfin-stand-in/src/library.rs`); `crates/app/tests/series_marks.rs` and the player's tests test it.
- **Removal**: once a write reads the user data it changes fresh, or a sweep saves the cached instances; re-check on each new server
  baseline. Reporting it upstream is the owner's call.

### A list read can fall back to the cached instance too
- `GetUserDataBatch`, which lists use, reads the per-user cache first and, on a miss, the item's instance's `UserData` before the
  database (v12.3, 06de749, read 2026-10-27). So a list read is stale in the rare case of an item still in the item cache
  (`LibraryManager`'s LRU of `Environment.ProcessorCount * 100` items) but dropped from the per-user cache. Not seen on a server; no
  workaround: the next write through the item, a scan, or the LRU puts it right.

## Sources
- Jellyfin server `v12.2` (ca0f16eb): `Jellyfin.Api/Controllers/{UserViewsController, ItemsController, UserLibraryController,
  TvShowsController, LibraryController, SearchController, PlaystateController, ImageController}.cs`,
  `Jellyfin.Api/Extensions/DtoExtensions.cs`, `Jellyfin.Api/Helpers/RequestHelpers.cs`, `MediaBrowser.Controller/Dto/DtoOptions.cs`,
  `MediaBrowser.Model/Dto/{BaseItemDto, UserItemDataDto, BaseItemPerson}.cs`, `MediaBrowser.Model/Entities/ChapterInfo.cs`,
  `MediaBrowser.Model/Querying/{ItemFields, ItemFilter}.cs`, `Jellyfin.Data/Enums/{CollectionType, ItemSortBy}.cs`,
  `MediaBrowser.Model/Drawing/ImageFormat.cs`, `MediaBrowser.Controller/Drawing/{ImageHelper, ImageProcessingOptions}.cs`,
  `src/Jellyfin.Drawing/ImageProcessor.cs`, `Emby.Server.Implementations/Library/{UserViewManager.cs, Search/SearchManager.cs}`. Read
  2026-10-25.
- jellyfin-web `v12.2` (61b1f890): `src/components/homesections/sections/{recentlyAdded, nextUp, resume}.ts`,
  `src/constants/homeSectionType.ts`, `src/components/cardbuilder/utils/url.ts`, `src/utils/items.ts`,
  `src/scripts/settings/userSettings.js`. Read 2026-10-25.
- Microsoft Learn, read 2026-10-25:
  - BitmapImage class (Windows App SDK), updated 2026-07-28:
    https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.media.imaging.bitmapimage
  - BitmapImage.UriSource, updated 2026-07-28:
    https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.media.imaging.bitmapimage.urisource
  - BitmapImage.DecodePixelWidth, updated 2026-07-28:
    https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.media.imaging.bitmapimage.decodepixelwidth
  - BitmapSource.SetSourceAsync, updated 2026-07-28:
    https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.media.imaging.bitmapsource.setsourceasync
  - ListView and GridView data virtualization, ms.date 2026-03-16:
    https://learn.microsoft.com/en-us/windows/apps/develop/performance/listview-and-gridview-data-optimization
  - ItemsRepeater, ms.date 2026-09-19: https://learn.microsoft.com/en-us/windows/apps/develop/ui/controls/items-repeater
  - User interface migration (including WinUI), ms.date 2026-07-13:
    https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/migrate-to-windows-app-sdk/guides/winui3
- A third-party cross-platform framework's image documentation (unpackaged `ms-appdata` statement), read 2026-10-25; not a Microsoft
  source, cited only as the reason the file path route is marked not verified.
