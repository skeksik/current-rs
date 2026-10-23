//! The M1 flows end to end through the bridge, against the Jellyfin stand-in on 127.0.0.1: opening, adding a server, signing in
//! with a password and with Quick Connect, failures, cancelling, the saved-server list, restoring, the 401 rule, signing out and
//! removing a server. Settings go to a temporary folder, secrets to memory.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use current_app::App;
use current_store::{MemorySecrets, Secrets};
use jellyfin_stand_in::{
    PASSWORD, QUICK_CONNECT_CODE, SERVER_ID, SERVER_NAME, StandIn, USER_ID, USER_NAME,
};

use super::*;
use crate::tests::{WAIT, block_on, eventually};

/// Records what a listener receives.
struct Recorder<T>(Mutex<Sender<T>>);

fn recorder<T>() -> (Box<Recorder<T>>, Receiver<T>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(Recorder(Mutex::new(tx))), rx)
}

impl<T> Recorder<T> {
    fn send(&self, value: T) {
        let _ = self.0.lock().unwrap().send(value);
    }
}

impl AddServerStateListener for Recorder<AddServerState> {
    fn on_value(&self, value: AddServerState) {
        self.send(value);
    }
}

impl SignInStateListener for Recorder<SignInState> {
    fn on_value(&self, value: SignInState) {
        self.send(value);
    }
}

impl DiscoveryStateListener for Recorder<DiscoveryState> {
    fn on_value(&self, value: DiscoveryState) {
        self.send(value);
    }
}

impl SavedServersListener for Recorder<Vec<SavedServerDiff>> {
    fn on_diffs(&self, diffs: Vec<SavedServerDiff>) {
        self.send(diffs);
    }
}

impl CapabilitiesListener for Recorder<CapabilitiesSnapshot> {
    fn on_value(&self, value: CapabilitiesSnapshot) {
        self.send(value);
    }
}

/// Every value received so far, without waiting.
fn drain<T>(rx: &Receiver<T>) -> Vec<T> {
    rx.try_iter().collect()
}

/// Receives until `done` holds for a value; returns every value received, the last one matching.
fn until<T: std::fmt::Debug>(rx: &Receiver<T>, what: &str, done: impl Fn(&T) -> bool) -> Vec<T> {
    let mut seen = Vec::new();
    loop {
        match rx.recv_timeout(WAIT) {
            Ok(value) => {
                let matched = done(&value);
                seen.push(value);
                if matched {
                    return seen;
                }
            }
            Err(_) => panic!("timed out waiting for {what}; received {seen:?}"),
        }
    }
}

fn stand_in() -> StandIn {
    runtime::handle()
        .unwrap()
        .block_on(StandIn::start())
        .unwrap()
}

/// A core on `dir` whose secrets outlive it, so that a second core on the same folder finds the token (what the keyring does).
fn core_on(dir: &Path, secrets: &Arc<MemorySecrets>) -> Arc<Core> {
    let secrets: Arc<dyn Secrets> = secrets.clone();
    let (app, report) = App::open(dir, secrets, "Example PC").unwrap();
    Arc::new(Core::with_app(
        app,
        Some(report.into()),
        runtime::handle().unwrap(),
    ))
}

fn ready(add: &AddServerFlow, address: &str) -> NewServerInfo {
    block_on(add.probe(address.to_owned(), CancelToken::new())).unwrap();
    match add.state() {
        AddServerState::Ready { server } => server,
        other => panic!("{other:?}"),
    }
}

/// Adds the stand-in and signs in with the password; returns the sign-in flow.
fn signed_in(core: &Core, stand_in: &StandIn) -> Arc<SignInFlow> {
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in().unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    assert!(
        matches!(flow.state(), SignInState::SignedIn { .. }),
        "{:?}",
        flow.state()
    );
    flow
}

fn first_server() -> ServerRef {
    ServerRef {
        service: "jellyfin".into(),
        key: "1".into(),
    }
}

fn status(core: &Core) -> ServerStatusSnapshot {
    let (listener, rx) = recorder();
    let _handle = core.subscribe_capabilities(listener);
    rx.recv_timeout(WAIT).unwrap().servers.remove(0)
}

