//! Secrets (access tokens, API keys) in the OS keyring, behind a trait (ADR 0013).
//!
//! Secrets never go into the settings file: without a keyring, a sign-in works for the run but is not remembered. Secret values
//! never appear in `Debug` output or errors.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use current_model::{ServerRef, UserId};

/// The service name every entry is stored under. Windows shows it as the end of the credential's name ("<account>.Current"), macOS
/// as the keychain item's name.
pub const SERVICE_NAME: &str = "Current";

/// A secret value: a token or key. Its `Debug` output is redacted and it has no `Display`, so it cannot reach a log by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that sends it to its server.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

/// Which secret: a keyring account. Opaque, so every account is made by one of its constructors and none names anything private
/// (ADR 0013).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SecretKey {
    account: String,
}

impl SecretKey {
    /// A user's access token on a server. `user` is the id the server gave the user, never a user name, so the keyring viewer
    /// shows nothing private.
    pub fn new(server: ServerRef, user: UserId) -> Self {
        Self {
            account: format!("{server}/{user}"),
        }
    }

    /// One chunk of a gateway secret (see [`GatewayKey`](crate::GatewayKey)).
    pub(crate) fn chunk(gateway_account: &str, index: usize) -> Self {
        Self {
            account: format!("{gateway_account}/{index}"),
        }
    }

    /// The keyring account: for a token `<service type>/<server key>/<user id>`, e.g. `jellyfin/1/5d1c...`; for a gateway secret's
    /// chunk `gateway/<server key>/<address id>/<chunk>`, e.g. `gateway/1/2/0`.
    pub fn account(&self) -> &str {
        &self.account
    }
}

/// Why a keyring call failed. No variant carries the platform's error text: some of it can include the stored bytes.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    /// There is no keyring on this system, or it is locked or refused access. Sign-in still works but is not remembered.
    #[error("no keyring is available to remember the sign-in")]
    Unavailable,
    /// The secret is larger than the keyring takes (2,560 bytes in Windows Credential Manager).
    #[error("the secret is too large for the keyring")]
    TooLong,
    /// Any other keyring failure.
    #[error("the keyring failed")]
    Failed,
}

/// Where secrets are kept. Every method blocks (the OS keyring can take a while, or show a prompt): call it off the async runtime.
pub trait Secrets: Send + Sync + std::fmt::Debug {
    /// The secret, or `None` if none is stored.
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretsError>;
    /// Stores the secret, replacing any earlier one.
    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretsError>;
    /// Removes the secret. Removing one that is not there succeeds.
    fn delete(&self, key: &SecretKey) -> Result<(), SecretsError>;
    /// Whether this platform has a keyring backend at all. When it does, [`SecretsError::Unavailable`] means locked or refused,
    /// and secrets stored earlier may still be there; when it does not (Linux until its app task), nothing was ever stored.
    fn has_backend(&self) -> bool {
        true
    }
}

/// Secrets kept in memory for the life of the process: for tests. It can refuse secrets over a size, as Windows does, and fail
/// writes to chosen accounts.
#[derive(Debug, Default)]
pub struct MemorySecrets {
    entries: Mutex<HashMap<SecretKey, Secret>>,
    /// The largest secret taken, in bytes of UTF-16, as Windows Credential Manager counts a password.
    size_limit: Option<usize>,
    /// Accounts starting with one of these fail to be written or deleted.
    failing: Mutex<Vec<String>>,
    /// How many more writes or deletes succeed before every later one fails; `None`: no limit.
    writes_left: Mutex<Option<usize>>,
}

