//! Gateway secrets (ADR 0017): the credentials an authentication gateway in front of a server wants, per address, in the keyring.
//!
//! One secret per address, under `gateway/<server key>/<address id>`. The caller serialises whatever it has (a Basic password,
//! cookies, extra headers, or several of them) into one string; the store never looks inside. The settings file records only
//! which kinds an address has ([`ServerAddress::gateway`](current_model::ServerAddress::gateway)), never a value.
//!
//! A Windows credential holds at most 2,560 bytes (ADR 0013), and Windows' store writes a password as UTF-16, two bytes per unit
//! (windows-native-keyring-store 1.1.0, `validate_password`). Gateway cookies can be larger, so every gateway secret is stored as
//! numbered chunks, `.../0`, `.../1`, ..., of at most 1,280 UTF-16 units each, on every platform. Chunk 0 starts with a header
//! `<chunks>:<bytes>\n` (the number of chunks, and the secret's length in UTF-8 bytes). A read that finds a chunk missing, one too
//! many, a bad header or a wrong length treats the secret as absent and deletes every chunk.

use current_model::{AddressId, ServerRef};

use crate::{Secret, SecretKey, Secrets, SecretsError, SettingsStore, StoreError};

/// The most UTF-16 units one chunk holds: 2,560 bytes in Windows Credential Manager.
const CHUNK_UNITS: usize = 1280;
/// Room kept in chunk 0 for its header. Two decimal `usize`s, a colon and a newline need at most 42.
const HEADER_UNITS: usize = 48;

/// Which gateway secret: an address of a server. The address is named by its id, never its URL, so nothing private reaches the
/// keyring viewer and the secret stays with the address when it is reordered.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct GatewayKey {
    pub server: ServerRef,
    pub address: AddressId,
}

impl GatewayKey {
    pub fn new(server: ServerRef, address: AddressId) -> Self {
        Self { server, address }
    }

    /// The account the chunks are stored under: `gateway/<server key>/<address id>`, e.g. `gateway/1/2`.
    pub fn account(&self) -> String {
        format!("gateway/{}/{}", self.server.key, self.address)
    }

    fn chunk(&self, index: usize) -> SecretKey {
        SecretKey::chunk(&self.account(), index)
    }
}

/// A gateway secret, serialised by its caller. Its `Debug` output is redacted and it has no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct GatewaySecret(String);

impl GatewaySecret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value, for the one place that applies it to requests.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GatewaySecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GatewaySecret(<redacted>)")
    }
}

/// The address's gateway secret, or `None` if none is stored or what is stored is incomplete (then it is deleted). Blocking.
pub fn get_gateway_secret(
    secrets: &(impl Secrets + ?Sized),
    key: &GatewayKey,
) -> Result<Option<GatewaySecret>, SecretsError> {
    let Some(first) = secrets.get(&key.chunk(0))? else {
        // Chunks left by a write that never reached chunk 0 (written last) are removed.
        if secrets.get(&key.chunk(1))?.is_some() {
            let _ = delete_chunks(secrets, key, 0);
        }
        return Ok(None);
    };
    let Some((count, length, data)) = parse_header(first.expose()) else {
        let _ = delete_chunks(secrets, key, 1);
        return Ok(None);
    };
    let mut value = String::with_capacity(length);
    value.push_str(data);
    for index in 1..count {
        match secrets.get(&key.chunk(index))? {
            Some(chunk) => value.push_str(chunk.expose()),
            None => {
                let _ = delete_chunks(secrets, key, count);
                return Ok(None);
            }
        }
    }
    if value.len() != length || secrets.get(&key.chunk(count))?.is_some() {
        let _ = delete_chunks(secrets, key, count);
        return Ok(None);
    }
    Ok(Some(GatewaySecret(value)))
}