/// Applies diffs as a UI does.
fn apply(list: &mut Vec<SavedServerSnapshot>, diff: SavedServerDiff) {
    match diff {
        SavedServerDiff::Append { values } => list.extend(values),
        SavedServerDiff::Insert { index, value } => list.insert(index as usize, value),
        SavedServerDiff::Set { index, value } => list[index as usize] = value,
        SavedServerDiff::Remove { index } => {
            list.remove(index as usize);
        }
        SavedServerDiff::Reset { values } => *list = values,
    }
}

#[test]
fn open_core_creates_then_loads_the_settings() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().to_str().unwrap().to_owned();
    let core = block_on(open_core(
        dir.clone(),
        "Example PC".into(),
        SecretStore::Memory,
    ))
    .unwrap();
    assert_eq!(core.load_report(), Some(LoadReport::Created));
    assert!(core.servers().is_empty());
    let report = block_on(core.export_diagnostics(CancelToken::new())).unwrap();
    assert!(report.contains("\nSettings: created\n"), "{report}");
    assert!(!report.contains(&dir), "no path in {report}");
    assert!(!report.contains("Example PC"), "no device name in {report}");
    drop(core);

    let core = block_on(open_core(dir, String::new(), SecretStore::Memory)).unwrap();
    assert_eq!(core.load_report(), Some(LoadReport::Loaded));
    assert_eq!(Core::new().unwrap().load_report(), None);
}

#[test]
fn open_core_reports_a_folder_it_cannot_use() {
    let temp = tempfile::tempdir().unwrap();
    // A file where the data folder should be.
    let file = temp.path().join("file");
    std::fs::write(&file, "x").unwrap();
    let result = block_on(open_core(
        file.to_str().unwrap().to_owned(),
        String::new(),
        SecretStore::Memory,
    ));
    assert!(
        matches!(
            result,
            Err(CoreError::Store {
                problem: StoreProblem::Io { .. }
            })
        ),
        "{:?}",
        result.err()
    );
}

#[test]
fn adding_a_server_and_signing_in_with_a_password() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let core = core_on(temp.path(), &Arc::new(MemorySecrets::new()));
    let (servers_listener, servers_rx) = recorder();
    let _servers = core.subscribe_servers(servers_listener);
    let mut mirror = Vec::new();
    for diff in servers_rx.recv_timeout(WAIT).unwrap() {
        apply(&mut mirror, diff);
    }
    assert!(mirror.is_empty());

    let add = core.add_server();
    let (listener, add_rx) = recorder();
    let _add_state = add.subscribe_state(listener);
    assert!(matches!(add.sign_in().err(), Some(CoreError::NotReady)));
    let server = ready(&add, stand_in.address());
    assert_eq!(
        server,
        NewServerInfo {
            id: SERVER_ID.into(),
            name: SERVER_NAME.into(),
            version: "12.2.0".into(),
            product: "Jellyfin Server".into(),
            https_failed_tls: false,
        }
    );
    let add_states = until(&add_rx, "Ready", |state| {
        matches!(state, AddServerState::Ready { .. })
    });

    let flow = add.sign_in().unwrap();
    let (listener, sign_in_rx) = recorder();
    let _sign_in_state = flow.subscribe_state(listener);
    block_on(flow.check_quick_connect(CancelToken::new())).unwrap();
    assert_eq!(
        flow.state(),
        SignInState::Ready {
            quick_connect: Some(true)
        }
    );
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    let SignInState::SignedIn {
        server,
        user,
        remembered,
    } = flow.state()
    else {
        panic!("{:?}", flow.state())
    };
    assert_eq!(server, first_server());
    assert_eq!(user.id, USER_ID);
    assert_eq!(user.name, USER_NAME);
    assert!(remembered);

    // The saved-server list follows, as diffs.
    eventually("the list mirrors the core", || {
        for diff in drain(&servers_rx).into_iter().flatten() {
            apply(&mut mirror, diff);
        }
        mirror == core.servers() && !mirror.is_empty()
    });
    assert_eq!(
        mirror,
        vec![SavedServerSnapshot {
            server: first_server(),
            id: SERVER_ID.into(),
            name: SERVER_NAME.into(),
            address_count: 1,
            users: vec![SavedUserSnapshot {
                id: USER_ID.into(),
                name: USER_NAME.into()
            }],
            active_user: Some(USER_ID.into()),
        }]
    );
    let status = status(&core);
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);

    // Nothing that crossed the bridge holds the address or the password.
    let mut seen = format!("{add_states:?}{mirror:?}{status:?}");
    seen += &format!(
        "{:?}",
        until(&sign_in_rx, "SignedIn", |state| matches!(
            state,
            SignInState::SignedIn { .. }
        ))
    );
    let port = stand_in.address().rsplit(':').next().unwrap();
    for secret in ["127.0.0.1", port, PASSWORD] {
        assert!(!seen.contains(secret), "{secret} in {seen}");
    }
}

