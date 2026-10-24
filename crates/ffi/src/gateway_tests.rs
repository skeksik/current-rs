//! Authentication gateways through the bridge (ADR 0017), against the Jellyfin stand-in behind its gateway modes: setup's gateway
//! states and actions, the input problems, the status after a gateway's session ended at restore, and signing in to the gateway
//! again. Settings go to a temporary folder, secrets to memory.

use std::path::Path;
use std::sync::Arc;

use current_app::App;
use current_store::{MemorySecrets, Secrets};
use jellyfin_stand_in::{
    GATE_COOKIE, GATE_COOKIE_VALUE, GATE_HEADER, GATE_HEADER_VALUE, GATE_PASSWORD, GATE_REALM,
    GATE_USER, Gate, PASSWORD, StandIn, USER_NAME,
};

use super::*;
use crate::tests::{block_on, eventually};

fn stand_in(gate: Gate) -> StandIn {
    let stand_in = runtime::handle()
        .unwrap()
        .block_on(StandIn::start())
        .unwrap();
    stand_in.set_gate(gate);
    stand_in
}

fn core_on(dir: &Path, secrets: &Arc<MemorySecrets>) -> Arc<Core> {
    let secrets: Arc<dyn Secrets> = secrets.clone();
    let (app, report) = App::open(dir, secrets, "Example PC").unwrap();
    Arc::new(Core::with_app(
        app,
        Some(report.into()),
        runtime::handle().unwrap(),
    ))
}

fn cookie(value: &str) -> GatewayCookie {
    GatewayCookie {
        name: GATE_COOKIE.into(),
        value: value.into(),
        domain: "127.0.0.1".into(),
        path: "/".into(),
        secure: false,
        http_only: true,
        expires_unix_ms: None,
    }
}

fn header(name: &str, value: &str) -> GatewayHeader {
    GatewayHeader {
        name: name.into(),
        value: value.into(),
    }
}

fn first_server() -> ServerRef {
    ServerRef {
        service: "jellyfin".into(),
        key: "1".into(),
    }
}

fn sign_in(add: &AddServerFlow) {
    let flow = add.sign_in().unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    assert!(
        matches!(flow.state(), SignInState::SignedIn { .. }),
        "{:?}",
        flow.state()
    );
}

#[test]
fn a_basic_gateway_asks_for_its_password_through_the_bridge() {
    let stand_in = stand_in(Gate::Basic);
    let core = Core::new().unwrap();
    let add = core.add_server();
    block_on(add.probe(stand_in.address().into(), CancelToken::new())).unwrap();
    assert_eq!(
        add.state(),
        AddServerState::GatewayPassword {
            origin: stand_in.address().into(),
            realm: Some(GATE_REALM.into()),
            wrong: false,
            plain_http: false,
        }
    );
    // Nothing is sent for a user name that cannot be used.
    assert_eq!(
        block_on(add.submit_password(
            "a:b".into(),
            GATE_PASSWORD.into(),
            false,
            CancelToken::new()
        )),
        Err(CoreError::Gateway {
            problem: GatewayInputProblem::InvalidUserName
        })
    );
    block_on(add.submit_password(GATE_USER.into(), "wrong".into(), false, CancelToken::new()))
        .unwrap();
    assert!(
        matches!(
            add.state(),
            AddServerState::GatewayPassword { wrong: true, .. }
        ),
        "{:?}",
        add.state()
    );
    // Cookies are not what this state asks for.
    assert_eq!(
        block_on(add.submit_cookies(vec![cookie(GATE_COOKIE_VALUE)], false, CancelToken::new())),
        Err(CoreError::NotReady)
    );
    block_on(add.submit_password(
        GATE_USER.into(),
        GATE_PASSWORD.into(),
        false,
        CancelToken::new(),
    ))
    .unwrap();
    assert!(
        matches!(add.state(), AddServerState::Ready { .. }),
        "{:?}",
        add.state()
    );
    sign_in(&add);
    let status = &core.app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in && !status.gateway_sign_in_needed);
    // The password and its header never reach the request log.
    let log = core.diagnostics_text();
    assert!(
        !log.contains(GATE_PASSWORD) && !log.contains("Basic "),
        "{log}"
    );
}

#[test]
fn a_header_gate_takes_extra_headers_and_refuses_reserved_names() {
    let stand_in = stand_in(Gate::Header);
    let core = Core::new().unwrap();
    let add = core.add_server();
    block_on(add.probe(stand_in.address().into(), CancelToken::new())).unwrap();
    assert_eq!(
        add.state(),
        AddServerState::GatewayRefused {
            origin: stand_in.address().into(),
            plain_http: false,
        }
    );
    assert_eq!(
        block_on(add.submit_headers(vec![header("Cookie", "x")], false, CancelToken::new())),
        Err(CoreError::Gateway {
            problem: GatewayInputProblem::ReservedHeader
        })
    );
    assert_eq!(
        block_on(add.submit_headers(Vec::new(), false, CancelToken::new())),
        Err(CoreError::Gateway {
            problem: GatewayInputProblem::NoHeaders
        })
    );
    block_on(add.submit_headers(
        vec![header(GATE_HEADER, GATE_HEADER_VALUE)],
        false,
        CancelToken::new(),
    ))
    .unwrap();
    assert!(
        matches!(add.state(), AddServerState::Ready { .. }),
        "{:?}",
        add.state()
    );
    assert!(!core.diagnostics_text().contains(GATE_HEADER_VALUE));
}