/// Stores the address's gateway secret, replacing any earlier one. Chunk 0 is written last, so a write cut short leaves no
/// secret that reads as whole; one that fails part-way deletes what it wrote. Blocking.
pub fn set_gateway_secret(
    secrets: &(impl Secrets + ?Sized),
    key: &GatewayKey,
    secret: &GatewaySecret,
) -> Result<(), SecretsError> {
    let old_count = stored_count(secrets, key)?;
    let chunks = split(secret.expose());
    let header = format!("{}:{}\n", chunks.len(), secret.expose().len());
    let written = (|| {
        for (index, chunk) in chunks.iter().enumerate().skip(1) {
            secrets.set(&key.chunk(index), &Secret::new(*chunk))?;
        }
        secrets.set(
            &key.chunk(0),
            &Secret::new(format!("{header}{}", chunks[0])),
        )?;
        // An earlier, longer secret's last chunks would read as one too many.
        for index in chunks.len()..old_count.max(chunks.len() + 1) {
            secrets.delete(&key.chunk(index))?;
        }
        Ok(())
    })();
    if written.is_err() {
        let _ = delete_chunks(secrets, key, chunks.len().max(old_count));
    }
    written
}

/// Removes the address's gateway secret, every chunk. Removing one that is not there succeeds. Blocking.
pub fn delete_gateway_secret(
    secrets: &(impl Secrets + ?Sized),
    key: &GatewayKey,
) -> Result<(), SecretsError> {
    let count = stored_count(secrets, key)?;
    delete_chunks(secrets, key, count)
}

/// Deletes the address's gateway secret and reports whether one may be left, as removing a server does for tokens: a delete
/// that failed, or a keyring that is unavailable on a platform with a backend. A failure counts only when `has_kinds` (the
/// address's settings list a gateway kind); without, any secret found is an orphan of an interrupted save. Blocking.
pub fn forget_gateway_secret(
    secrets: &(impl Secrets + ?Sized),
    key: &GatewayKey,
    has_kinds: bool,
) -> bool {
    match delete_gateway_secret(secrets, key) {
        Ok(()) => false,
        Err(SecretsError::Unavailable) => has_kinds && secrets.has_backend(),
        Err(SecretsError::TooLong | SecretsError::Failed) => has_kinds,
    }
}

/// What [`remove_address`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AddressRemoval {
    /// No saved server has that address.
    NotSaved,
    /// The address is gone from the settings. `credentials_left` as in [`forget_gateway_secret`].
    Removed { credentials_left: bool },
}

/// Removes one address of a saved server: from the settings first, then its gateway secret from the keyring, so a secret is
/// never left behind by an address that is still saved (ADR 0017). Blocking.
pub fn remove_address(
    settings: &SettingsStore,
    secrets: &(impl Secrets + ?Sized),
    server: &ServerRef,
    address: AddressId,
) -> Result<AddressRemoval, StoreError> {
    let mut removed = None;
    settings.update(|settings| {
        if let Some(saved) = settings
            .servers
            .iter_mut()
            .find(|saved| &saved.server == server)
            && let Some(index) = saved.addresses.iter().position(|a| a.id == Some(address))
        {
            removed = Some(saved.addresses.remove(index));
        }
    })?;
    let Some(removed) = removed else {
        return Ok(AddressRemoval::NotSaved);
    };
    let key = GatewayKey::new(server.clone(), address);
    let credentials_left = forget_gateway_secret(secrets, &key, !removed.gateway.is_empty());
    Ok(AddressRemoval::Removed { credentials_left })
}

/// The number of chunks chunk 0's header names, or 1 when there is no readable header (chunk 0 alone, if anything).
fn stored_count(
    secrets: &(impl Secrets + ?Sized),
    key: &GatewayKey,
) -> Result<usize, SecretsError> {
    Ok(secrets
        .get(&key.chunk(0))?
        .and_then(|first| parse_header(first.expose()).map(|(count, ..)| count))
        .unwrap_or(1))
}