#[test]
fn a_wrong_password_fails_and_saves_nothing() {
    let stand_in = stand_in();
    let core = Core::new().unwrap();
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in().unwrap();
    block_on(flow.with_password(USER_NAME.into(), "wrong".into(), CancelToken::new())).unwrap();
    assert_eq!(
        flow.state(),
        SignInState::Failed {
            problem: SignInProblem::WrongCredentials
        }
    );
    assert!(core.servers().is_empty());
}

#[test]
fn a_bad_address_fails_with_its_reason() {
    let core = Core::new().unwrap();
    let add = core.add_server();
    block_on(add.probe("ftp://media.example.com".into(), CancelToken::new())).unwrap();
    assert_eq!(
        add.state(),
        AddServerState::Failed {
            problem: ProbeProblem::InvalidAddress {
                reason: AddressProblem::UnsupportedScheme
            }
        }
    );
    add.reset();
    assert_eq!(add.state(), AddServerState::Idle);
}

#[test]
fn quick_connect_shows_the_code_then_signs_in() {
    let stand_in = stand_in();
    let core = Core::new().unwrap();
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in().unwrap();
    let (listener, rx) = recorder();
    let _state = flow.subscribe_state(listener);
    block_on(flow.with_quick_connect(CancelToken::new())).unwrap();
    let states = until(&rx, "SignedIn", |state| {
        matches!(state, SignInState::SignedIn { .. })
    });
    // A slow listener may skip the code (values are latest-only); here nothing is slow.
    assert!(
        states.contains(&SignInState::QuickConnectWaiting {
            code: QUICK_CONNECT_CODE.into()
        }),
        "{states:?}"
    );
}

#[test]
fn quick_connect_turned_off_fails() {
    let stand_in = stand_in();
    stand_in.set_quick_connect(false);
    let core = Core::new().unwrap();
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in().unwrap();
    block_on(flow.check_quick_connect(CancelToken::new())).unwrap();
    assert_eq!(
        flow.state(),
        SignInState::Ready {
            quick_connect: Some(false)
        }
    );
    block_on(flow.with_quick_connect(CancelToken::new())).unwrap();
    assert_eq!(
        flow.state(),
        SignInState::Failed {
            problem: SignInProblem::QuickConnectDisabled
        }
    );
}

/// An address that accepts connections and never answers.
fn silent_address() -> (std::net::TcpListener, String) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    (listener, address)
}

#[test]
fn a_cancelled_probe_returns_to_idle() {
    let (_listener, address) = silent_address();
    let core = Core::new().unwrap();
    let add = core.add_server();
    let token = CancelToken::new();
    let canceller = Arc::clone(&token);
    let flow = Arc::clone(&add);
    let probing = thread::spawn(move || block_on(flow.probe(address, token)));
    eventually("probing", || add.state() == AddServerState::Probing);
    canceller.cancel();
    assert_eq!(probing.join().unwrap(), Err(CoreError::Cancelled));
    assert_eq!(add.state(), AddServerState::Idle);
}

#[test]
fn a_cancel_leaves_a_newer_probe_alone() {
    let (_listener, address) = silent_address();
    let stand_in = stand_in();
    let core = Core::new().unwrap();
    let add = core.add_server();
    let token = CancelToken::new();
    let canceller = Arc::clone(&token);
    let flow = Arc::clone(&add);
    let older = thread::spawn(move || block_on(flow.probe(address, token)));
    eventually("probing", || add.state() == AddServerState::Probing);
    ready(&add, stand_in.address());
    canceller.cancel();
    assert_eq!(older.join().unwrap(), Err(CoreError::Cancelled));
    assert!(matches!(add.state(), AddServerState::Ready { .. }));
}

#[test]
fn a_cancelled_quick_connect_returns_to_ready() {
    let stand_in = stand_in();
    let core = Core::new().unwrap();
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in().unwrap();
    let token = CancelToken::new();
    token.cancel();
    assert_eq!(
        block_on(flow.with_quick_connect(token)),
        Err(CoreError::Cancelled)
    );
    assert!(matches!(flow.state(), SignInState::Ready { .. }));
}

