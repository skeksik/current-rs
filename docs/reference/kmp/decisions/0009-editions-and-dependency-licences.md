# 0009: Editions and dependency licences

Status: accepted (2026-10-18); the editions are superseded by ADR 0014 (2026-10-19): two editions, Store and Private, and no build
switch. The licence rules below stand.

## Context
Current is meant to be sold and its code may become private. Some features are welcome in app stores, others are not (store rules on adult
content) or need licences we may not have (full Dolby Vision, some codec patents). The removable extensions of ADR 0008 already let a build
leave features out; this decides which builds exist and which third-party licences each may contain. Facts and sources: docs/platform-notes.md.

## Decision
**Dependency licences.** Every distributed build may contain only permissively licensed code (MIT, BSD, ISC, Apache-2.0, zlib and the like)
and LGPL-2.1 libraries. No GPL or AGPL code in a distributed build. LGPL-3.0 or MPL-2.0 only with an explicit decision per library.
For every LGPL library we ship: its licence and source (with our changes and build configuration) are published with each release, it stays
replaceable (a separate dynamic library; where only static linking is possible, our app is offered as object files for relinking), our terms
allow modifying and debugging it, and the app has an open-source licences screen.

**Editions**, chosen by the same build switch as the extensions:
| Edition | Distribution | Contains | Leaves out |
|---|---|---|---|
| Store | App Store, Google Play, Microsoft Store | core features, OS and hardware decoders first, Dolby Vision through the native player or its base layer; the adult extension only where a store accepts it | anything whose licence or patents are unclear |
| Full | direct downloads (MSI, DEB, APK, web), sold commercially | Store + the adult extension + software decoders for the long tail of formats | the same unclear-licence pieces as Store |
| Personal | never distributed; built for one's own devices | Full + pieces whose licence forbids commercial or closed distribution (for example full Dolby Vision processing, GPL-only components) | nothing |

The Personal edition exists only as a build switch and is never published, sold or handed to anyone: distribution is what triggers the GPL's
obligations. Patents are different (they can cover use too), so a Personal build lowers the licence risk, not necessarily the patent risk.
It is built only when a concrete feature needs it.

## Consequences
- Engine builds are ours: prebuilt packages often enable GPL or LGPL-3.0 parts (mpv's default build, mpv-android, MPVKit's default products,
  FFmpeg built with `--enable-gpl` or `--enable-version3`), so FFmpeg and libmpv are built from source in LGPL-2.1 configurations.
- Patents are a separate check per codec before a commercial release: prefer OS and hardware decoders for HEVC and AAC; review the software
  decoders the Full edition ships (for example VC-1/WMV, RealVideo, DTS).
- (Superseded by ADR 0014, which drops extensions; kept for the store research.) The adult extension in stores is decided per store. The app ships no content and only organizes the user's own library, and a Stash
  client is listed in the US App Store with a 16+ rating (checked 2026-10-18), so it is possible there; review still judges each app.
  Ship it disabled until the user connects a server that offers it, and keep it removable so a rejected store gets a build without it.
- **Current's own code is proprietary.** No open-source licence is granted, so no LICENSE file: without one, copyright law reserves all
  rights to the owner. A licence granted on public code cannot be taken back for copies already made, so none is added while the repo
  is public. Third-party licences still apply from the first build given to anyone else (testers included).
