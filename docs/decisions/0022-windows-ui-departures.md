# 0022: Windows UI departures

Status: accepted (2026-10-27, owner), from the Windows UI polish pass (fix/windows-ui-polish) and the manager's review of it.

## Context
ADR 0001 makes native behaviour the default: "Departing from it needs a reason and the owner's agreement, recorded in a decision record."
The Windows UI pass took sizes and behaviour from Microsoft's design pages and, where they are silent, from Microsoft's own apps
(Windows Settings, File Explorer), with each source quoted next to the value in the code. A few of the owner's choices depart from
that guidance or go beyond it. This record lists them with their reasons, so a later pass does not "fix" them back and macOS and Linux
know which choices are Windows' own and which are the owner's.

## Decision
Each choice below stays until the owner changes it.

1. **An icon on every menu item** of a library's Sort, Filter and View menus. Microsoft: "Consider providing menu item icons for: The
   most commonly used items. Menu items whose icon is standard or well known. Menu items whose icon well illustrates what the command
   does", and "Don't feel obligated to provide icons for commands that don't have a standard visualization" ("Menu flyout and menu
   bar", learn.microsoft.com, ms.date 2026-10-13, read 2026-10-27). Reason: the owner prefers every item with a glyph, so the menus
   scan alike (owner, 2026-10-26). Each glyph comes from Segoe Fluent Icons; an item with no clear glyph uses the nearest one.
2. **Sort, Filter and View as icon-only buttons** with a chevron, on the page's title row. File Explorer labels its Sort and View
   buttons (seen on the owner's PC, 2026-10-27). Reason: the owner wants the title row quiet (2026-10-26). Each button keeps a tooltip
   and a Narrator name.
3. **The title bar's search tied to the pane's mode, and modal when folded.** Microsoft: "If global search functionality is present, a
   searchbox should be added to the title bar, centered to the window" and "The search box needs to be responsive to react to window
   size changes" ("Windows app title bar", learn.microsoft.com, ms.date 2024-07-31, read 2026-10-27); it says nothing of how. Current
   follows Windows Settings (measured on the owner's PC, 2026-10-26): when the navigation pane folds to its icon rail, the box becomes
   a Search button that opens a box under the title bar, over the page, on a solid plate with a shadow. Beyond Settings: while that
   box is open, a clear layer blocks clicks on everything behind it, the title bar's buttons included (owner, 2026-10-26), and Esc or
   a click outside closes it.
4. **A Refresh button in the title bar**, beside Back, on pages that can refresh (Home, a library, a details page); F5 and Ctrl+R stay.
   The title bar page above lists a back button, icon, title, search, a person picture and tabs, not other commands. Reason: one place
   for Refresh on every page, instead of a button on each page (owner, 2026-10-26).
5. **The tile shade.** An in-progress tile shows its play mark, bar and time left in the dark theme whatever the app's theme, on a
   shade from clear to 80 % black over the poster's bottom (App.xaml, `TileScrimBrush`). The rest of the app follows the Windows light
   or dark setting. Reason: white text on a darkened picture reads on any poster; the shade is measured against the 4.5:1 text
   contrast ("Accessible text requirements", learn.microsoft.com, ms.date 2026-08-21) at the lightest point behind the glyphs (the
   comment in App.xaml quotes the method and result). High contrast replaces it with the system's window colour.
6. **2560 × 1440 named "1080p"** in the quality menu and its setting. Jellyfin names anything up to 2560 × 1440 "1080p"
   (`MediaStream.GetResolutionText`, jellyfin/jellyfin master, read 2026-10-26; ADR 0019, choice 10). Reason: the names then match the
   server's own and those of other clients of it; "1440p" is not used for now (owner, 2026-10-26).

7. **The search's Catalog source shown, unavailable, without a Seerr server** (owner, 2026-10-27; ADR 0018, "Global search filters").
   CLAUDE.md: "No feature may assume a service exists: read the capabilities state and hide what is unavailable." Reason: the app is
   also used without Seerr, and the option should be visible. The item is disabled (it takes no focus, and Narrator reads it as
   unavailable) and its own name says why, "Catalog (needs Seerr)", as a disabled control shows no tooltip ("It can't handle any input
   events on itself", Control.IsEnabled, learn.microsoft.com, updated 2026-07-28, read 2026-10-27).

## Consequences
- A review checks a new menu, title bar item or shade against this list before calling it a departure.
- macOS and Linux decide their own; none of these carries over without the owner's choice for that platform.
