# Player UX

Rules for the player on every platform. The player UI is our own on every engine (ADR 0007); these rules are what that UI does. The
controller (`feature:player`) holds the state and the rules that are not drawing; each UI (SwiftUI, Compose, React) renders them. None
of this is built yet. Home, offline states and settings: docs/architecture.md, "UX rules".

## Keyboard (desktop, web, Mac)
- Space play/pause, Left/Right seek, Up/Down volume, M mute, F full screen. Each shows a brief on-screen indicator.
- Esc closes only the topmost transient UI (a menu or sheet first), as everywhere else (docs/architecture.md); with none open it
  leaves full screen. It never closes the player: that is Back.
- Closing a window or the player saves the resume point first.
- Back works as elsewhere: Alt+Left, the mouse back button, Cmd+[ on a Mac.

## Pointer
- Controls show on pointer move and hide after an idle time; the cursor hides with them.
- Cards and controls have hover states.
- Right-click opens the same menu as a long-press.

## Scrubber
- Click to seek, drag to scrub.
- Hovering shows the time and a trickplay thumbnail.
- Scrubbing shows a thumbnail and still shows where playback is.

## Touch
- Long-press plays at 2x while held, with a small chip.
- Pinch toggles fit and fill.
- The player follows the device orientation. "Lock to landscape" appears only where it applies.

## Subtitles and tracks
- Subtitles move up while the controls show, never under them.
- Track menus group tracks by language, then forced / SDH / full, instead of one flat list. Each track has a detail line: codec,
  channels, Default or Forced.

## Time
Elapsed, remaining, and "Ends at HH:MM", all adjusted for playback speed.

## Next Up
- When the server has no outro segment, a compact prompt shows N seconds before the end (N is a setting).
- The next item loads in the same player.

## Picture in Picture
- Starts automatically when the user leaves the app while playing, where the platform allows (docs/platform-notes.md, "Apple system
  integration with a custom player").
- Returning restores the player's layout and orientation. This needs an explicit test: an existing third-party Jellyfin client on
  Apple platforms came back from PiP on an iPhone rotated, with dead controls.

## Accessibility
- Every control has a spoken label we wrote. Never an SF Symbol or icon name ("waveform", "gearshape") or a generic word ("Edit").
- Dynamic Type scales titles too.
