//! The settings file: `settings.json` in the data folder, versioned, saved atomically (ADR 0013).
//!
//! - A missing file starts from defaults and is written at once, so the device id is kept from the first run.
//! - An older format is migrated step by step, then saved in the current format.
//! - A file that is not valid JSON or not a settings object is moved aside to `settings.corrupt-<unix seconds>.json` and the store
//!   starts from defaults. It is never overwritten in place.
//! - A file from a newer version is read for the fields this version knows and never written: the newer version finds it unchanged.
//! - Fields this version does not know are kept and written back, so an added field never needs a migration.

use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use current_model::{
    AppTheme, SavedServer, SavedUser, ServerAddress, ServerId, ServerKey, ServerRef, ServiceType,
    UserId,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{DeviceId, StoreError};

/// The settings file's name in the data folder.
pub const SETTINGS_FILE: &str = "settings.json";
/// The format this version writes. Raise it only for a change an older version would misread, and add a step to `MIGRATIONS`.
pub const SETTINGS_VERSION: u64 = 1;

const TEMP_FILE: &str = "settings.json.tmp";
const VERSION_KEY: &str = "version";
/// A rename over a file that another process (an indexer, a virus scanner) has open can fail with "access denied" on Windows for a
/// moment. Retried a few times, 10 ms more each time: at most 100 ms in all.
const RENAME_ATTEMPTS: u32 = 5;

/// One migration step: turns the object of format `n` into format `n + 1`. The `version` key is handled by the caller.
type Migration = fn(&mut Map<String, Value>);

/// `MIGRATIONS[i]` turns format `i + 1` into format `i + 2`. Format 1 is the first, so there are none yet.
const MIGRATIONS: &[Migration] = &[];

/// The settings the core keeps. Add a field with `#[serde(default)]` on [`Stored`] and it reads older files without a migration.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    /// This install's id (ADR 0013).
    pub device_id: DeviceId,
    /// The servers the app signed in to, with their addresses and users (ADR 0015). Never a token: those live in [`Secrets`](crate::Secrets).
    pub servers: Vec<SavedServer>,
    /// The highest numeric server key ever given out, so a removed server's key is never given to another server (its keyring
    /// entries might have outlived it). Older files have none: the highest key in `servers` stands in.
    pub last_server_key: u64,
    /// The theme the app shows. A value this version does not know (a newer version's, or not a name at all) reads as
    /// [`AppTheme::System`] and is written back unchanged until the theme is changed here.
    pub app_theme: AppTheme,
    /// The stored `app_theme` this version did not know, kept for the version that wrote it.
    unknown_app_theme: Option<Value>,
    /// Fields this version does not know, written back unchanged.
    extra: Map<String, Value>,
}

impl Settings {
    fn new(device_id: DeviceId) -> Self {
        Self {
            device_id,
            servers: Vec::new(),
            last_server_key: 0,
            app_theme: AppTheme::System,
            unknown_app_theme: None,
            extra: Map::new(),
        }
    }
}

/// What the store found when it opened, for the UI to tell the user when it matters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadReport {
    /// There was no settings file: a new one was written.
    Created,
    /// The file was read as it is.
    Loaded,
    /// The file was in an older format, migrated and saved.
    Migrated { from: u64 },
    /// The file could not be read. It was kept under `backup_file` (a file name in the data folder) and the settings were reset.
    ResetCorrupt { backup_file: String },
    /// The file comes from a newer version of Current: it is used read-only and saving fails with [`StoreError::NewerVersion`].
    NewerVersion { found: u64 },
}