#[test]
fn a_second_core_restores_the_session() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = core_on(temp.path(), &secrets);
    signed_in(&core, &stand_in);
    drop(core);

    let core = core_on(temp.path(), &secrets);
    assert_eq!(core.load_report(), Some(LoadReport::Loaded));
    assert_eq!(core.servers()[0].active_user.as_deref(), Some(USER_ID));
    block_on(core.restore_sessions(CancelToken::new())).unwrap();
    let status = status(&core);
    assert!(status.signed_in);
    assert_eq!(status.reachability, Reachability::Reachable);
    assert_eq!(status.user.unwrap().name, USER_NAME);
}

#[test]
fn a_revoked_session_is_signed_out_at_restore_and_can_sign_in_again() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = core_on(temp.path(), &secrets);
    signed_in(&core, &stand_in);
    drop(core);
    stand_in.revoke_all();

    let core = core_on(temp.path(), &secrets);
    block_on(core.restore_sessions(CancelToken::new())).unwrap();
    eventually("signed out", || !status(&core).signed_in);
    eventually("the active user is cleared", || {
        core.servers()[0].active_user.is_none()
    });

    let unknown = ServerRef {
        service: "jellyfin".into(),
        key: "9".into(),
    };
    assert!(matches!(
        core.sign_in_again(unknown).err(),
        Some(CoreError::UnknownServer)
    ));
    let flow = core.sign_in_again(first_server()).unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    assert!(matches!(flow.state(), SignInState::SignedIn { .. }));
    assert!(status(&core).signed_in);
}

#[test]
fn sign_out_tells_the_server_and_clears_the_user() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let core = core_on(temp.path(), &Arc::new(MemorySecrets::new()));
    signed_in(&core, &stand_in);
    assert_eq!(stand_in.live_sessions(), 1);

    let result = block_on(core.sign_out(first_server())).unwrap();
    assert_eq!(result, SignOutResult { server_told: true });
    assert_eq!(stand_in.live_sessions(), 0);
    assert!(!status(&core).signed_in);
    eventually("the list shows no active user", || {
        core.servers()[0].active_user.is_none()
    });

    // Signed out already: the server is not told again, and that is no error.
    let again = block_on(core.sign_out(first_server())).unwrap();
    assert_eq!(again, SignOutResult { server_told: false });
    assert!(matches!(
        block_on(core.sign_out(ServerRef {
            service: "jellyfin".into(),
            key: "9".into()
        })),
        Err(CoreError::UnknownServer)
    ));
}

#[test]
fn removing_a_server_signs_out_and_forgets_it() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = core_on(temp.path(), &secrets);
    signed_in(&core, &stand_in);
    assert_eq!(stand_in.live_sessions(), 1);
    let (listener, rx) = recorder();
    let _servers = core.subscribe_servers(listener);
    let mut list = Vec::new();
    for diff in until(&rx, "the saved server", |diffs| !diffs.is_empty())
        .into_iter()
        .flatten()
    {
        apply(&mut list, diff);
    }
    assert_eq!(list.len(), 1);

    let result = block_on(core.remove_server(first_server())).unwrap();
    assert_eq!(
        result,
        RemoveServerResult {
            removed: true,
            credentials_left: false
        }
    );
    // The list, the capabilities state and the keyring forget it at once; the server hears of it in the background.
    for diff in until(&rx, "the removal", |diffs| !diffs.is_empty())
        .into_iter()
        .flatten()
    {
        apply(&mut list, diff);
    }
    assert!(list.is_empty());
    assert!(core.servers().is_empty());
    let (listener, capabilities) = recorder();
    let _capabilities = core.subscribe_capabilities(listener);
    assert!(capabilities.recv_timeout(WAIT).unwrap().servers.is_empty());
    let key = current_store::SecretKey::new(
        (&first_server()).into(),
        current_model::UserId::new(USER_ID),
    );
    assert!(secrets.get(&key).unwrap().is_none());
    eventually("the server is told", || stand_in.live_sessions() == 0);

    // Removing it again, or a server never saved, does nothing.
    let again = block_on(core.remove_server(first_server())).unwrap();
    assert!(!again.removed);

    // A second core on the same folder does not bring it back.
    drop(core);
    let core = core_on(temp.path(), &secrets);
    assert!(core.servers().is_empty());
}