#[test]
fn an_expired_portal_session_asks_for_the_gateway_again_and_the_session_goes_on() {
    let stand_in = stand_in(Gate::Portal);
    let temp = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemorySecrets::new());
    let core = core_on(temp.path(), &secrets);
    let add = core.add_server();
    block_on(add.probe(stand_in.address().into(), CancelToken::new())).unwrap();
    assert_eq!(
        add.state(),
        AddServerState::GatewaySignIn {
            url: stand_in.address().into(),
            origin: stand_in.address().into(),
            redirected: false,
            location_origin: None,
            plain_http: false,
        }
    );
    assert_eq!(
        block_on(add.follow_redirect(CancelToken::new())),
        Err(CoreError::NotReady)
    );
    // A cookie the portal does not take: still waiting.
    block_on(add.submit_cookies(vec![cookie("not-yet")], false, CancelToken::new())).unwrap();
    assert!(matches!(add.state(), AddServerState::GatewaySignIn { .. }));
    block_on(add.submit_cookies(vec![cookie(GATE_COOKIE_VALUE)], false, CancelToken::new()))
        .unwrap();
    sign_in(&add);
    drop((add, core));

    // Overnight, the portal's session ended: the restored server asks for the gateway, signed in all the same.
    stand_in.set_gate_cookie("renewed-gate-cookie");
    let core = core_on(temp.path(), &secrets);
    block_on(core.restore_sessions(CancelToken::new())).unwrap();
    eventually("the gateway asks again", || {
        core.app.capabilities().borrow().servers[0].gateway_sign_in_needed
    });
    let status = core.app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in);
    assert!(ServerStatusSnapshot::from(&status).gateway_sign_in_needed);

    let flow = core.sign_in_to_gateway(first_server()).unwrap();
    block_on(flow.start(CancelToken::new())).unwrap();
    assert!(
        matches!(
            flow.state(),
            GatewaySignInState::WebSignIn {
                redirected: false,
                ..
            }
        ),
        "{:?}",
        flow.state()
    );
    block_on(flow.submit_cookies(vec![cookie("renewed-gate-cookie")], CancelToken::new())).unwrap();
    assert_eq!(flow.state(), GatewaySignInState::Done { remembered: true });
    eventually("the status clears", || {
        !core.app.capabilities().borrow().servers[0].gateway_sign_in_needed
    });

    let unknown = ServerRef {
        service: "jellyfin".into(),
        key: "9".into(),
    };
    assert!(matches!(
        core.sign_in_to_gateway(unknown),
        Err(CoreError::UnknownServer)
    ));
    let log = core.diagnostics_text();
    assert!(
        !log.contains(GATE_COOKIE) && !log.contains("renewed-gate-cookie"),
        "{log}"
    );
}

#[test]
fn a_network_change_finds_an_ended_portal_session_without_a_restart() {
    let stand_in = stand_in(Gate::Portal);
    let core = Core::new().unwrap();
    let add = core.add_server();
    block_on(add.probe(stand_in.address().into(), CancelToken::new())).unwrap();
    block_on(add.submit_cookies(vec![cookie(GATE_COOKIE_VALUE)], false, CancelToken::new()))
        .unwrap();
    sign_in(&add);
    eventually("the address in use", || {
        core.app.capabilities().borrow().servers[0].address == Some(1)
    });

    // What the C# test does: the portal's session ends, then the network changes.
    stand_in.set_gate_cookie("renewed-gate-cookie");
    core.network_changed(NetworkFacts {
        connectivity: Connectivity::Full,
        metered: None,
        constrained: None,
        interface: InterfaceKind::Wifi,
        vpn: None,
        network_key: Some("away".into()),
    });
    eventually("the gateway asks again", || {
        core.app.capabilities().borrow().servers[0].gateway_sign_in_needed
    });
    let flow = core.sign_in_to_gateway(first_server()).unwrap();
    block_on(flow.start(CancelToken::new())).unwrap();
    block_on(flow.submit_cookies(vec![cookie("renewed-gate-cookie")], CancelToken::new())).unwrap();
    assert_eq!(flow.state(), GatewaySignInState::Done { remembered: false });
    eventually("the status clears", || {
        !core.app.capabilities().borrow().servers[0].gateway_sign_in_needed
    });
}
