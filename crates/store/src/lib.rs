//! What the core keeps between runs (ADR 0013): the settings file, secrets in the OS keyring and the device identity.
//!
//! The UI passes the data folder at start: it knows how the app was packaged (unpackaged or MSIX, sandboxed, Flatpak, Android).
//! Everything here is blocking file or keyring I/O behind a sync API. Callers on an async path run it on a blocking thread
//! (`spawn_blocking`); the store never blocks a runtime thread itself (ADR 0002, portability).

mod device;
mod error;
mod secrets;
mod settings;

pub use device::DeviceId;
pub use error::StoreError;
pub use secrets::{
    KeyringSecrets, MemorySecrets, SERVICE_NAME, Secret, SecretKey, Secrets, SecretsError,
};
pub use settings::{LoadReport, SETTINGS_FILE, SETTINGS_VERSION, Settings, SettingsStore};
