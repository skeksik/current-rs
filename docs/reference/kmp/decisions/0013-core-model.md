# 0013: Core domain model: identity and open values

Status: accepted (2026-10-19), with the first `core:model` sources. ADR 0014 later dropped the extensions mentioned in ADR 0008 and
added the capabilities state to `core:model`.

## Context
Every later M1 item builds on `core:model`: the UIs pick how to show an item by its media kind, the capabilities state reads the
user's permissions, and the data layers map their DTOs onto it. ADR 0008 makes media kinds open and every service optional. The types
reach three UIs: Compose directly, SwiftUI through the `Shared` framework, and React through the `web` facade (ADR 0011).

## Decision
- **Identity is server + id.** `MediaId(server: ServerRef, id)`, with `ServerRef(service: ServiceType, key)`. Ids are only unique
  within one server, and several servers per device is on the roadmap, so the server is part of the key from the start. The `key` is
  opaque and assigned by the app when a server is added: never a hostname, URL or server-reported id. The `id` is opaque too; a service
  whose ids are only unique per kind (TMDB ids in Seerr) puts the kind into it in its data layer.
- **Services are open, like kinds.** `ServiceType` has the constants `jellyfin`, `seerr` and `stash`, but a later source (local files, M7)
  adds its own without changing core. Features read the capabilities state, not a `when` over services.
- **One shape for every open value:** `ServiceType`, `MediaKind`, `ImageType` and `Permission` are each `data class X(val value: String)`
  with lowerCamel companion constants (`MediaKind.movie`, read in Swift like enum cases) and a `known` list. Values are compared as given,
  never normalised, so unknown ones round-trip unchanged. Data layers map their service's names onto the known values and pass anything
  else through; values the app adds itself are prefixed (`local.file`) so they cannot collide with a service's names.
- **Plain classes only.** No sealed hierarchies and no value classes: how value classes look in Swift and through `@JsExport` has not
  been checked (docs/platform-notes.md has nothing on it), while plain classes are what the `web` facade already exports. The facade
  maps these types to its own view classes anyway (ADR 0011).
- **Artwork** is `Artwork(type, url, aspectRatio?, blurHash?)`. The data layer builds the URL (core knows no server).
  `AspectRatio` keeps the reduced fraction, so a 1000×1500 image equals the 2:3 poster constant.
- **User** is `User(id, server, name, permissions, avatar?)`, one per server; Stash has none (one key per server). Permissions start
  small (`admin`, `request`, `request4k`, `manageRequests`) and do not imply each other: the data layer expands its service's rules, such
  as an admin flag that grants everything, into the full set.
- **No kotlinx.serialization** in `core:model`: DTOs stay `internal` in `data:*` and nothing here is serialised directly.

## Consequences
- No exhaustive `when` over kinds, services or permissions anywhere; code handles the values it knows and a default.
- A typo in a value is a new value, not a compile error: use the constants.
- Left for later: provider ids and joins (M2, M3),
  image sizes per request (M2, with `data:jellyfin` images), request and media status (M3).
