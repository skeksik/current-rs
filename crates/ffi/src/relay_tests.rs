//! Relayed playback through the bridge (ADR 0020 task 3), with the test engine, against the Jellyfin stand-in behind its gateway
//! modes. The test engine opens nothing, so these tests fetch the URL it was given as a player would, and check what reached the
//! stand-in: the gateway's credentials and the session token went upstream, and the player's URL held neither.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use current_app::test_engine::{self, TestCommand};
use jellyfin_stand_in::{
    DIRECT_PLAY_ITEM, GATE_COOKIE, GATE_COOKIE_VALUE, GATE_HEADER, GATE_HEADER_VALUE,
    GATE_PASSWORD, GATE_USER, Gate, OTHER_GATE_HEADER_VALUE, PASSWORD, STREAM_LENGTH, StreamSeen,
    TRANSCODE_ITEM, TRANSCODE_ITEM_PATH_ID, USER_NAME, library::EPISODE_IDS, master_playlist_path,
    segment_body, stream_byte,
};

use super::*;
use crate::player_tests::{DISPLAY, SIZE, engine_turn, item, recorder, signed_in, stand_in};
use crate::tests::{WAIT, block_on, eventually};

/// An answer from the relay, as a player reads it.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) head: String,
    pub(crate) body: Vec<u8>,
}

/// GETs `url` (on 127.0.0.1) as a player would, with `range` if given, on a connection of its own. An error when nothing listens.
pub(crate) fn fetch(url: &str, range: Option<&str>) -> std::io::Result<Reply> {
    let rest = url.strip_prefix("http://").expect("an http URL");
    let at = rest.find('/').unwrap_or(rest.len());
    let (authority, target) = rest.split_at(at);
    let mut stream = TcpStream::connect(authority)?;
    stream.set_read_timeout(Some(WAIT * 4))?;
    let range = range.map_or_else(String::new, |range| format!("Range: {range}\r\n"));
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: {authority}\r\n{range}Connection: close\r\n\r\n"
    )?;
    let mut data = Vec::new();
    stream.read_to_end(&mut data)?;
    let split = data
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("a whole head");
    let head = String::from_utf8_lossy(&data[..split]).to_ascii_lowercase();
    let status = head[9..12].parse().expect("a status");
    let mut body = data[split + 4..].to_vec();
    if head.contains("transfer-encoding: chunked") {
        body = unchunk(&body);
    }
    Ok(Reply { status, head, body })
}

fn unchunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let line = data.windows(2).position(|w| w == b"\r\n").unwrap();
        let size =
            usize::from_str_radix(std::str::from_utf8(&data[..line]).unwrap().trim(), 16).unwrap();
        if size == 0 {
            return out;
        }
        out.extend_from_slice(&data[line + 2..line + 2 + size]);
        data = &data[line + 4 + size..];
    }
}

/// The URL the test engine was given last.
fn loaded_url() -> String {
    test_engine::last_control()
        .unwrap()
        .loaded_urls()
        .pop()
        .expect("a load")
}

/// `http://127.0.0.1:<port>` of a relay URL.
fn relay_origin(url: &str) -> String {
    let rest = url.strip_prefix("http://").unwrap();
    format!("http://{}", &rest[..rest.find('/').unwrap()])
}