impl MemorySecrets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Refuses secrets larger than `bytes` (as UTF-16) with [`SecretsError::TooLong`], like Windows Credential Manager at 2,560.
    pub fn with_size_limit(bytes: usize) -> Self {
        Self {
            size_limit: Some(bytes),
            ..Self::default()
        }
    }

    /// Makes every later write or delete of an account starting with `prefix` fail with [`SecretsError::Failed`].
    pub fn fail_accounts_starting_with(&self, prefix: &str) {
        lock(&self.failing).push(prefix.to_owned());
    }

    /// Lets `count` more writes or deletes succeed, then makes every later one fail with [`SecretsError::Failed`]: a process that
    /// stops part-way.
    pub fn fail_writes_after(&self, count: usize) {
        *lock(&self.writes_left) = Some(count);
    }

    /// The accounts stored, sorted.
    pub fn accounts(&self) -> Vec<String> {
        let mut accounts: Vec<_> = lock(&self.entries)
            .keys()
            .map(|key| key.account().to_owned())
            .collect();
        accounts.sort();
        accounts
    }

    fn check_writable(&self, key: &SecretKey) -> Result<(), SecretsError> {
        if lock(&self.failing)
            .iter()
            .any(|prefix| key.account().starts_with(prefix.as_str()))
        {
            return Err(SecretsError::Failed);
        }
        match &mut *lock(&self.writes_left) {
            Some(0) => Err(SecretsError::Failed),
            Some(left) => {
                *left -= 1;
                Ok(())
            }
            None => Ok(()),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Secrets for MemorySecrets {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        Ok(lock(&self.entries).get(key).cloned())
    }

    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretsError> {
        self.check_writable(key)?;
        if let Some(limit) = self.size_limit
            && secret.expose().encode_utf16().count() * 2 > limit
        {
            return Err(SecretsError::TooLong);
        }
        lock(&self.entries).insert(key.clone(), secret.clone());
        Ok(())
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        self.check_writable(key)?;
        lock(&self.entries).remove(key);
        Ok(())
    }
}

/// The OS keyring: Windows Credential Manager or the macOS login keychain. On other platforms (Linux until its app task, iOS,
/// tvOS, Android) there is no backend yet and every call returns [`SecretsError::Unavailable`].
pub struct KeyringSecrets {
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    store: Option<std::sync::Arc<keyring_core::CredentialStore>>,
}

impl std::fmt::Debug for KeyringSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyringSecrets")
            .field("available", &self.is_available())
            .finish()
    }
}

impl Default for KeyringSecrets {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
mod platform {
    use std::collections::HashMap;
    use std::sync::Arc;

    use keyring_core::{CredentialStore, Entry, Error};

    use super::{SERVICE_NAME, SecretKey, SecretsError};

    #[cfg(target_os = "windows")]
    pub(super) fn store() -> Option<Arc<CredentialStore>> {
        let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new().ok()?;
        Some(store)
    }

    #[cfg(target_os = "macos")]
    pub(super) fn store() -> Option<Arc<CredentialStore>> {
        let store: Arc<CredentialStore> =
            apple_native_keyring_store::keychain::Store::new().ok()?;
        Some(store)
    }

    /// Windows: `Local` persistence keeps the credential on this computer. The store's default, `Enterprise`, roams with a domain
    /// profile, and a token belongs to this install's device id.
    #[cfg(target_os = "windows")]
    const MODIFIERS: &[(&str, &str)] = &[("persistence", "Local")];
    #[cfg(target_os = "macos")]
    const MODIFIERS: &[(&str, &str)] = &[];

    pub(super) fn entry(store: &CredentialStore, key: &SecretKey) -> Result<Entry, SecretsError> {
        let modifiers: HashMap<&str, &str> = MODIFIERS.iter().copied().collect();
        store
            .build(SERVICE_NAME, key.account(), Some(&modifiers))
            .map_err(map_error)
    }