#[test]
fn removing_a_server_with_settings_from_a_newer_version_fails() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("settings.json"),
        format!(
            concat!(
                r#"{{"version": 99, "device_id": "abc", "#,
                r#""servers": [{{"service": "jellyfin", "key": "1", "server_id": "{}"}}]}}"#
            ),
            SERVER_ID
        ),
    )
    .unwrap();
    let core = core_on(temp.path(), &Arc::new(MemorySecrets::new()));
    assert!(matches!(
        block_on(core.remove_server(first_server())),
        Err(CoreError::Store {
            problem: StoreProblem::NewerVersion { found: 99 }
        })
    ));
    assert_eq!(core.servers().len(), 1);
}

#[test]
fn network_reports_are_accepted() {
    let core = Core::new().unwrap();
    core.network_changed(NetworkFacts {
        connectivity: Connectivity::Full,
        metered: Some(false),
        constrained: None,
        interface: InterfaceKind::Wifi,
        vpn: None,
        network_key: Some("home".into()),
    });
    core.returned_to_foreground();
}

#[test]
fn network_facts_convert() {
    let facts: current_model::NetworkFacts = NetworkFacts {
        connectivity: Connectivity::Offline,
        metered: None,
        constrained: Some(true),
        interface: InterfaceKind::Cellular,
        vpn: Some(true),
        network_key: None,
    }
    .into();
    assert_eq!(facts.connectivity, current_model::Connectivity::None);
    assert_eq!(facts.interface, current_model::InterfaceKind::Cellular);
    assert_eq!((facts.constrained, facts.vpn), (Some(true), Some(true)));
}

#[test]
fn discovery_ends_and_a_pick_needs_a_listed_server() {
    let core = Core::new().unwrap();
    let discovery = core.discover_servers();
    let (listener, rx) = recorder();
    let _state = discovery.subscribe_state(listener);
    block_on(discovery.search(CancelToken::new())).unwrap();
    // Whether the container may broadcast, and whether a server answers, depends on the machine: only the shape is checked.
    // The first delivery may already be `Searching`: values are latest-only.
    until(&rx, "the search ends", |state| {
        matches!(
            state,
            DiscoveryState::Done { .. } | DiscoveryState::Failed { .. }
        )
    });

    let add = core.add_server();
    assert!(!add.pick(Arc::clone(&discovery), "not-listed".into()));
    assert_eq!(add.state(), AddServerState::Idle);
    discovery.reset();
    assert_eq!(discovery.state(), DiscoveryState::Idle);
}

#[test]
fn a_cancelled_search_returns_to_idle() {
    let core = Core::new().unwrap();
    let discovery = core.discover_servers();
    let token = CancelToken::new();
    let canceller = Arc::clone(&token);
    let flow = Arc::clone(&discovery);
    let search = thread::spawn(move || block_on(flow.search(token)));
    thread::sleep(Duration::from_millis(100));
    canceller.cancel();
    let result = search.join().unwrap();
    // On a machine where the search fails at once, it may end before the cancel.
    if result == Err(CoreError::Cancelled) {
        assert_eq!(discovery.state(), DiscoveryState::Idle);
    } else {
        assert!(matches!(discovery.state(), DiscoveryState::Failed { .. }));
    }
}

#[test]
fn flow_listeners_are_counted_and_released() {
    let core = Core::new().unwrap();
    let add = core.add_server();
    let (listener, rx) = recorder::<AddServerState>();
    let handle = add.subscribe_state(listener);
    rx.recv_timeout(WAIT).unwrap();
    let (listener, _servers) = recorder::<Vec<SavedServerDiff>>();
    let servers = core.subscribe_servers(listener);
    assert_eq!(core.live_listeners(), 2);
    handle.cancel();
    servers.cancel();
    eventually("both are released", || core.live_listeners() == 0);
}

#[test]
fn signing_out_of_a_server_that_is_gone_still_signs_out_here() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let core = core_on(temp.path(), &Arc::new(MemorySecrets::new()));
    signed_in(&core, &stand_in);
    drop(stand_in);

    // The server answers nothing (the app reports Network(Unreachable)): signed out on this device all the same.
    let result = block_on(core.sign_out(first_server())).unwrap();
    assert_eq!(result, SignOutResult { server_told: false });
    assert!(!status(&core).signed_in);
    eventually("the list shows no active user", || {
        core.servers()[0].active_user.is_none()
    });
}