fn gate_cookie(value: &str) -> GatewayCookie {
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

/// Adds the stand-in at `address` through its gateway `gate` and signs in.
fn signed_in_through(core: &Core, address: &str, gate: Gate, header_value: &str) {
    let add = core.add_server();
    block_on(add.probe(address.into(), CancelToken::new())).unwrap();
    match gate {
        Gate::Basic => block_on(add.submit_password(
            GATE_USER.into(),
            GATE_PASSWORD.into(),
            false,
            CancelToken::new(),
        ))
        .unwrap(),
        Gate::Portal => block_on(add.submit_cookies(
            vec![gate_cookie(GATE_COOKIE_VALUE)],
            false,
            CancelToken::new(),
        ))
        .unwrap(),
        Gate::Header => block_on(add.submit_headers(
            vec![GatewayHeader {
                name: GATE_HEADER.into(),
                value: header_value.into(),
            }],
            false,
            CancelToken::new(),
        ))
        .unwrap(),
        Gate::Off => {}
    }
    assert!(
        matches!(add.state(), AddServerState::Ready { .. }),
        "{:?}",
        add.state()
    );
    let flow = add.sign_in().unwrap();
    block_on(flow.with_password(USER_NAME.into(), PASSWORD.into(), CancelToken::new())).unwrap();
    assert!(
        matches!(flow.state(), SignInState::SignedIn { .. }),
        "{:?}",
        flow.state()
    );
    eventually("an address", || {
        core.app
            .capabilities()
            .borrow()
            .servers
            .first()
            .is_some_and(|status| status.address.is_some())
    });
}

/// Plays `id` and waits until it plays.
fn play(player: &Player, id: &str) {
    block_on(player.play(item(id), 0, CancelToken::new())).unwrap();
    eventually("playing", || {
        let state = player.state();
        state.state == PlayerStateKind::Playing && state.item == Some(item(id))
    });
}

fn failed_with(player: &Player, problem: PlaybackProblem) {
    eventually("the failure", || {
        player.state().problem.as_ref() == Some(&problem)
    });
    assert_eq!(player.state().state, PlayerStateKind::Failed);
}

/// Checks that the player's URL is the relay's and holds no credential, token or `ApiKey`.
fn assert_relayed(url: &str) {
    assert!(url.starts_with("http://127.0.0.1:"), "not relayed");
    for secret in [
        "ApiKey",
        "api_key",
        "t0ken",
        GATE_COOKIE,
        GATE_COOKIE_VALUE,
        GATE_HEADER_VALUE,
        OTHER_GATE_HEADER_VALUE,
        GATE_PASSWORD,
    ] {
        assert!(!url.contains(secret), "{secret} in the player's URL");
    }
}

/// What a stream request carried upstream: Jellyfin's token in a header, and only the gateway's credential for `gate`.
fn assert_carried(seen: &StreamSeen, gate: Gate, header_value: &str) {
    assert!(!seen.target.contains("ApiKey"), "{seen:?}");
    let jellyfin = format!("{} {}", seen.authorization, seen.emby_authorization);
    assert!(jellyfin.contains("Token=\""), "{seen:?}");
    match gate {
        Gate::Basic => assert!(seen.authorization.starts_with("Basic "), "{seen:?}"),
        Gate::Portal => assert!(
            seen.cookie
                .contains(&format!("{GATE_COOKIE}={GATE_COOKIE_VALUE}")),
            "{seen:?}"
        ),
        Gate::Header => assert_eq!(seen.gate_header, header_value, "{seen:?}"),
        Gate::Off => {}
    }
}

#[test]
fn direct_play_goes_through_a_cookie_a_basic_and_a_header_gateway() {
    let _turn = engine_turn();
    for gate in [Gate::Portal, Gate::Basic, Gate::Header] {
        let stand_in = stand_in(gate);
        let core = Core::new().unwrap();
        signed_in_through(&core, stand_in.address(), gate, GATE_HEADER_VALUE);
        let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
        play(&player, DIRECT_PLAY_ITEM);
        let url = loaded_url();
        assert_relayed(&url);
        let control = test_engine::last_control().unwrap();
        // The relay's token to redact; no reference opened outside the relay for a file that is not HLS.
        assert!(control.commands().iter().any(|command| matches!(
            command,
            TestCommand::Load {
                secrets: 1,
                references: false,
                ..
            }
        )));

        // A range, as a seek asks: the server's bytes, as the server sent them.
        let reply = fetch(&url, Some("bytes=1000-1999")).unwrap();
        assert_eq!(reply.status, 206, "{gate:?}");
        assert!(
            reply
                .head
                .contains(&format!("content-range: bytes 1000-1999/{STREAM_LENGTH}"))
        );
        assert_eq!(
            reply.body,
            (1000..2000).map(stream_byte).collect::<Vec<_>>()
        );
        let whole = fetch(&url, None).unwrap();
        assert_eq!(whole.status, 200);
        assert_eq!(whole.body.len(), STREAM_LENGTH);
        let seen = stand_in.streams();
        assert_eq!(seen.len(), 2, "{gate:?}");
        assert_eq!(seen[0].range, "bytes=1000-1999");
        for seen in &seen {
            assert_carried(seen, gate, GATE_HEADER_VALUE);
        }
        // Another item's streams are not the route's, whatever the player asks.
        let other = url.replace(DIRECT_PLAY_ITEM, TRANSCODE_ITEM);
        assert_eq!(fetch(&other, None).unwrap().status, 404);
        assert_eq!(stand_in.streams().len(), 2);
        assert_eq!(player.state().state, PlayerStateKind::Playing);

        // A stop removes the route; it was the only one, so the relay stops listening.
        player.stop().unwrap();
        eventually("idle", || player.state().state == PlayerStateKind::Idle);
        assert!(
            fetch(&url, None).is_err(),
            "{gate:?}: the relay still listens"
        );
        block_on(player.close());
    }
}

#[test]
fn an_address_without_a_gateway_gets_the_direct_url() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Off);
    let core = Core::new().unwrap();
    signed_in(&core, &stand_in, false);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    play(&player, DIRECT_PLAY_ITEM);
    let url = loaded_url();
    assert!(url.starts_with(stand_in.address()), "not the server's URL");
    assert!(url.contains("ApiKey="));
    let control = test_engine::last_control().unwrap();
    assert!(control.commands().iter().any(|command| matches!(
        command,
        TestCommand::Load {
            secrets: 1,
            references: true,
            ..
        }
    )));
    // The player fetches it itself: the stand-in takes the token in the URL.
    assert_eq!(fetch(&url, Some("bytes=0-9")).unwrap().status, 206);
    block_on(player.close());
}

