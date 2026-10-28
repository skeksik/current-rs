//! The session's side of the user's arrangement (ADR 0024, [`crate::navigation`]): the pane's groups as the user arranged them,
//! following the libraries and every saved change, Home's rows for the editor, and the editor's moves, hides and resets.

use std::sync::{Arc, Weak};

use current_model::{Capabilities, ItemRef, Library};
use current_store::{NavigationScope, StoreError};
use tokio::sync::watch;

use super::home::{self, HomeRowKind};
use super::{Context, kind_tabs};
use crate::navigation::{self, Arranged, BaseItem};

/// A group of items the user arranges.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NavGroup {
    /// The Movies and Series tabs.
    Tabs,
    /// Favourites and Recently added.
    Views,
    Libraries,
    /// Home's rows.
    HomeRows,
}

impl NavGroup {
    /// Its name in the settings file.
    fn name(self) -> &'static str {
        match self {
            Self::Tabs => navigation::TABS,
            Self::Views => navigation::VIEWS,
            Self::Libraries => navigation::LIBRARIES,
            Self::HomeRows => navigation::HOME_ROWS,
        }
    }
}

/// A tab or a view in the pane: `key` is the tab's kind (`movie`, `series`) or the view's ([`navigation::FAVOURITES`],
/// [`navigation::RECENTLY_ADDED`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneEntry {
    pub key: String,
    pub hidden: bool,
}

/// A library in the pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneLibrary {
    pub library: Library,
    pub hidden: bool,
    /// The server keeps it out of "My media": hidden until the user shows it here.
    pub hidden_on_server: bool,
}

/// The pane's groups of one server as the user arranged them, hidden items included (the pane leaves them out, its edit mode and
/// Settings show them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneLayout {
    pub tabs: Vec<PaneEntry>,
    pub views: Vec<PaneEntry>,
    pub libraries: Vec<PaneLibrary>,
}

/// One of Home's rows for the editor, also one without items now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HomeRowSetting {
    pub key: String,
    pub kind: HomeRowKind,
    /// The library's name for a Latest row; empty for the others, which the UI names.
    pub title: String,
    pub library: Option<ItemRef>,
    pub hidden: bool,
}

/// The views' keys in their default order.
const VIEWS: [&str; 2] = [navigation::FAVOURITES, navigation::RECENTLY_ADDED];

impl Context {
    /// Whose arrangement this session shows: the server's signed-in user.
    fn scope(&self) -> Option<NavigationScope> {
        let shared = self.shared.upgrade()?;
        let user = shared.saved(&self.server)?.active_user?;
        Some(NavigationScope::new(self.server.clone(), user))
    }

    /// `group`'s saved entries, oldest move first.
    fn placements(&self, group: NavGroup) -> Vec<current_store::Placement> {
        let (Some(shared), Some(scope)) = (self.shared.upgrade(), self.scope()) else {
            return Vec::new();
        };
        shared
            .navigation
            .group(shared.settings(), &scope, group.name())
    }

    /// The libraries the server keeps out of "My media", as the session last read them.
    fn hidden_on_server(&self) -> Vec<ItemRef> {
        self.client()
            .map(|client| client.hidden_libraries())
            .unwrap_or_default()
    }

    /// `group`'s default order, from `libraries` (the last list loaded).
    fn base(&self, group: NavGroup, libraries: &[Library]) -> Vec<BaseItem> {
        match group {
            NavGroup::Tabs => {
                let hidden = self.hidden_on_server();
                let shown: Vec<Library> = libraries
                    .iter()
                    .filter(|library| !hidden.contains(&library.item))
                    .cloned()
                    .collect();
                kind_tabs(&shown)
                    .iter()
                    .map(|kind| BaseItem::shown(kind.as_str()))
                    .collect()
            }
            NavGroup::Views => VIEWS.into_iter().map(BaseItem::shown).collect(),
            NavGroup::Libraries => {
                let hidden = self.hidden_on_server();
                libraries
                    .iter()
                    .map(|library| BaseItem {
                        key: library.item.id.as_str().to_owned(),
                        hidden: hidden.contains(&library.item),
                    })
                    .collect()
            }
            NavGroup::HomeRows => self
                .home_specs(libraries)
                .iter()
                .map(|spec| BaseItem::shown(spec.key()))
                .collect(),
        }
    }

    /// Home's rows in their default order: the Latest rows in the pane's order of the libraries.
    pub(super) fn home_specs(&self, libraries: &[Library]) -> Vec<home::Spec> {
        let mut excludes = self.hidden_on_server();
        if let Ok(client) = self.client() {
            excludes.extend(client.latest_excludes());
        }
        let ordered: Vec<Library> = self
            .arrange(NavGroup::Libraries, libraries)
            .into_iter()
            .filter_map(|item| {
                libraries
                    .iter()
                    .find(|library| library.item.id.as_str() == item.key)
                    .cloned()
            })
            .collect();
        home::specs(&ordered, &excludes)
    }

    /// `group` as the user arranged it.
    fn arrange(&self, group: NavGroup, libraries: &[Library]) -> Vec<Arranged> {
        navigation::arrange(&self.base(group, libraries), &self.placements(group))
    }

    /// Home's rows as the user arranged them, from `libraries`: each spec with whether it is hidden.
    pub(super) fn arranged_home(&self, libraries: &[Library]) -> Vec<(home::Spec, bool)> {
        let specs = self.home_specs(libraries);
        self.arrange(NavGroup::HomeRows, libraries)
            .into_iter()
            .filter_map(|item| {
                let spec = specs.iter().find(|spec| spec.key() == item.key)?.clone();
                Some((spec, item.hidden))
            })
            .collect()
    }