    pub(super) fn map_error(error: Error) -> SecretsError {
        match error {
            Error::NoStorageAccess(_) | Error::NoDefaultStore => SecretsError::Unavailable,
            Error::TooLong(..) => SecretsError::TooLong,
            _ => SecretsError::Failed,
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl KeyringSecrets {
    /// Opens the OS keyring. Never fails: a keyring that cannot be opened makes every call return
    /// [`SecretsError::Unavailable`].
    pub fn new() -> Self {
        Self {
            store: platform::store(),
        }
    }

    /// Whether a keyring could be opened. Calls can still fail if it is locked or refuses access.
    pub fn is_available(&self) -> bool {
        self.store.is_some()
    }

    fn entry(&self, key: &SecretKey) -> Result<keyring_core::Entry, SecretsError> {
        let store = self.store.as_deref().ok_or(SecretsError::Unavailable)?;
        platform::entry(store, key)
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Secrets for KeyringSecrets {
    fn get(&self, key: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        match self.entry(key)?.get_password() {
            Ok(value) => Ok(Some(Secret(value))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(error) => Err(platform::map_error(error)),
        }
    }

    fn set(&self, key: &SecretKey, secret: &Secret) -> Result<(), SecretsError> {
        self.entry(key)?
            .set_password(secret.expose())
            .map_err(platform::map_error)
    }

    fn delete(&self, key: &SecretKey) -> Result<(), SecretsError> {
        match self.entry(key)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(error) => Err(platform::map_error(error)),
        }
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl KeyringSecrets {
    /// No keyring backend on this platform yet: every call returns [`SecretsError::Unavailable`].
    pub fn new() -> Self {
        Self {}
    }

    pub fn is_available(&self) -> bool {
        false
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
impl Secrets for KeyringSecrets {
    fn get(&self, _: &SecretKey) -> Result<Option<Secret>, SecretsError> {
        Err(SecretsError::Unavailable)
    }

    fn set(&self, _: &SecretKey, _: &Secret) -> Result<(), SecretsError> {
        Err(SecretsError::Unavailable)
    }

    fn delete(&self, _: &SecretKey) -> Result<(), SecretsError> {
        Err(SecretsError::Unavailable)
    }

    fn has_backend(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use current_model::{ServerKey, ServiceType};

    use super::*;

    fn key(user: &str) -> SecretKey {
        SecretKey::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            UserId::new(user),
        )
    }

    #[test]
    fn the_account_names_service_server_and_user_id() {
        assert_eq!(key("5d1c").account(), "jellyfin/1/5d1c");
    }

    #[test]
    fn debug_never_shows_a_secret() {
        let secret = Secret::new("token-value-123");
        assert_eq!(format!("{secret:?}"), "Secret(<redacted>)");
        let memory = MemorySecrets::new();
        memory.set(&key("u"), &secret).unwrap();
        let text = format!("{memory:?} {:?}", memory.get(&key("u")));
        assert!(!text.contains("token-value-123"), "{text}");
    }

    #[test]
    fn memory_secrets_round_trip() {
        let memory = MemorySecrets::new();
        assert_eq!(memory.get(&key("u")), Ok(None));
        memory.set(&key("u"), &Secret::new("a")).unwrap();
        memory.set(&key("u"), &Secret::new("b")).unwrap();
        assert_eq!(memory.get(&key("u")), Ok(Some(Secret::new("b"))));
        assert_eq!(memory.get(&key("other")), Ok(None));
        memory.delete(&key("u")).unwrap();
        memory.delete(&key("u")).unwrap();
        assert_eq!(memory.get(&key("u")), Ok(None));
    }

    /// Deletes the test's entry however the test ends, so a failure leaves nothing in the keyring.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    struct Cleanup<'a>(&'a KeyringSecrets, SecretKey);

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = self.0.delete(&self.1);
        }
    }

    /// Writes, reads, replaces and deletes a real entry under a random user id. Runs on the Windows and macOS CI runners.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn the_os_keyring_round_trips() {
        let keyring = KeyringSecrets::new();
        assert!(keyring.is_available());
        let test_key = SecretKey::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("test")),
            UserId::new(crate::DeviceId::generate().unwrap().to_string()),
        );
        let _cleanup = Cleanup(&keyring, test_key.clone());

        assert_eq!(keyring.get(&test_key), Ok(None));
        keyring.set(&test_key, &Secret::new("first")).unwrap();
        keyring.set(&test_key, &Secret::new("second")).unwrap();
        assert_eq!(keyring.get(&test_key), Ok(Some(Secret::new("second"))));
        keyring.delete(&test_key).unwrap();
        assert_eq!(keyring.get(&test_key), Ok(None));
        keyring.delete(&test_key).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn a_secret_over_the_credential_manager_limit_is_too_long() {
        let keyring = KeyringSecrets::new();
        let test_key = SecretKey::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("test")),
            UserId::new(crate::DeviceId::generate().unwrap().to_string()),
        );
        let _cleanup = Cleanup(&keyring, test_key.clone());
        assert_eq!(
            keyring.set(&test_key, &Secret::new("x".repeat(4096))),
            Err(SecretsError::TooLong)
        );
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn without_a_backend_the_keyring_is_unavailable() {
        let keyring = KeyringSecrets::new();
        assert!(!keyring.is_available());
        assert_eq!(keyring.get(&key("u")), Err(SecretsError::Unavailable));
        assert_eq!(
            keyring.set(&key("u"), &Secret::new("a")),
            Err(SecretsError::Unavailable)
        );
        assert_eq!(keyring.delete(&key("u")), Err(SecretsError::Unavailable));
    }
}