#[test]
fn hls_is_rewritten_through_the_relay_and_a_playlist_that_points_elsewhere_fails() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Header);
    let core = Core::new().unwrap();
    signed_in_through(&core, stand_in.address(), Gate::Header, GATE_HEADER_VALUE);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    play(&player, TRANSCODE_ITEM);
    let url = loaded_url();
    assert_relayed(&url);
    let control = test_engine::last_control().unwrap();
    // HLS opens its segments: references allowed, the relay rewrites every playlist.
    assert!(control.commands().iter().any(|command| matches!(
        command,
        TestCommand::Load {
            references: true,
            ..
        }
    )));
    let origin = relay_origin(&url);
    let master = fetch(&url, None).unwrap();
    assert_eq!(master.status, 200);
    let master = String::from_utf8(master.body).unwrap();
    let variant = master
        .lines()
        .find(|line| line.contains("main.m3u8"))
        .unwrap()
        .to_owned();
    assert!(
        variant.starts_with('/') && !variant.contains("ApiKey"),
        "{master}"
    );
    let media =
        String::from_utf8(fetch(&format!("{origin}{variant}"), None).unwrap().body).unwrap();
    let segment = media
        .lines()
        .find(|line| line.contains("/hls1/main/0.mp4"))
        .unwrap()
        .to_owned();
    let reply = fetch(&format!("{origin}{segment}"), None).unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.body,
        segment_body(&format!("/videos/{TRANSCODE_ITEM_PATH_ID}/hls1/main/0.mp4"))
    );
    for seen in stand_in.streams() {
        assert_carried(&seen, Gate::Header, GATE_HEADER_VALUE);
    }
    assert!(
        stand_in
            .playlists()
            .iter()
            .all(|seen| seen.gate_header == GATE_HEADER_VALUE && !seen.target.contains("ApiKey"))
    );

    // A playlist that names another host fails the playlist: nothing goes there, and playback fails once.
    stand_in.set_playlist(
        &master_playlist_path(),
        "#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=8000000\nhttps://cdn.example.com/main.m3u8\n",
    );
    play(&player, TRANSCODE_ITEM);
    let url = loaded_url();
    assert_eq!(fetch(&url, None).unwrap().status, 502);
    failed_with(&player, PlaybackProblem::PlaylistElsewhere);
    block_on(player.close());
}

#[test]
fn an_expired_gateway_cookie_mid_play_asks_for_the_gateway_and_keeps_the_session() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Portal);
    let core = Core::new().unwrap();
    signed_in_through(&core, stand_in.address(), Gate::Portal, "");
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    play(&player, DIRECT_PLAY_ITEM);
    let url = loaded_url();
    assert_eq!(fetch(&url, Some("bytes=0-99")).unwrap().status, 206);

    // The portal's session ends: its sign-in page comes back where media was expected. The engine fails on the 502 at once, as
    // libmpv would; the 401 rule's answer is still what the user sees, with no fallback to another route.
    stand_in.set_gate_cookie("renewed-gate-cookie");
    let plans = stand_in.count(&format!("/Items/{DIRECT_PLAY_ITEM}/PlaybackInfo"));
    assert_eq!(fetch(&url, Some("bytes=100-199")).unwrap().status, 502);
    let control = test_engine::last_control().unwrap();
    control.report_error();
    control.report_eof();
    failed_with(&player, PlaybackProblem::GatewaySignInNeeded);
    assert_eq!(
        stand_in.count(&format!("/Items/{DIRECT_PLAY_ITEM}/PlaybackInfo")),
        plans
    );
    let status = core.app.capabilities().borrow().servers[0].clone();
    assert!(status.signed_in, "{status:?}");
    assert!(status.gateway_sign_in_needed, "{status:?}");
    assert_eq!(stand_in.live_sessions(), 1);
    block_on(player.close());
}

