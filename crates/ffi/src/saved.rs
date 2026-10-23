//! The saved servers as a list the UI mirrors with diffs (ADR 0004). The app publishes the whole list on every change; this keeps an
//! [`ObservableList`] in step with it, keyed by server key, so that one server signing in is one `Set`, not a `Reset`.

use std::collections::HashSet;
use std::sync::Arc;

use current_app::ObservableList;
use current_model::SavedServer;
use tokio::runtime::Handle;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::diff::list_diff;
use crate::records::{SavedServerSnapshot, ServerRef};

list_diff!(
    /// A change to the saved servers.
    SavedServerDiff,
    SavedServerSnapshot
);

/// Mirrors `saved` into `list` until the app goes away or the task is aborted. The list matches the app's at once.
pub(crate) fn pump(
    runtime: &Handle,
    mut saved: watch::Receiver<Vec<SavedServer>>,
    list: Arc<ObservableList<SavedServerSnapshot>>,
) -> JoinHandle<()> {
    list.reset(snapshot(&saved.borrow_and_update()));
    runtime.spawn(async move {
        while saved.changed().await.is_ok() {
            let next = snapshot(&saved.borrow_and_update());
            sync(&list, next);
        }
    })
}

fn snapshot(saved: &[SavedServer]) -> Vec<SavedServerSnapshot> {
    saved.iter().map(Into::into).collect()
}

/// Changes `list` into `next` with the fewest diffs for the changes the app makes: servers removed, added, or changed in place. If
/// the servers that stay were reordered, the list is reset instead.
pub(crate) fn sync(list: &ObservableList<SavedServerSnapshot>, next: Vec<SavedServerSnapshot>) {
    let keys: HashSet<&ServerRef> = next.iter().map(|saved| &saved.server).collect();
    let mut current = list.snapshot();
    // Only this function changes the list, so the indices it computes stay valid.
    for index in (0..current.len()).rev() {
        if !keys.contains(&current[index].server) {
            let _ = list.remove(index);
            current.remove(index);
        }
    }
    let kept: HashSet<&ServerRef> = current.iter().map(|saved| &saved.server).collect();
    let same_order = current.iter().map(|saved| &saved.server).eq(next
        .iter()
        .map(|saved| &saved.server)
        .filter(|key| kept.contains(key)));
    if !same_order {
        list.reset(next);
        return;
    }
    let mut old = current.into_iter().peekable();
    for (index, saved) in next.into_iter().enumerate() {
        match old.peek() {
            Some(previous) if previous.server == saved.server => {
                if *previous != saved {
                    let _ = list.set(index, saved);
                }
                old.next();
            }
            _ => {
                let _ = list.insert(index, saved);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use current_app::VectorDiff;

    use super::*;

    fn server(key: &str, name: &str) -> SavedServerSnapshot {
        SavedServerSnapshot {
            server: ServerRef {
                service: "jellyfin".into(),
                key: key.into(),
            },
            id: format!("id-{key}"),
            name: name.into(),
            address_count: 1,
            users: Vec::new(),
            active_user: None,
        }
    }

    /// The diffs `sync` sends to go from `from` to `to`, checked by applying them.
    fn diffs(
        from: Vec<SavedServerSnapshot>,
        to: Vec<SavedServerSnapshot>,
    ) -> Vec<VectorDiff<SavedServerSnapshot>> {
        let list = ObservableList::new();
        list.reset(from.clone());
        let mut subscription = list.subscribe();
        sync(&list, to.clone());
        assert_eq!(list.snapshot(), to);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut all = Vec::new();
            // The first batch is the Reset to `from`.
            let first = subscription.next_batch().await.unwrap();
            assert_eq!(first, vec![VectorDiff::Reset { values: from }]);
            while let Ok(Some(batch)) = tokio::time::timeout(
                std::time::Duration::from_millis(20),
                subscription.next_batch(),
            )
            .await
            {
                all.extend(batch);
            }
            all
        })
    }

    #[test]
    fn a_new_server_is_appended_and_a_change_is_a_set() {
        let a = server("1", "A");
        assert_eq!(
            diffs(vec![a.clone()], vec![a.clone(), server("2", "B")]),
            vec![VectorDiff::Insert {
                index: 1,
                value: server("2", "B")
            }]
        );
        assert_eq!(
            diffs(
                vec![a.clone(), server("2", "B")],
                vec![a, server("2", "B2")]
            ),
            vec![VectorDiff::Set {
                index: 1,
                value: server("2", "B2")
            }]
        );
    }

    #[test]
    fn a_removed_server_is_removed() {
        assert_eq!(
            diffs(
                vec![server("1", "A"), server("2", "B"), server("3", "C")],
                vec![server("1", "A"), server("3", "C")]
            ),
            vec![VectorDiff::Remove { index: 1 }]
        );
    }

    #[test]
    fn a_reorder_is_a_reset() {
        let next = vec![server("2", "B"), server("1", "A")];
        assert_eq!(
            diffs(vec![server("1", "A"), server("2", "B")], next.clone()),
            vec![VectorDiff::Reset { values: next }]
        );
    }

    #[test]
    fn an_unchanged_list_sends_nothing() {
        let list = vec![server("1", "A")];
        assert_eq!(diffs(list.clone(), list), vec![]);
    }
}
