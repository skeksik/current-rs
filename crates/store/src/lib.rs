//! What the core keeps between runs (ADR 0013): the settings file, secrets in the OS keyring (tokens, and gateway secrets per
//! address, ADR 0017) and the device identity.
//!
//! The UI passes the data folder at start: it knows how the app was packaged (unpackaged or MSIX, sandboxed, Flatpak, Android).
//! Everything here is blocking file or keyring I/O behind a sync API. Callers on an async path run it on a blocking thread
//! (`spawn_blocking`); the store never blocks a runtime thread itself (ADR 0002, portability).

mod browse;
mod device;
mod error;
mod gateway;
mod playback;
mod secrets;
mod settings;

pub use browse::{
    BrowseSettings, KindPage, LibraryChoices, RememberedSearchFilter, RememberedSort,
    RememberedView, SearchChoices,
};
pub use device::DeviceId;
pub use error::StoreError;
pub use gateway::{
    AddressRemoval, GatewayKey, GatewaySecret, delete_gateway_secret, forget_gateway_secret,
    get_gateway_secret, remove_address, set_gateway_secret,
};
pub use playback::{PlaybackSettings, StreamQuality};
pub use secrets::{
    KeyringSecrets, MemorySecrets, SERVICE_NAME, Secret, SecretKey, Secrets, SecretsError,
};
pub use settings::{LoadReport, SETTINGS_FILE, SETTINGS_VERSION, Settings, SettingsStore};