/// Deletes chunks 0 to `at_least` and every chunk after them that is there.
fn delete_chunks(
    secrets: &(impl Secrets + ?Sized),
    key: &GatewayKey,
    at_least: usize,
) -> Result<(), SecretsError> {
    let mut result = Ok(());
    for index in 0.. {
        if index > at_least && secrets.get(&key.chunk(index))?.is_none() {
            break;
        }
        // Keep going after a failure, so as much as possible goes.
        if let Err(error) = secrets.delete(&key.chunk(index)) {
            result = Err(error);
        }
    }
    result
}

/// `(chunks, bytes, chunk 0's data)` from chunk 0, if its header is well formed.
fn parse_header(first: &str) -> Option<(usize, usize, &str)> {
    let (header, data) = first.split_once('\n')?;
    let (count, length) = header.split_once(':')?;
    let digits = |text: &str| -> Option<usize> {
        // `parse` alone would take a leading `+`.
        if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
            text.parse().ok()
        } else {
            None
        }
    };
    let count = digits(count).filter(|count| *count >= 1)?;
    Some((count, digits(length)?, data))
}

/// The secret cut into chunks on character boundaries: chunk 0 leaves room for the header. Always at least one chunk.
fn split(value: &str) -> Vec<&str> {
    let mut chunks = Vec::new();
    let mut start = 0;
    let mut units = 0;
    let mut capacity = CHUNK_UNITS - HEADER_UNITS;
    for (offset, char) in value.char_indices() {
        if units + char.len_utf16() > capacity {
            chunks.push(&value[start..offset]);
            start = offset;
            units = 0;
            capacity = CHUNK_UNITS;
        }
        units += char.len_utf16();
    }
    chunks.push(&value[start..]);
    chunks
}

#[cfg(test)]
mod tests {
    use current_model::{
        GatewayKind, SavedServer, ServerAddress, ServerId, ServerKey, ServiceType,
    };

    use super::*;
    use crate::MemorySecrets;

    /// Windows Credential Manager's limit.
    const WINDOWS_LIMIT: usize = 2560;

    fn key() -> GatewayKey {
        GatewayKey::new(
            ServerRef::new(ServiceType::JELLYFIN, ServerKey::new("1")),
            AddressId::new(2),
        )
    }

    fn memory() -> MemorySecrets {
        MemorySecrets::with_size_limit(WINDOWS_LIMIT)
    }

    fn chunk_accounts(count: usize) -> Vec<String> {
        (0..count)
            .map(|index| format!("gateway/1/2/{index}"))
            .collect()
    }

    #[test]
    fn the_account_names_server_key_and_address_id() {
        assert_eq!(key().account(), "gateway/1/2");
        assert_eq!(key().chunk(3).account(), "gateway/1/2/3");
    }

    #[test]
    fn secrets_of_any_size_round_trip_in_chunks_the_limit_takes() {
        let at_limit = "a".repeat(CHUNK_UNITS - HEADER_UNITS);
        let cookies = "c".repeat(9000);
        // Non-BMP characters (two UTF-16 units) and multi-byte ones across chunk boundaries.
        let wide = "\u{1F600}é".repeat(1500);
        for (value, chunks) in [
            ("", 1),
            ("user:password", 1),
            (at_limit.as_str(), 1),
            (&*format!("{at_limit}b"), 2),
            (cookies.as_str(), 8),
            (wide.as_str(), 4),
        ] {
            let memory = memory();
            set_gateway_secret(&memory, &key(), &GatewaySecret::new(value)).unwrap();
            assert_eq!(memory.accounts(), chunk_accounts(chunks), "{}", value.len());
            assert_eq!(
                get_gateway_secret(&memory, &key()),
                Ok(Some(GatewaySecret::new(value)))
            );
        }
    }

    #[test]
    fn a_shorter_secret_replaces_every_chunk_of_a_longer_one() {
        let memory = memory();
        set_gateway_secret(&memory, &key(), &GatewaySecret::new("x".repeat(5000))).unwrap();
        assert_eq!(memory.accounts().len(), 4);
        set_gateway_secret(&memory, &key(), &GatewaySecret::new("short")).unwrap();
        assert_eq!(memory.accounts(), chunk_accounts(1));
        assert_eq!(
            get_gateway_secret(&memory, &key()),
            Ok(Some(GatewaySecret::new("short")))
        );
    }

