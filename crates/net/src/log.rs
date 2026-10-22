//! The request log: the last requests of every server, redacted so it can be pasted into a bug report (ADR 0006).
//!
//! An entry is built from the request as the calling crate made it, before the server's address and credentials are added, so it can
//! hold neither. Query values are replaced unless their key is on a short list of harmless keys.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use current_model::ServerRef;

/// What replaces a query value that is not known to be harmless.
pub const REDACTED: &str = "[redacted]";

/// Query keys whose values are kept: paging, sorting and field selection, which hold no user text and no secrets. Compared without
/// case. Everything else, search terms included, is redacted.
const HARMLESS_QUERY_KEYS: &[&str] = &[
    "limit",
    "startindex",
    "sortby",
    "sortorder",
    "fields",
    "includeitemtypes",
    "excludeitemtypes",
    "recursive",
    "page",
    "take",
    "skip",
    "enableimages",
    "enableuserdata",
    "imagetypelimit",
];

/// How many entries a log keeps by default.
pub const DEFAULT_CAPACITY: usize = 200;

/// How a logged request ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The server answered with this status.
    Status(u16),
    /// No answer, or an answer that could not be used; the error's kind.
    Error(&'static str),
}

/// One request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogEntry {
    pub at: SystemTime,
    /// The server, by its app-assigned key: stands in for the host, which is never stored.
    pub server: ServerRef,
    /// The 1-based place, in the user's list, of the address the request went to; `None` for a server with one address or a
    /// request that found no address. Stands in for the address, which is never stored.
    pub address: Option<u8>,
    pub method: String,
    /// The path relative to the server's address, as the calling crate gave it.
    pub path: String,
    /// Query pairs with every value outside the harmless keys replaced by [`REDACTED`].
    pub query: Vec<(String, String)>,
    pub outcome: Outcome,
    pub duration: Duration,
}

impl LogEntry {
    /// One line: time, server, address position, method, path with redacted query, outcome and duration.
    pub fn to_line(&self) -> String {
        let mut line = format!("{} {}", format_utc(self.at), self.server);
        if let Some(position) = self.address {
            let _ = write!(line, " address {position}");
        }
        let _ = write!(line, " {} {}", self.method, self.path);
        for (index, (key, value)) in self.query.iter().enumerate() {
            let separator = if index == 0 { '?' } else { '&' };
            let _ = write!(line, "{separator}{key}={value}");
        }
        let _ = match &self.outcome {
            Outcome::Status(status) => write!(line, " -> {status}"),
            Outcome::Error(kind) => write!(line, " -> {kind}"),
        };
        let _ = write!(line, " ({} ms)", self.duration.as_millis());
        line
    }
}

/// Redacts query pairs for the log.
pub fn redact_query(query: &[(String, String)]) -> Vec<(String, String)> {
    query
        .iter()
        .map(|(key, value)| {
            let harmless = HARMLESS_QUERY_KEYS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(key));
            (
                key.clone(),
                if harmless {
                    value.clone()
                } else {
                    REDACTED.to_owned()
                },
            )
        })
        .collect()
}

/// A bounded log shared by every server's client: the oldest entry goes when a new one arrives at capacity.
#[derive(Debug)]
pub struct RequestLog {
    capacity: usize,
    entries: Mutex<VecDeque<LogEntry>>,
}

impl Default for RequestLog {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl RequestLog {
    /// A log keeping the last `capacity` entries (at least one).
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            entries: Mutex::new(VecDeque::with_capacity(capacity)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn push(&self, entry: LogEntry) {
        let mut entries = self.lock();
        while entries.len() >= self.capacity {
            entries.pop_front();
        }
        entries.push_back(entry);
    }

    /// The entries, oldest first.
    pub fn entries(&self) -> Vec<LogEntry> {
        self.lock().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.lock().clear();
    }

    /// The whole log as text, one entry per line, oldest first: what "copy diagnostics" puts on the clipboard.
    pub fn to_text(&self) -> String {
        self.lock()
            .iter()
            .map(|entry| entry.to_line() + "\n")
            .collect()
    }

    fn lock(&self) -> MutexGuard<'_, VecDeque<LogEntry>> {
        // A panic while holding the lock cannot leave the queue half-changed (each change is one call), so a poisoned lock is safe.
        self.entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` in UTC.
fn format_utc(at: SystemTime) -> String {
    let since_epoch = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = since_epoch.as_secs();
    let (days, rest) = (seconds / 86_400, seconds % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60,
        since_epoch.subsec_millis()
    )
}

/// The calendar date `days` after 1970-01-01, by Howard Hinnant's `civil_from_days`
/// (https://howardhinnant.github.io/date_algorithms.html, read 2026-10-22), for dates on or after the epoch.
fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use current_model::{ServerKey, ServiceType};

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn entry(path: &str) -> LogEntry {
        LogEntry {
            at: UNIX_EPOCH + Duration::from_millis(1_761_100_000_123),
            server: ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            address: None,
            method: "GET".into(),
            path: path.into(),
            query: redact_query(&pairs(&[("searchTerm", "Some Title"), ("Limit", "20")])),
            outcome: Outcome::Status(200),
            duration: Duration::from_millis(42),
        }
    }

    #[test]
    fn query_values_are_redacted_unless_harmless() {
        let redacted = redact_query(&pairs(&[
            ("api_key", "secret-key"),
            ("ApiKey", "secret-key"),
            ("token", "secret-token"),
            ("password", "hunter2"),
            ("searchTerm", "Some Title"),
            ("someFutureParam", "x"),
            ("Limit", "20"),
            ("sortBy", "SortName"),
            ("fields", "Overview"),
        ]));
        assert_eq!(
            redacted,
            pairs(&[
                ("api_key", REDACTED),
                ("ApiKey", REDACTED),
                ("token", REDACTED),
                ("password", REDACTED),
                ("searchTerm", REDACTED),
                ("someFutureParam", REDACTED),
                ("Limit", "20"),
                ("sortBy", "SortName"),
                ("fields", "Overview"),
            ])
        );
    }

    #[test]
    fn a_line_has_time_server_request_outcome_and_duration() {
        assert_eq!(
            entry("/Items").to_line(),
            "2025-10-22T02:26:40.123Z jellyfin/1 GET /Items?searchTerm=[redacted]&Limit=20 -> 200 (42 ms)"
        );
        let positioned = LogEntry {
            address: Some(2),
            ..entry("/Items")
        };
        assert!(
            positioned
                .to_line()
                .contains(" jellyfin/1 address 2 GET /Items?"),
            "{}",
            positioned.to_line()
        );
        let failed = LogEntry {
            query: Vec::new(),
            outcome: Outcome::Error("timeout"),
            ..entry("/System/Info")
        };
        assert!(
            failed
                .to_line()
                .ends_with("GET /System/Info -> timeout (42 ms)")
        );
    }

    #[test]
    fn the_log_keeps_only_the_newest_entries() {
        let log = RequestLog::new(2);
        for path in ["/a", "/b", "/c"] {
            log.push(entry(path));
        }
        let paths: Vec<_> = log.entries().into_iter().map(|entry| entry.path).collect();
        assert_eq!(paths, ["/b", "/c"]);
        assert_eq!(log.to_text().lines().count(), 2);
        log.clear();
        assert!(log.to_text().is_empty());
        assert_eq!(RequestLog::new(0).capacity(), 1);
    }

    #[test]
    fn dates_are_calendar_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
        assert_eq!(format_utc(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }
}
