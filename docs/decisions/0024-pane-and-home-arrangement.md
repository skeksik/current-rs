# 0024: The pane's and Home's arrangement, kept in Current

Status: Accepted (owner, 2026-10-28), from feat/pane-order. The owner chose the storage, the merge rule, the Settings section, the
pane's edit mode and the Edit Home dialog on 2026-10-28.

## Context
The owner wants to reorder or hide the pane's items within their groups (the Movies and Series tabs; Favourites and Recently added;
the libraries), with Home and Settings fixed, and to reorder and hide Home's rows (roadmap, owner, 2026-10-26 and 2026-10-28).

What Jellyfin 12.3 offers (tag v12.3, 06de749, source read 2026-10-28; not checked on a server):
- **The user's configuration.** `UserConfiguration` (MediaBrowser.Model/Configuration/UserConfiguration.cs) has `OrderedViews` (the
  library order), `MyMediaExcludes` (libraries hidden from "My media"), `LatestItemsExcludes` and `GroupedFolders`, all `Guid[]`.
- **Who honours them.** `UserViewManager.GetUserViews`, behind `GET /UserViews`, leaves out `MyMediaExcludes` unless
  `includeHidden=true`, then orders by the index in `OrderedViews`; views not in it come last, by sort name. `LatestItemsExcludes` is
  honoured by `UserViewManager.GetLatestItems`, `TVSeriesManager.GetNextUp` and `ItemsController` (Latest).
- **Writing them.** `POST /Users/Configuration` (`UserController.UpdateUserConfiguration`) takes the whole configuration, and
  `UserManager.UpdateConfigurationAsync` overwrites every field: a read, change and write back, the last write winning. It needs
  `UserPolicy.EnableUserPreferenceAccess` for the user's own configuration (`RequestHelpers.AssertCanUpdateUser`), else 403. It
  publishes no `UserUpdated` message, so another client's change is not pushed (ADR 0023). The lists hold library ids only.
- **Renamed libraries.** `LibraryStructureController.ReplaceLibraryIdInUserPreferencesAsync` rewrites a library's old id to its new
  one in those lists; nothing removes a removed library's id.

WinUI's NavigationView has no reordering (no `CanReorderItems` or reorder event in its class reference, Windows App SDK 2.0, updated
2026-07-28, read 2026-10-28). ListView has it: "ListViews support drag-based reordering out of the box ... At minimum, you add the
AllowDrop and CanReorderItems properties" ("Drag and drop", learn.microsoft.com, ms.date 2026-09-27, read 2026-10-28).

## Decision
1. **Kept in Current, never written to the server** (owner, 2026-10-28). The order and the hidden state are kept per server and user
   in the settings file (ADR 0013), `navigation`, keyed `service/server key/user id`. Each group (`tabs`, `views`, `libraries`,
   `home_rows`) is a list of entries `{key, after?, hidden?}`; keys, groups and unknown fields are open values and pass through.
   Current never writes the user's Jellyfin configuration: no read-modify-write, no 403 case.
2. **Jellyfin's order is the default.** The libraries come from `GET /UserViews?includeHidden=true`, in Jellyfin's order; those in
   the user's `MyMediaExcludes` start hidden, and the editor says "Hidden on the server". The tabs, the views and Home's rows start in
   the app's order: Movies, Series; Favourites, Recently added; Continue watching, Next up, Favourites, Recently added, the Latest
   rows in the pane's library order, Seen recently. A Latest row exists only for a library not in `LatestItemsExcludes` nor in
   `MyMediaExcludes` and of a kind with a "latest" (rows the server leaves out stay out): a library hidden on the server gets no
   Latest row even when the user shows it in the pane, as before this record (owner, 2026-10-28).
3. **The merge rule** (owner, 2026-10-28; `current-app`, `navigation`):
   - An item the user never moved keeps its place in the default order relative to the other unmoved items, so it follows a change
     in Jellyfin's order.
   - Moving an item stores the item just above it at the drop (or "first") and makes it the group's latest move.
   - Items that were placed right after the moved item are re-anchored onto the item that was above it, so they stay where they
     showed (owner, 2026-10-28: without it they followed the moved item, which a test of random moves found).
   - Resolving takes the unmoved items in the default order, then puts each moved item, oldest move first, right after its anchor;
     an anchor not in the list now is followed along its own anchor, and when nothing on that chain is, the item goes first.
   - The user's hide or show wins over the default; choosing what the default says forgets the choice. A new item shows in its
     default place (a new Jellyfin library last, as `/UserViews` lists it). A removed item's entry is kept: it still anchors others,
     and the item returns to its place if its id comes back. Reset forgets the group. Removing a server forgets its users' entries.
4. **Home asks only for the rows that show**, in the user's order; a row shown again loads.
5. **Windows** (ADR 0022, departures 8 and 9):
   - Settings → Personalization: two expanders, "Navigation pane" (a list per group, per server) and "Home" (its rows), each a
     ListView reordered by dragging (CanReorderItems, AllowDrop, CanDragItems), with Move up and Move down, Alt+Up and Alt+Down, a
     ToggleSwitch per item, and a Reset button per group.
   - The pane's edit mode, entered only from an "Edit navigation" item with a pencil in the pane's footer, shown while the pane is
     open; left with Done, Escape, choosing a page, or closing the pane. The same lists, an eye toggle button instead of the switch
     (the 300 px pane has no room for one). Home shows above the lists and Settings in the footer, both dimmed and fixed (the
     design's mock): the pane's custom content sits above the menu items (NavigationView.xaml, microsoft-ui-xaml main at b9fa30e,
     read 2026-10-28), so Home there is a dimmed label and the real item is collapsed meanwhile; Settings is disabled.
   - An "Edit Home" button in Home's title row opens the Home list in a ContentDialog, with Done.
   - Every change is saved at once; the pane and Home follow.

## Consequences
- Nothing syncs between devices or with other clients: the arrangement is this install's. Jellyfin's per-client
  `DisplayPreferences` (`DisplayPreferencesController`, `CustomPrefs` under a client name, no `AssertCanUpdateUser` check) could carry
  it later, if the owner asks.
- Like "exclude from latest", Jellyfin's `MyMediaExcludes` is read at sign-in and at the user check (`GET /Users/Me`); a change on
  the server while Current runs shows after the next check or a restart.
- Not verified: Alt+Up and Alt+Down as a Windows convention; whether ListView's drag reordering works from the keyboard on its own;
  where the footer item sits relative to Settings; the edit lists' height in the pane's custom content (to check on the owner's
  run).
- macOS and Linux build their own editors on the same core API (`Browse::pane`, `home_layout`, `move_item`, `set_hidden`,
  `reset_group`).