    #[test]
    fn delete_removes_every_chunk_and_succeeds_when_none_is_there() {
        let memory = memory();
        delete_gateway_secret(&memory, &key()).unwrap();
        set_gateway_secret(&memory, &key(), &GatewaySecret::new("x".repeat(5000))).unwrap();
        // Another address's secret stays.
        let other = GatewayKey::new(key().server, AddressId::new(20));
        set_gateway_secret(&memory, &other, &GatewaySecret::new("other")).unwrap();
        delete_gateway_secret(&memory, &key()).unwrap();
        assert_eq!(memory.accounts(), ["gateway/1/20/0"]);
        assert_eq!(get_gateway_secret(&memory, &key()), Ok(None));
    }

    fn damaged(change: impl FnOnce(&MemorySecrets)) -> MemorySecrets {
        let memory = memory();
        set_gateway_secret(&memory, &key(), &GatewaySecret::new("x".repeat(3000))).unwrap();
        assert_eq!(memory.accounts().len(), 3);
        change(&memory);
        memory
    }

    #[test]
    fn a_missing_extra_or_altered_chunk_reads_as_absent_and_is_cleaned_up() {
        type Damage = fn(&MemorySecrets);
        let cases: [(&str, Damage); 6] = [
            ("missing middle", |m| m.delete(&key().chunk(1)).unwrap()),
            ("missing last", |m| m.delete(&key().chunk(2)).unwrap()),
            ("missing first", |m| m.delete(&key().chunk(0)).unwrap()),
            ("extra", |m| {
                m.set(&key().chunk(3), &Secret::new("x")).unwrap()
            }),
            ("shortened", |m| {
                m.set(&key().chunk(2), &Secret::new("x")).unwrap()
            }),
            ("bad header", |m| {
                m.set(&key().chunk(0), &Secret::new("+3:3000\nx")).unwrap()
            }),
        ];
        for (case, change) in cases {
            let memory = damaged(change);
            assert_eq!(get_gateway_secret(&memory, &key()), Ok(None), "{case}");
            assert!(
                memory.accounts().is_empty(),
                "{case}: {:?}",
                memory.accounts()
            );
        }
    }

    #[test]
    fn a_write_that_fails_part_way_leaves_nothing() {
        let memory = memory();
        set_gateway_secret(&memory, &key(), &GatewaySecret::new("old")).unwrap();
        memory.fail_accounts_starting_with("gateway/1/2/2");
        assert_eq!(
            set_gateway_secret(&memory, &key(), &GatewaySecret::new("x".repeat(5000))),
            Err(SecretsError::Failed)
        );
        assert!(memory.accounts().is_empty(), "{:?}", memory.accounts());
        assert_eq!(get_gateway_secret(&memory, &key()), Ok(None));
    }

    #[test]
    fn debug_never_shows_a_gateway_secret() {
        let secret = GatewaySecret::new("cookie-value-123");
        assert_eq!(format!("{secret:?}"), "GatewaySecret(<redacted>)");
        let memory = memory();
        set_gateway_secret(&memory, &key(), &secret).unwrap();
        let text = format!("{memory:?} {:?}", get_gateway_secret(&memory, &key()));
        assert!(!text.contains("cookie-value-123"), "{text}");
    }

    #[test]
    fn a_failure_counts_as_left_only_for_an_address_with_kinds() {
        let memory = memory();
        assert!(!forget_gateway_secret(&memory, &key(), true));
        memory.fail_accounts_starting_with("gateway/");
        assert!(forget_gateway_secret(&memory, &key(), true));
        assert!(!forget_gateway_secret(&memory, &key(), false));
    }