/// The file's shape. `version` is kept outside it: it is read before and written after.
#[derive(Serialize, Deserialize)]
struct Stored {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_id: Option<String>,
    /// Added in ADR 0015; older files have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    servers: Vec<StoredServer>,
    /// Added with "remove a server" (ADR 0015, 2026-10-23); older files have none.
    #[serde(default, skip_serializing_if = "is_zero")]
    last_server_key: u64,
    /// Added with the app theme (2026-10-24); older files have none. Any JSON value, so that one this version cannot read is kept
    /// rather than making the file corrupt: a corrupt file resets the device id, and a theme is not worth that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    app_theme: Option<Value>,
    #[serde(flatten)]
    extra: Map<String, Value>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// A saved server as the file keeps it. Private: the public shape is `current-model`'s [`SavedServer`].
#[derive(Serialize, Deserialize)]
struct StoredServer {
    service: String,
    key: String,
    /// The server's own id. Defaulted: a service without one (Stash, ADR 0014) may store none.
    #[serde(default)]
    server_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    addresses: Vec<StoredAddress>,
    #[serde(default)]
    users: Vec<StoredUser>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_user: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredAddress {
    url: String,
    #[serde(default)]
    plain_http_confirmed: bool,
}

#[derive(Serialize, Deserialize)]
struct StoredUser {
    id: String,
    #[serde(default)]
    name: String,
}

impl From<StoredServer> for SavedServer {
    fn from(stored: StoredServer) -> Self {
        Self {
            server: ServerRef::new(ServiceType::new(stored.service), ServerKey::new(stored.key)),
            id: ServerId::new(stored.server_id),
            name: stored.name,
            addresses: stored
                .addresses
                .into_iter()
                .map(|address| ServerAddress {
                    url: address.url,
                    plain_http_confirmed: address.plain_http_confirmed,
                })
                .collect(),
            users: stored
                .users
                .into_iter()
                .map(|user| SavedUser {
                    id: UserId::new(user.id),
                    name: user.name,
                })
                .collect(),
            active_user: stored.active_user.map(UserId::new),
        }
    }
}

impl From<&SavedServer> for StoredServer {
    fn from(saved: &SavedServer) -> Self {
        Self {
            service: saved.server.service.to_string(),
            key: saved.server.key.to_string(),
            server_id: saved.id.to_string(),
            name: saved.name.clone(),
            addresses: saved
                .addresses
                .iter()
                .map(|address| StoredAddress {
                    url: address.url.clone(),
                    plain_http_confirmed: address.plain_http_confirmed,
                })
                .collect(),
            users: saved
                .users
                .iter()
                .map(|user| StoredUser {
                    id: user.id.to_string(),
                    name: user.name.clone(),
                })
                .collect(),
            active_user: saved.active_user.as_ref().map(ToString::to_string),
        }
    }
}

/// The settings of one data folder. Open it once per process: it is the folder's only writer, and saves are serialized by a lock.
/// Every method does blocking file I/O.
pub struct SettingsStore {
    dir: PathBuf,
    state: Mutex<State>,
}

struct State {
    settings: Settings,
    /// The format of a newer file this store must not write over.
    newer: Option<u64>,
}

impl std::fmt::Debug for SettingsStore {
    /// Leaves out the folder: its path holds the user's account name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SettingsStore").finish_non_exhaustive()
    }
}

impl SettingsStore {
    /// Opens the settings in `dir`, creating the folder and the file if needed.
    pub fn open(dir: &Path) -> Result<(Self, LoadReport), StoreError> {
        Self::open_with(dir, MIGRATIONS, SETTINGS_VERSION)
    }