#[test]
fn cancelling_while_signing_in_returns_to_ready_and_saves_nothing() {
    const SIGN_IN: &str = "/Users/AuthenticateByName";
    let stand_in = stand_in();
    let core = Core::new().unwrap();
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in().unwrap();
    stand_in.hold(SIGN_IN);
    let token = CancelToken::new();
    let call = {
        let (flow, token) = (Arc::clone(&flow), Arc::clone(&token));
        thread::spawn(move || {
            block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), token))
        })
    };
    eventually("the request waits at the server", || {
        stand_in.count(SIGN_IN) == 1 && flow.state() == SignInState::SigningIn
    });
    token.cancel();
    assert_eq!(call.join().unwrap(), Err(CoreError::Cancelled));
    assert!(
        matches!(flow.state(), SignInState::Ready { .. }),
        "{:?}",
        flow.state()
    );

    // The server answers after all: too late, nothing is saved.
    stand_in.release(SIGN_IN);
    thread::sleep(Duration::from_millis(200));
    assert!(matches!(flow.state(), SignInState::Ready { .. }));
    assert!(core.servers().is_empty());
}

#[test]
fn cancelling_while_quick_connect_waits_stops_polling() {
    const CONNECT: &str = "/QuickConnect/Connect";
    let stand_in = stand_in();
    stand_in.set_quick_connect_pending(true);
    let core = Core::new().unwrap();
    let add = core.add_server();
    ready(&add, stand_in.address());
    let flow = add.sign_in_polling_every(Duration::from_millis(20));
    let token = CancelToken::new();
    let call = {
        let (flow, token) = (Arc::clone(&flow), Arc::clone(&token));
        thread::spawn(move || block_on(flow.with_quick_connect(token)))
    };
    eventually("polling", || {
        stand_in.count(CONNECT) >= 2
            && flow.state()
                == SignInState::QuickConnectWaiting {
                    code: QUICK_CONNECT_CODE.into(),
                }
    });
    token.cancel();
    assert_eq!(call.join().unwrap(), Err(CoreError::Cancelled));
    assert!(
        matches!(flow.state(), SignInState::Ready { .. }),
        "{:?}",
        flow.state()
    );
    let polls = stand_in.count(CONNECT);
    thread::sleep(Duration::from_millis(200));
    assert_eq!(
        stand_in.count(CONNECT),
        polls,
        "no polling after the cancel"
    );
}

#[test]
fn a_cancelled_restore_still_finishes_its_check() {
    const ME: &str = "/Users/Me";
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = core_on(temp.path(), &secrets);
    signed_in(&core, &stand_in);
    drop(core);

    stand_in.hold(ME);
    let core = core_on(temp.path(), &secrets);
    let token = CancelToken::new();
    let call = {
        let (core, token) = (Arc::clone(&core), Arc::clone(&token));
        thread::spawn(move || block_on(core.restore_sessions(token)))
    };
    eventually("the user check waits at the server", || {
        stand_in.count(ME) == 1
    });
    token.cancel();
    assert_eq!(call.join().unwrap(), Err(CoreError::Cancelled));

    // Cancelling stopped only the waiting: the check gets its answer and confirms the user (the server's policy, not the saved
    // name only).
    stand_in.release(ME);
    eventually("the user is confirmed", || {
        status(&core)
            .user
            .is_some_and(|user| user.permissions.contains(&"EnableMediaPlayback".to_owned()))
    });
    assert_eq!(stand_in.count(ME), 1);
}

#[test]
fn diagnostics_after_a_sign_in_hold_nothing_private() {
    let stand_in = stand_in();
    let temp = tempfile::tempdir().unwrap();
    let core = core_on(temp.path(), &Arc::new(MemorySecrets::new()));
    signed_in(&core, &stand_in);
    let report = block_on(core.export_diagnostics(CancelToken::new())).unwrap();
    assert!(report.contains("/Users/AuthenticateByName"), "{report}");
    let port = stand_in.address().rsplit(':').next().unwrap();
    let dir = temp.path().to_str().unwrap();
    // The device name goes to the server in every request's header; it must not reach the report.
    for private in ["Example PC", PASSWORD, "127.0.0.1", port, dir, USER_NAME] {
        assert!(!report.contains(private), "{private} in {report}");
    }
}