    #[test]
    fn removing_an_address_saves_the_settings_then_deletes_its_secret() {
        let temp = tempfile::tempdir().unwrap();
        let (settings, _) = SettingsStore::open(temp.path()).unwrap();
        let server = key().server;
        let mut saved = SavedServer {
            server: server.clone(),
            id: ServerId::new("x"),
            name: String::new(),
            addresses: Vec::new(),
            users: Vec::new(),
            active_user: None,
            last_address_id: 1,
        };
        let mut gated = ServerAddress::new("https://media.example.com", false);
        gated.gateway = vec![GatewayKind::BASIC];
        assert_eq!(saved.add_address(gated), AddressId::new(2));
        saved.add_address(ServerAddress::new("http://192.168.1.10:8096", true));
        settings.update(|s| s.servers = vec![saved]).unwrap();
        let memory = memory();
        set_gateway_secret(&memory, &key(), &GatewaySecret::new("x".repeat(3000))).unwrap();

        assert_eq!(
            remove_address(&settings, &memory, &server, AddressId::new(2)),
            Ok(AddressRemoval::Removed {
                credentials_left: false
            })
        );
        assert!(memory.accounts().is_empty());
        let saved = &settings.get().servers[0];
        assert_eq!(saved.addresses.len(), 1);
        assert_eq!(saved.addresses[0].id, Some(AddressId::new(3)));
        assert_eq!(
            remove_address(&settings, &memory, &server, AddressId::new(2)),
            Ok(AddressRemoval::NotSaved)
        );

        // A keyring that fails leaves the settings saved and says a credential may be left, for an address with kinds.
        let mut gated = ServerAddress::new("https://media.example.com", false);
        gated.gateway = vec![GatewayKind::COOKIES];
        settings
            .update(|s| {
                s.servers[0].add_address(gated);
            })
            .unwrap();
        memory.fail_accounts_starting_with("gateway/");
        assert_eq!(
            remove_address(&settings, &memory, &server, AddressId::new(4)),
            Ok(AddressRemoval::Removed {
                credentials_left: true
            })
        );
        assert_eq!(settings.get().servers[0].addresses.len(), 1);
    }

    /// Writes, reads, shortens and deletes a chunked secret in the real keyring under a random server key, and deletes it however
    /// the test ends. Runs on the Windows and macOS CI runners, as ADR 0013's keyring test does.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn chunked_secrets_round_trip_and_delete_together_in_the_os_keyring() {
        struct Cleanup<'a>(&'a crate::KeyringSecrets, GatewayKey);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = delete_gateway_secret(self.0, &self.1);
            }
        }
        let keyring = crate::KeyringSecrets::new();
        assert!(keyring.is_available());
        let test_key = GatewayKey::new(
            ServerRef::new(
                ServiceType::JELLYFIN,
                ServerKey::new(format!("test-{}", crate::DeviceId::generate().unwrap())),
            ),
            AddressId::new(1),
        );
        let _cleanup = Cleanup(&keyring, test_key.clone());

        assert_eq!(get_gateway_secret(&keyring, &test_key), Ok(None));
        // Larger than one Windows credential: three chunks.
        let large = GatewaySecret::new("c".repeat(3000));
        set_gateway_secret(&keyring, &test_key, &large).unwrap();
        assert_eq!(get_gateway_secret(&keyring, &test_key), Ok(Some(large)));
        let short = GatewaySecret::new("user:password");
        set_gateway_secret(&keyring, &test_key, &short).unwrap();
        assert_eq!(get_gateway_secret(&keyring, &test_key), Ok(Some(short)));
        assert_eq!(keyring.get(&test_key.chunk(1)), Ok(None));

        set_gateway_secret(&keyring, &test_key, &GatewaySecret::new("x".repeat(3000))).unwrap();
        delete_gateway_secret(&keyring, &test_key).unwrap();
        for index in 0..4 {
            assert_eq!(
                keyring.get(&test_key.chunk(index)),
                Ok(None),
                "chunk {index}"
            );
        }
        assert_eq!(get_gateway_secret(&keyring, &test_key), Ok(None));
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    #[ignore = "no keyring backend on this platform yet (ADR 0013): the real keyring test runs on Windows and macOS"]
    fn chunked_secrets_round_trip_and_delete_together_in_the_os_keyring() {}
}
