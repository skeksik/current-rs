# 0005: Core domain model

Status: accepted (2026-10-22). Amended 2026-10-25 (ADR 0018, approved by the owner): browsing types and `Artwork.image`.

## Context
Every later crate (services, playback, app state, the bridge) shares one vocabulary for servers, items, users and what each server
offers. Anyone may point Current at any mix of Jellyfin, Seerr and Stash servers, several of each, and servers add values over time.

## Decision
- **Identity is server + id.** A server is a `ServerRef`: its `ServiceType` and a `ServerKey` the app assigns when the server is added.
  The key is opaque and never a hostname or URL, so it survives address changes (roaming, M1) and is safe in logs (`jellyfin/1`). Ids
  (`ItemId`, `UserId`) are opaque strings unique only within one server; an item anywhere is an `ItemRef` (server + id).
- **Open values for what servers define:** `ServiceType`, `MediaKind`, `ImageType`, `Permission`, `Feature`. Each is a string newtype
  with associated constants for the values Current gives meaning to. Any other value is still valid and round-trips byte for byte: no
  case folding, no trimming, no mapping to "unknown". Service crates map their server's wire values to the constants where the meaning
  matches and pass everything else through. `Reachability` is a closed enum because Current, not a server, defines it.
- **Playback values** (ADR 0019, amended 2026-10-25): `MediaSource`, `MediaStream`, `Route`, `PlaybackPlan`, `Segment`,
  `PlaybackProblem`, `TranscodingFacts`, `PlaybackPreferences` and `EngineCapabilities`, with the open values `StreamKind`, `Codec`
  (FFmpeg's names), `Container`, `VideoRange`, `VideoRangeType`, `SubtitleDelivery`, `RouteReason`, `SegmentKind`, `SubtitleMode`
  and `EngineErrorKind`. `Route` is closed: Current defines the routes. No URL, path or token is in any of them.
- **Artwork** carries its type, URL (a string: the model parses nothing), an optional aspect ratio stored reduced (`AspectRatio::new`
  reduces 1920:1080 to 16:9 and rejects zero sides) and an optional BlurHash.
- **Users** carry id, server, name, permissions and an optional avatar.
- **Capabilities:** one `ServerStatus` per configured server (signed in, reachability, user, version, features). A feature asks
  `Capabilities::is_available(&ServiceType)` before showing itself: a signed-in server of that service exists. Reachability is separate,
  so an unreachable server keeps its features with an offline state (docs/architecture.md).
- **Pure:** no I/O and no serde. Wire formats belong to the service crates (wire types stay private there) and storage to
  `current-store`; each maps to these types. No dependencies at all.
- **Bridge-ready:** plain records and enums with owned fields, no generics or lifetimes in public shapes, so UniFFI can export them as
  records and custom string types once D1 fixes the bridge pattern. UniFFI is not added here.

## Consequences
- Comparing two items without their server is impossible by construction.
- A new server value needs no release to pass through; giving it meaning is a new constant plus the service crate's mapping.
- Constants for media kinds, image types and permissions are a first set; service crates add constants when they map a new meaning.

## Amendment (2026-10-25, from ADR 0018)
Approved by the owner with ADR 0018.
- **`Artwork.url` becomes `Artwork.image: ImageRef`** (item, image type, index, tag). An address changes with roaming (ADR 0014) and
  must not cross the bridge, so the service crate builds the URL when it fetches, from the address that serves at that moment.
- The model gains the browsing records `UserState`, `ItemSummary` (with `EpisodeLinks`), `ItemDetails`, `Library`, `CalendarDate`,
  `MediaSummary`, `StreamSummary`, `Person`, `Chapter` and `ItemQuery`, the open values `CollectionKind`, `SortKey`, `ItemFilter` and
  `PersonKind`, the closed `SortOrder` (Current defines it), and new `MediaKind` constants (`MUSIC_ALBUM`, `AUDIO`, `MUSIC_ARTIST`,
  `MUSIC_VIDEO`, `VIDEO`, `FOLDER`, `BOX_SET`). Durations are `std::time::Duration`; dates are `CalendarDate`, since the model takes no
  dependency for a date type. Still pure, with no dependencies.