    /// The libraries as the last load read them; empty before.
    pub(super) fn known_libraries(&self) -> Vec<Library> {
        self.libraries
            .subscribe()
            .borrow()
            .clone()
            .unwrap_or_default()
    }

    /// The pane's groups from `libraries`.
    fn pane_layout(&self, libraries: &[Library]) -> PaneLayout {
        let entries = |group| {
            self.arrange(group, libraries)
                .into_iter()
                .map(|item| PaneEntry {
                    key: item.key,
                    hidden: item.hidden,
                })
                .collect()
        };
        PaneLayout {
            tabs: entries(NavGroup::Tabs),
            views: entries(NavGroup::Views),
            libraries: self
                .arrange(NavGroup::Libraries, libraries)
                .into_iter()
                .filter_map(|item| {
                    let library = libraries
                        .iter()
                        .find(|library| library.item.id.as_str() == item.key)?;
                    Some(PaneLibrary {
                        library: library.clone(),
                        hidden: item.hidden,
                        hidden_on_server: item.hidden_by_default,
                    })
                })
                .collect(),
        }
    }

    /// Keeps the pane's layout following the libraries and every saved arrangement, until the session's browse state is gone.
    pub(super) fn start_pane(self: &Arc<Self>) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let follower = self.spawn(follow_pane(
            Arc::downgrade(self),
            self.libraries.subscribe(),
            shared.navigation.subscribe(),
            shared.capabilities.subscribe(),
        ));
        *super::lock(&self.pane_follower) = Some(follower);
    }

    /// Replaces `group`'s entries with what `change` makes of them and its default order. Blocking file I/O.
    fn change_group(
        &self,
        group: NavGroup,
        change: impl FnOnce(&[BaseItem], &[current_store::Placement]) -> Vec<current_store::Placement>,
    ) -> Result<(), StoreError> {
        let (Some(shared), Some(scope)) = (self.shared.upgrade(), self.scope()) else {
            return Ok(());
        };
        let base = self.base(group, &self.known_libraries());
        let saved = self.placements(group);
        let entries = change(&base, &saved);
        shared
            .navigation
            .set_group(shared.settings(), &scope, group.name(), entries)
    }
}

/// Publishes the pane's layout again when the libraries change, the user saves an arrangement, or the server's status changes: the
/// user check that follows a restore reads the libraries kept out of "My media" (`GET /Users/Me`), which may come after the first
/// libraries.
async fn follow_pane(
    context: Weak<Context>,
    mut libraries: watch::Receiver<Option<Vec<Library>>>,
    mut revision: watch::Receiver<u64>,
    mut capabilities: watch::Receiver<Capabilities>,
) {
    loop {
        let Some(context) = context.upgrade() else {
            return;
        };
        let layout = libraries
            .borrow_and_update()
            .as_ref()
            .map(|libraries| context.pane_layout(libraries));
        revision.borrow_and_update();
        capabilities.borrow_and_update();
        context.pane.send_if_modified(|shown| {
            let changed = *shown != layout;
            *shown = layout;
            changed
        });
        drop(context);
        tokio::select! {
            changed = libraries.changed() => if changed.is_err() { return },
            changed = revision.changed() => if changed.is_err() { return },
            changed = capabilities.changed() => if changed.is_err() { return },
        }
    }
}

impl super::Browse {
    /// The pane's groups as the user arranged them: `None` until the libraries first loaded. It follows the libraries and every
    /// change the user saves, on this device (ADR 0024).
    pub fn pane(&self) -> watch::Receiver<Option<PaneLayout>> {
        self.ctx.pane.subscribe()
    }

    /// Home's rows as the user arranged them, hidden ones and those without items now included, for the editor.
    pub fn home_layout(&self) -> Vec<HomeRowSetting> {
        self.ctx
            .arranged_home(&self.ctx.known_libraries())
            .into_iter()
            .map(|(spec, hidden)| {
                let row = spec.row();
                HomeRowSetting {
                    key: row.key,
                    kind: row.kind,
                    title: row.title,
                    library: row.library,
                    hidden,
                }
            })
            .collect()
    }

    /// Moves `key` to `to` in `group` (an index in the group as arranged, hidden items included) and saves it. Blocking file I/O:
    /// call it off the async runtime.
    ///
    /// # Errors
    /// The settings could not be saved (a newer version's file, a failed write): nothing changed.
    pub fn move_item(&self, group: NavGroup, key: &str, to: usize) -> Result<(), StoreError> {
        self.ctx
            .change_group(group, |base, saved| navigation::moved(base, saved, key, to))
    }

    /// Hides or shows `key` in `group` and saves it, as [`super::Browse::move_item`].
    ///
    /// # Errors
    /// As [`super::Browse::move_item`].
    pub fn set_hidden(&self, group: NavGroup, key: &str, hidden: bool) -> Result<(), StoreError> {
        self.ctx.change_group(group, |base, saved| {
            navigation::with_hidden(base, saved, key, hidden)
        })
    }

    /// Forgets the user's arrangement of `group`: back to the server's or the app's order. As [`super::Browse::move_item`].
    ///
    /// # Errors
    /// As [`super::Browse::move_item`].
    pub fn reset_group(&self, group: NavGroup) -> Result<(), StoreError> {
        self.ctx.change_group(group, |_, _| Vec::new())
    }
}
