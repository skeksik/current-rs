# 0001: A native UI per platform over a shared Rust core

Status: accepted (2026-10-22)

## Context
Current must run on Windows, macOS and Linux first, then Android, iOS, tvOS and the web, and it must feel native on each. A single
cross-platform UI toolkit saves writing screens more than once, but on desktop it gives up exactly what a media app needs: drawing its own
controls over hardware-rendered HDR video, the platform's accessibility APIs, and the platform's own navigation, keyboard, focus and
windowing conventions, which then have to be imitated by hand. Keeping the logic shared and the UI native avoids that trade.

## Decision
- **A shared Rust core** holds all logic: service clients, storage, playback planning and the presentation state each screen renders
  (ADR 0002).
- **A native UI per platform**, each written in the platform's own toolkit:

| Platform | UI | Order |
|---|---|---|
| Windows | WinUI 3 (Windows App SDK) in C# | first |
| macOS | SwiftUI, with AppKit where SwiftUI has no API | second |
| Linux | GTK4 + libadwaita, in Rust (gtk4-rs) | third |
| Android, iOS/iPadOS, tvOS, web | Jetpack Compose; SwiftUI (shared with macOS); chosen later | after the desktop apps |

- **Desktop first.** Spikes, the first usable version and the next features target Windows, then macOS, then Linux. Android, iOS, tvOS
  and the web follow once the desktop apps are usable. The core stays portable to them (ADR 0002).
- **Native behaviour by default.** Each app looks and behaves as its platform's own apps do: its controls, navigation, keyboard, focus,
  windowing, accessibility, materials and conventions. Fluent with Mica and acrylic on Windows; Liquid Glass and Apple's Human Interface
  Guidelines on macOS; the GNOME Human Interface Guidelines on Linux. Where the platform has a behaviour, we keep it. Departing from it
  needs a reason (the platform's behaviour is questionable for this app) and the owner's agreement, recorded in a decision record.
- **The player is our own on every platform** (custom controls over the video, never the system player view), but it looks and
  behaves like a player native to that platform: Liquid Glass on Apple, Fluent on Windows, libadwaita on Linux. Its layout may differ
  from the system player's (buttons placed differently), and each platform gets the features this project needs on top.

## Consequences
- Every screen is written once per platform UI. The shared core keeps that to rendering and platform integration: no logic is
  duplicated in a UI.
- Accessibility comes from the native toolkits (UI Automation in WinUI, the Apple accessibility APIs, AT-SPI in GTK) instead of being
  rebuilt.
- Each platform needs its own video surface and its own system integration (media keys and the media flyout on Windows, Now Playing,
  PiP and AirPlay on Apple, MPRIS on Linux). The desktop spikes check these first (docs/roadmap.md).
- C# and Swift cannot be built in the Linux cloud containers; their builds run in CI on Windows and macOS runners and on the owner's
  machines.