    fn open_with(
        dir: &Path,
        migrations: &[Migration],
        current: u64,
    ) -> Result<(Self, LoadReport), StoreError> {
        fs::create_dir_all(dir).map_err(StoreError::io("create the folder for"))?;
        let path = dir.join(SETTINGS_FILE);
        let bytes = match fs::read(&path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(StoreError::io("read")(error)),
        };
        let parsed = match bytes {
            None => Parsed::Missing,
            Some(bytes) => parse(&bytes, migrations, current),
        };

        let (settings, newer, report, save) = match parsed {
            Parsed::Missing => (
                Settings::new(DeviceId::generate()?),
                None,
                LoadReport::Created,
                true,
            ),
            Parsed::Corrupt => {
                let backup_file = back_up(dir)?;
                let settings = Settings::new(DeviceId::generate()?);
                (
                    settings,
                    None,
                    LoadReport::ResetCorrupt { backup_file },
                    true,
                )
            }
            Parsed::Newer { found, stored } => {
                // Never written, so a missing id made here lasts only for this run.
                let settings = match stored {
                    Some(stored) => from_stored(stored)?.0,
                    None => Settings::new(DeviceId::generate()?),
                };
                (
                    settings,
                    Some(found),
                    LoadReport::NewerVersion { found },
                    false,
                )
            }
            Parsed::Current { from, stored } => {
                let (settings, made_id) = from_stored(stored)?;
                if from < current {
                    (settings, None, LoadReport::Migrated { from }, true)
                } else {
                    (settings, None, LoadReport::Loaded, made_id)
                }
            }
        };

        let store = Self {
            dir: dir.to_owned(),
            state: Mutex::new(State { settings, newer }),
        };
        if save {
            store.write(&store.lock().settings, current)?;
        }
        Ok((store, report))
    }

    /// A copy of the current settings.
    pub fn get(&self) -> Settings {
        self.lock().settings.clone()
    }

    /// The format of a newer file this store will not write over, if it was opened on one: saves then fail with
    /// [`StoreError::NewerVersion`]. Lets callers refuse work whose result could not be saved.
    pub fn newer_version(&self) -> Option<u64> {
        self.lock().newer
    }

