//! A large generated library for the paged-list tests (ADR 0018, section 3), and the controls those tests use: holding or failing
//! one page, reading which pages were asked for, changing the library's size and names, and the user's "exclude from latest"
//! libraries. Kept apart from [`crate::library`]. The library is not among the user's views: a test opens it by its id. Every name
//! is an example.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{Controls, lock};

/// The generated library.
pub const BIG_LIBRARY_ID: &str = "57a2d1e0000000000000000000006000";
/// How many items it holds until [`Controls::set_big_library_size`] changes it.
pub const BIG_LIBRARY_SIZE: usize = 450;

/// The id of the generated library's item at `index` (in name order).
pub fn big_item_id(index: usize) -> String {
    format!("57a2d1e00000000000000000007{index:05x}")
}

/// The name of the generated library's item at `index`, at `revision` ([`Controls::set_big_library_revision`]).
pub fn big_item_name(index: usize, revision: usize) -> String {
    if revision == 0 {
        format!("Example Item {index:04}")
    } else {
        format!("Example Item {index:04} (revision {revision})")
    }
}

/// What [`Controls::hold`], [`Controls::fail`] and [`Controls::cancelled`] take for the generated library's page that starts at
/// `start`.
pub fn page_key(start: usize) -> String {
    format!("/Items@{start}")
}

/// A request for a page of the generated library, as received.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSeen {
    pub start: usize,
    pub limit: usize,
    /// Whether it asked for the total (`enableTotalRecordCount` not `false`).
    pub with_total: bool,
}

#[derive(Default)]
pub(crate) struct Paging {
    /// The library's size; [`BIG_LIBRARY_SIZE`] when unset.
    size: Mutex<Option<usize>>,
    revision: AtomicUsize,
    /// Paths and page keys that answer 500.
    failing: Mutex<HashSet<String>>,
    pages: Mutex<Vec<PageSeen>>,
    /// The ids in the user's `LatestItemsExcludes`.
    pub(crate) latest_excludes: Mutex<Vec<String>>,
}

impl Paging {
    fn size(&self) -> usize {
        lock(&self.size).unwrap_or(BIG_LIBRARY_SIZE)
    }
}

/// The page key of a request for a page of the generated library, or `None` for any other request.
pub(crate) fn key(path: &str, query: &[(String, String)]) -> Option<String> {
    (path == "/Items" && param(query, "parentId") == Some(BIG_LIBRARY_ID))
        .then(|| page_key(number(query, "startIndex").unwrap_or(0)))
}

/// Records a page request.
pub(crate) fn seen(paging: &Paging, query: &[(String, String)]) {
    lock(&paging.pages).push(PageSeen {
        start: number(query, "startIndex").unwrap_or(0),
        limit: number(query, "limit").unwrap_or(usize::MAX),
        with_total: param(query, "enableTotalRecordCount") != Some("false"),
    });
}

/// Whether a request to `path` (with `key`, for a page) is to fail.
pub(crate) fn failing(paging: &Paging, path: &str, key: Option<&str>) -> bool {
    let failing = lock(&paging.failing);
    failing.contains(path) || key.is_some_and(|key| failing.contains(key))
}

/// A page of the generated library, in name order.
pub(crate) fn answer(paging: &Paging, query: &[(String, String)]) -> (u16, String) {
    let size = paging.size();
    let revision = paging.revision.load(Ordering::SeqCst);
    let start = number(query, "startIndex").unwrap_or(0);
    let limit = number(query, "limit").unwrap_or(usize::MAX);
    let items: Vec<String> = (start..size)
        .take(limit)
        .map(|index| {
            let id = big_item_id(index);
            format!(
                r#"{{"Name":"{}","ServerId":"{}","Id":"{id}","Type":"Movie","IsFolder":false,"ImageTags":{{}},"BackdropImageTags":[],"UserData":{{"PlaybackPositionTicks":0,"PlayCount":0,"IsFavorite":false,"Played":false,"Key":"{id}","ItemId":"{id}"}}}}"#,
                big_item_name(index, revision),
                crate::SERVER_ID
            )
        })
        .collect();
    let total = if param(query, "enableTotalRecordCount") == Some("false") {
        0
    } else {
        size
    };
    (
        200,
        format!(
            r#"{{"Items":[{}],"TotalRecordCount":{total},"StartIndex":{start}}}"#,
            items.join(",")
        ),
    )
}

fn param<'a>(query: &'a [(String, String)], key: &str) -> Option<&'a str> {
    query
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(key))
        .map(|(_, value)| value.as_str())
}

fn number(query: &[(String, String)], key: &str) -> Option<usize> {
    param(query, key)?.parse().ok()
}

impl Controls {
    /// Requests to `path_or_page` (a path, or a [`page_key`]) answer 500 until [`Controls::unfail`].
    pub fn fail(&self, path_or_page: &str) {
        lock(&self.state.paging.failing).insert(path_or_page.to_owned());
    }

    /// Requests to `path_or_page` answer again.
    pub fn unfail(&self, path_or_page: &str) {
        lock(&self.state.paging.failing).remove(path_or_page);
    }

    /// The requests for pages of the generated library so far, oldest first.
    pub fn pages(&self) -> Vec<PageSeen> {
        lock(&self.state.paging.pages).clone()
    }

    /// Forgets the page requests seen so far.
    pub fn clear_pages(&self) {
        lock(&self.state.paging.pages).clear();
    }

    /// Makes the generated library hold `size` items.
    pub fn set_big_library_size(&self, size: usize) {
        *lock(&self.state.paging.size) = Some(size);
    }

    /// Renames every item of the generated library ([`big_item_name`]).
    pub fn set_big_library_revision(&self, revision: usize) {
        self.state.paging.revision.store(revision, Ordering::SeqCst);
    }

    /// The libraries the user keeps out of "latest", as `/Users/Me` and the sign-in report them from now on.
    pub fn set_latest_excludes(&self, libraries: &[&str]) {
        *lock(&self.state.paging.latest_excludes) =
            libraries.iter().map(|id| (*id).to_owned()).collect();
    }
}