#[test]
fn a_jellyfin_401_mid_play_ends_the_session_only_after_the_token_check() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Header);
    let core = Core::new().unwrap();
    signed_in_through(&core, stand_in.address(), Gate::Header, GATE_HEADER_VALUE);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();

    // A 401 to a token that works: Jellyfin refused this one item, and the session goes on (ADR 0015 as amended 2026-10-25).
    stand_in.refuse_stream(&format!("/Videos/{DIRECT_PLAY_ITEM}/stream"));
    play(&player, DIRECT_PLAY_ITEM);
    let checks = stand_in.count("/Users/Me");
    assert_eq!(fetch(&loaded_url(), None).unwrap().status, 502);
    failed_with(&player, PlaybackProblem::NotFound);
    assert!(stand_in.count("/Users/Me") > checks, "no token check");
    assert!(core.app.capabilities().borrow().servers[0].signed_in);

    // The session revoked. A progress report gets a 401 first and starts the session's check; the stream's 401 joins it: one probe
    // and one token check per session, and only the check's failure ends the session.
    play(&player, EPISODE_IDS[0]);
    let url = loaded_url();
    let checks = stand_in.count("/Users/Me");
    stand_in.hold("/Users/Me");
    stand_in.revoke_all();
    player.pause().unwrap();
    eventually("the token check", || {
        stand_in.count("/Users/Me") == checks + 1
    });
    assert_eq!(fetch(&url, Some("bytes=0-9")).unwrap().status, 502);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(player.state().state, PlayerStateKind::Paused);
    assert!(core.app.capabilities().borrow().servers[0].signed_in);
    stand_in.release("/Users/Me");
    failed_with(&player, PlaybackProblem::SignedOut);
    eventually("signed out", || {
        !core.app.capabilities().borrow().servers[0].signed_in
    });
    assert_eq!(
        stand_in.count("/Users/Me"),
        checks + 1,
        "a second token check"
    );
    block_on(player.close());
}

#[test]
fn an_address_switch_mid_play_goes_on_with_the_new_addresses_credentials_only() {
    let _turn = engine_turn();
    let stand_in = stand_in(Gate::Header);
    let (second, second_address) = runtime::handle()
        .unwrap()
        .block_on(stand_in.add_address())
        .unwrap();
    stand_in.set_gate_at(second, Gate::Header);
    let core = Core::new().unwrap();
    signed_in_through(&core, stand_in.address(), Gate::Header, GATE_HEADER_VALUE);
    // The same server on its other address, behind a gateway of its own.
    signed_in_through(
        &core,
        &second_address,
        Gate::Header,
        OTHER_GATE_HEADER_VALUE,
    );
    assert_eq!(core.app.servers()[0].addresses.len(), 2);
    let player = block_on(core.open_player(SIZE, DISPLAY)).unwrap();
    let (states, state_rx) = recorder();
    let _handle = player.subscribe_state(states);
    play(&player, DIRECT_PLAY_ITEM);
    let url = loaded_url();
    assert_relayed(&url);
    assert_eq!(fetch(&url, Some("bytes=0-99")).unwrap().status, 206);
    let first = stand_in.streams().last().unwrap().address;

    // The address in use goes away mid-play: the next range goes to the other one, with its own credentials.
    stand_in.close_address(first);
    let reply = fetch(&url, Some("bytes=100-199")).unwrap();
    assert_eq!(reply.status, 206);
    assert_eq!(reply.body, (100..200).map(stream_byte).collect::<Vec<_>>());
    let seen = stand_in.streams();
    let other = seen.last().unwrap().address;
    assert_ne!(other, first);
    for seen in &seen {
        let (own, other) = if seen.address == 0 {
            (GATE_HEADER_VALUE, OTHER_GATE_HEADER_VALUE)
        } else {
            (OTHER_GATE_HEADER_VALUE, GATE_HEADER_VALUE)
        };
        assert_carried(seen, Gate::Header, own);
        // The other address's credential is nowhere in the request.
        for field in [
            &seen.target,
            &seen.authorization,
            &seen.emby_authorization,
            &seen.cookie,
            &seen.gate_header,
        ] {
            assert!(!field.contains(other), "{seen:?}");
        }
    }
    assert!(seen.iter().any(|seen| seen.address == first));
    // Nothing failed meanwhile.
    assert!(
        !state_rx
            .try_iter()
            .any(|state| state.state == PlayerStateKind::Failed)
    );
    assert_eq!(player.state().state, PlayerStateKind::Playing);
    block_on(player.close());
}