    /// Changes the settings and saves them. If the save fails, the settings stay as they were.
    pub fn update(&self, change: impl FnOnce(&mut Settings)) -> Result<(), StoreError> {
        let mut state = self.lock();
        if let Some(found) = state.newer {
            return Err(StoreError::NewerVersion { found });
        }
        let mut next = state.settings.clone();
        change(&mut next);
        if next.app_theme != state.settings.app_theme {
            // The theme was picked here: an unknown stored value no longer stands.
            next.unknown_app_theme = None;
        }
        if next != state.settings {
            self.write(&next, SETTINGS_VERSION)?;
            state.settings = next;
        }
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // A panic while holding the lock leaves whole settings behind (they are replaced, never edited in place), so poisoning is
        // safe to ignore.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self, settings: &Settings, version: u64) -> Result<(), StoreError> {
        let stored = Stored {
            device_id: Some(settings.device_id.to_string()),
            servers: settings.servers.iter().map(StoredServer::from).collect(),
            last_server_key: settings.last_server_key,
            app_theme: stored_theme(settings),
            extra: settings.extra.clone(),
        };
        let mut object = Map::new();
        object.insert(VERSION_KEY.to_owned(), Value::from(version));
        if let Ok(Value::Object(fields)) = serde_json::to_value(stored) {
            object.extend(fields);
        }
        // Serializing a map of JSON values cannot fail.
        let mut bytes = serde_json::to_vec_pretty(&Value::Object(object)).unwrap_or_default();
        bytes.push(b'\n');
        write_atomic(&self.dir, &bytes)
    }
}

enum Parsed {
    Missing,
    Corrupt,
    /// The file's format, after migration to the current one.
    Current {
        from: u64,
        stored: Stored,
    },
    /// `stored` is `None` when the newer file's known fields have a shape this version cannot read.
    Newer {
        found: u64,
        stored: Option<Stored>,
    },
}

fn parse(bytes: &[u8], migrations: &[Migration], current: u64) -> Parsed {
    let Ok(Value::Object(mut object)) = serde_json::from_slice::<Value>(bytes) else {
        return Parsed::Corrupt;
    };
    let Some(from) = object
        .remove(VERSION_KEY)
        .and_then(|v| v.as_u64())
        .filter(|v| *v >= 1)
    else {
        return Parsed::Corrupt;
    };
    if from > current {
        let stored = serde_json::from_value(Value::Object(object)).ok();
        return Parsed::Newer {
            found: from,
            stored,
        };
    }
    for step in migrations.iter().skip((from - 1) as usize) {
        step(&mut object);
    }
    match serde_json::from_value(Value::Object(object)) {
        Ok(stored) => Parsed::Current { from, stored },
        Err(_) => Parsed::Corrupt,
    }
}

/// The settings from the file's fields, and whether a device id had to be made because it was missing or malformed.
fn from_stored(stored: Stored) -> Result<(Settings, bool), StoreError> {
    let (device_id, made) = match stored.device_id.as_deref().and_then(DeviceId::parse) {
        Some(id) => (id, false),
        None => (DeviceId::generate()?, true),
    };
    let (app_theme, unknown_app_theme) = match stored.app_theme {
        None => (AppTheme::System, None),
        Some(value) => match value.as_str().and_then(AppTheme::parse) {
            Some(theme) => (theme, None),
            None => (AppTheme::System, Some(value)),
        },
    };
    let settings = Settings {
        device_id,
        servers: stored.servers.into_iter().map(SavedServer::from).collect(),
        last_server_key: stored.last_server_key,
        app_theme,
        unknown_app_theme,
        extra: stored.extra,
    };
    Ok((settings, made))
}

/// The `app_theme` to write: an unknown value read from the file as it was, else the theme's name. The default is left out.
fn stored_theme(settings: &Settings) -> Option<Value> {
    match (&settings.unknown_app_theme, settings.app_theme) {
        (Some(unknown), _) => Some(unknown.clone()),
        (None, AppTheme::System) => None,
        (None, theme) => Some(Value::from(theme.as_str())),
    }
}

/// Moves the unreadable file aside and returns the backup's file name.
fn back_up(dir: &Path) -> Result<String, StoreError> {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = format!("settings.corrupt-{seconds}.json");
    let mut n = 1;
    while dir.join(&name).exists() {
        n += 1;
        name = format!("settings.corrupt-{seconds}-{n}.json");
    }
    rename_with_retry(&dir.join(SETTINGS_FILE), &dir.join(&name))
        .map_err(StoreError::io("back up"))?;
    Ok(name)
}

/// Writes a temporary file next to the settings, flushes it to disk and renames it over them, so the file is always either the
/// old or the new settings, never a partial write.
fn write_atomic(dir: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let temp = dir.join(TEMP_FILE);
    let result = (|| {
        let mut file = File::create(&temp).map_err(StoreError::io("write"))?;
        file.write_all(bytes).map_err(StoreError::io("write"))?;
        file.sync_all().map_err(StoreError::io("write"))?;
        drop(file);
        rename_with_retry(&temp, &dir.join(SETTINGS_FILE)).map_err(StoreError::io("save"))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn rename_with_retry(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 1;
    loop {
        match fs::rename(from, to) {
            Err(error)
                if error.kind() == io::ErrorKind::PermissionDenied && attempt < RENAME_ATTEMPTS =>
            {
                std::thread::sleep(Duration::from_millis(10 * u64::from(attempt)));
                attempt += 1;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_json(dir: &Path) -> Value {
        serde_json::from_slice(&fs::read(dir.join(SETTINGS_FILE)).unwrap()).unwrap()
    }

    fn write_file(dir: &Path, text: &str) {
        fs::write(dir.join(SETTINGS_FILE), text).unwrap();
    }

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_new_folder_gets_a_settings_file_with_a_device_id() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Current");
        let (store, report) = SettingsStore::open(&dir).unwrap();
        assert_eq!(report, LoadReport::Created);
        let json = read_json(&dir);
        assert_eq!(json["version"], SETTINGS_VERSION);
        assert_eq!(json["device_id"], store.get().device_id.as_str());
        assert_eq!(files(&dir), [SETTINGS_FILE]);
    }

    #[test]
    fn the_device_id_survives_reopening() {
        let temp = tempfile::tempdir().unwrap();
        let first = SettingsStore::open(temp.path()).unwrap().0.get().device_id;
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert_eq!(store.get().device_id, first);
    }

    #[test]
    fn an_update_is_saved_and_leaves_no_temporary_file() {
        let temp = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(temp.path()).unwrap();
        let id = DeviceId::parse("abc").unwrap();
        store.update(|s| s.device_id = id.clone()).unwrap();
        assert_eq!(store.get().device_id, id);
        assert_eq!(
            SettingsStore::open(temp.path()).unwrap().0.get().device_id,
            id
        );
        assert_eq!(files(temp.path()), [SETTINGS_FILE]);
    }

    #[test]
    fn unknown_fields_are_kept_on_save() {
        let temp = tempfile::tempdir().unwrap();
        write_file(
            temp.path(),
            r#"{"version": 1, "device_id": "abc", "later": {"x": [1, 2]}}"#,
        );
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        store
            .update(|s| s.device_id = DeviceId::parse("def").unwrap())
            .unwrap();
        let json = read_json(temp.path());
        assert_eq!(json["device_id"], "def");
        assert_eq!(json["later"]["x"][1], 2);
    }

    #[test]
    fn a_missing_or_malformed_device_id_is_replaced_and_saved() {
        for text in [
            r#"{"version": 1}"#,
            r#"{"version": 1, "device_id": "not valid"}"#,
        ] {
            let temp = tempfile::tempdir().unwrap();
            write_file(temp.path(), text);
            let (store, report) = SettingsStore::open(temp.path()).unwrap();
            assert_eq!(report, LoadReport::Loaded);
            assert_eq!(
                read_json(temp.path())["device_id"],
                store.get().device_id.as_str()
            );
        }
    }

    #[test]
    fn a_corrupt_file_is_kept_aside_and_the_settings_reset() {
        for text in [
            "{not json",
            "[1, 2]",
            r#"{"device_id": "abc"}"#,
            r#"{"version": 0}"#,
            r#"{"version": 1, "device_id": 5}"#,
        ] {
            let temp = tempfile::tempdir().unwrap();
            write_file(temp.path(), text);
            let (store, report) = SettingsStore::open(temp.path()).unwrap();
            let LoadReport::ResetCorrupt { backup_file } = report else {
                panic!("{text}: {report:?}");
            };
            assert!(
                backup_file.starts_with("settings.corrupt-"),
                "{backup_file}"
            );
            assert_eq!(
                fs::read_to_string(temp.path().join(&backup_file)).unwrap(),
                text
            );
            assert_eq!(
                read_json(temp.path())["device_id"],
                store.get().device_id.as_str()
            );
        }
    }

    #[test]
    fn a_second_corrupt_file_in_the_same_second_gets_its_own_backup() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path(), "first");
        SettingsStore::open(temp.path()).unwrap();
        write_file(temp.path(), "second");
        SettingsStore::open(temp.path()).unwrap();
        let backups: Vec<_> = files(temp.path())
            .into_iter()
            .filter(|f| f.starts_with("settings.corrupt-"))
            .collect();
        assert_eq!(backups.len(), 2);
    }

    #[test]
    fn a_newer_file_is_read_but_never_written() {
        let temp = tempfile::tempdir().unwrap();
        let text = r#"{"version": 99, "device_id": "abc", "new_thing": true}"#;
        write_file(temp.path(), text);
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::NewerVersion { found: 99 });
        assert_eq!(store.newer_version(), Some(99));
        assert_eq!(store.get().device_id.as_str(), "abc");
        let result = store.update(|s| s.device_id = DeviceId::parse("def").unwrap());
        assert_eq!(result, Err(StoreError::NewerVersion { found: 99 }));
        assert_eq!(store.get().device_id.as_str(), "abc");
        assert_eq!(
            fs::read_to_string(temp.path().join(SETTINGS_FILE)).unwrap(),
            text
        );
    }

    #[test]
    fn a_newer_file_with_an_unreadable_shape_is_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let text = r#"{"version": 2, "device_id": {"moved": "elsewhere"}}"#;
        write_file(temp.path(), text);
        let (_, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::NewerVersion { found: 2 });
        assert_eq!(
            fs::read_to_string(temp.path().join(SETTINGS_FILE)).unwrap(),
            text
        );
        assert_eq!(files(temp.path()), [SETTINGS_FILE]);
    }

    /// A pretend format 2 that renamed `device_id` to `install_id`, and format 3 that renamed it back: the path a real migration
    /// takes, with the steps run in order from the file's format.
    #[test]
    fn an_older_format_is_migrated_step_by_step_and_saved() {
        fn to_2(object: &mut Map<String, Value>) {
            if let Some(id) = object.remove("device_id") {
                object.insert("install_id".to_owned(), id);
            }
            object.insert("step_2".to_owned(), Value::Bool(true));
        }
        fn to_3(object: &mut Map<String, Value>) {
            if let Some(id) = object.remove("install_id") {
                object.insert("device_id".to_owned(), id);
            }
        }
        let steps: &[Migration] = &[to_2, to_3];

        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path(), r#"{"version": 1, "device_id": "abc"}"#);
        let (store, report) = SettingsStore::open_with(temp.path(), steps, 3).unwrap();
        assert_eq!(report, LoadReport::Migrated { from: 1 });
        assert_eq!(store.get().device_id.as_str(), "abc");
        let json = read_json(temp.path());
        assert_eq!(json["version"], 3);
        assert_eq!(json["step_2"], true);
        assert!(json.get("install_id").is_none());

        // From format 2, only the second step runs.
        write_file(temp.path(), r#"{"version": 2, "install_id": "def"}"#);
        let (store, report) = SettingsStore::open_with(temp.path(), steps, 3).unwrap();
        assert_eq!(report, LoadReport::Migrated { from: 2 });
        assert_eq!(store.get().device_id.as_str(), "def");
        assert!(read_json(temp.path()).get("step_2").is_none());
    }

    #[test]
    fn there_is_one_migration_per_format_after_the_first() {
        assert_eq!(MIGRATIONS.len() as u64, SETTINGS_VERSION - 1);
    }

    #[test]
    fn a_stale_temporary_file_does_not_matter() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(temp.path().join(TEMP_FILE), "half a write").unwrap();
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Created);
        assert_eq!(
            read_json(temp.path())["device_id"],
            store.get().device_id.as_str()
        );
        assert_eq!(files(temp.path()), [SETTINGS_FILE]);
    }

    #[test]
    fn debug_leaves_out_the_folder() {
        let temp = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(temp.path()).unwrap();
        let text = format!("{store:?}");
        assert!(!text.contains(&*temp.path().to_string_lossy()), "{text}");
    }

    fn saved_server() -> SavedServer {
        SavedServer {
            server: ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            id: ServerId::new("0123456789abcdef0123456789abcdef"),
            name: "Test Server".into(),
            addresses: vec![
                ServerAddress {
                    url: "https://media.example.com".into(),
                    plain_http_confirmed: false,
                },
                ServerAddress {
                    url: "http://192.168.1.10:8096".into(),
                    plain_http_confirmed: true,
                },
            ],
            users: vec![
                SavedUser {
                    id: UserId::new("5d1c0000000000000000000000000001"),
                    name: "Example User".into(),
                },
                SavedUser {
                    id: UserId::new("5d1c0000000000000000000000000002"),
                    name: "Another User".into(),
                },
            ],
            active_user: Some(UserId::new("5d1c0000000000000000000000000002")),
        }
    }

    #[test]
    fn saved_servers_round_trip() {
        let temp = tempfile::tempdir().unwrap();
        let (store, _) = SettingsStore::open(temp.path()).unwrap();
        let mut signed_out = saved_server();
        signed_out.server.key = ServerKey::new("2");
        signed_out.active_user = None;
        store
            .update(|s| s.servers = vec![saved_server(), signed_out.clone()])
            .unwrap();
        drop(store);

        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert_eq!(store.get().servers, [saved_server(), signed_out]);
        let json = read_json(temp.path());
        assert_eq!(json["version"], 1);
        assert_eq!(json["servers"][0]["key"], "1");
        assert_eq!(json["servers"][0]["service"], "jellyfin");
        assert_eq!(
            json["servers"][0]["addresses"][1]["plain_http_confirmed"],
            true
        );
        assert_eq!(
            json["servers"][0]["active_user"],
            "5d1c0000000000000000000000000002"
        );
        assert!(json["servers"][1].get("active_user").is_none());
    }

    #[test]
    fn the_last_server_key_round_trips_and_older_files_read_without_it() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path(), r#"{"version": 1, "device_id": "abc"}"#);
        let (store, _) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(store.get().last_server_key, 0);
        store.update(|s| s.last_server_key = 3).unwrap();
        assert_eq!(read_json(temp.path())["last_server_key"], 3);
        drop(store);
        let (store, _) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(store.get().last_server_key, 3);
    }

    #[test]
    fn the_app_theme_round_trips_and_older_files_read_it_as_system() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path(), r#"{"version": 1, "device_id": "abc"}"#);
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert_eq!(store.get().app_theme, AppTheme::System);
        for theme in [AppTheme::Dark, AppTheme::Light] {
            store.update(|s| s.app_theme = theme).unwrap();
            assert_eq!(read_json(temp.path())["app_theme"], theme.as_str());
            assert_eq!(
                SettingsStore::open(temp.path()).unwrap().0.get().app_theme,
                theme
            );
        }
        // The default is left out of the file.
        store.update(|s| s.app_theme = AppTheme::System).unwrap();
        assert!(read_json(temp.path()).get("app_theme").is_none());
    }

    #[test]
    fn an_unknown_app_theme_reads_as_system_and_is_kept_until_changed_here() {
        for stored in [r#""sepia""#, "5", r#"{"accent": "blue"}"#] {
            let temp = tempfile::tempdir().unwrap();
            write_file(
                temp.path(),
                &format!(r#"{{"version": 1, "device_id": "abc", "app_theme": {stored}}}"#),
            );
            let (store, report) = SettingsStore::open(temp.path()).unwrap();
            assert_eq!(report, LoadReport::Loaded, "{stored}");
            assert_eq!(store.get().app_theme, AppTheme::System);
            let original: Value = serde_json::from_str(stored).unwrap();

            // Saving something else writes it back unchanged.
            store.update(|s| s.last_server_key = 1).unwrap();
            assert_eq!(read_json(temp.path())["app_theme"], original);
            // Setting the theme it already reads as is no change.
            store.update(|s| s.app_theme = AppTheme::System).unwrap();
            assert_eq!(read_json(temp.path())["app_theme"], original);

            // Picking a theme here replaces it, and back to the default leaves the field out.
            store.update(|s| s.app_theme = AppTheme::Dark).unwrap();
            assert_eq!(read_json(temp.path())["app_theme"], "dark");
            store.update(|s| s.app_theme = AppTheme::System).unwrap();
            assert!(read_json(temp.path()).get("app_theme").is_none());
        }
    }

    #[test]
    fn a_file_without_servers_reads_as_none_and_stays_without() {
        let temp = tempfile::tempdir().unwrap();
        write_file(temp.path(), r#"{"version": 1, "device_id": "abc"}"#);
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert!(store.get().servers.is_empty());
        store
            .update(|s| s.device_id = DeviceId::parse("def").unwrap())
            .unwrap();
        assert!(read_json(temp.path()).get("servers").is_none());
    }

    #[test]
    fn a_saved_server_with_only_the_required_fields_reads() {
        let temp = tempfile::tempdir().unwrap();
        write_file(
            temp.path(),
            r#"{"version": 1, "device_id": "abc", "servers": [{"service": "jellyfin", "key": "1", "server_id": "x", "later": 1}, {"service": "stash", "key": "2"}]}"#,
        );
        let (store, report) = SettingsStore::open(temp.path()).unwrap();
        assert_eq!(report, LoadReport::Loaded);
        assert_eq!(store.newer_version(), None);
        let servers = store.get().servers;
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[1].id.as_str(), "");
        assert!(servers[0].addresses.is_empty() && servers[0].users.is_empty());
        assert_eq!(servers[0].active_user, None);
    }
}
